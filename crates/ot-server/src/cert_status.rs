//! Whether a control-plane client certificate is still good: OCSP first,
//! the revocation lists (`OT_TLS_CLIENT_CRL`) when OCSP gives no usable
//! answer, and refusal when neither can say (fail closed).
//!
//! rustls verifies certificates synchronously inside the handshake, so an
//! OCSP query cannot run there. The handshake checks the path to
//! `OT_TLS_CLIENT_CA` only (no lists: a stale list would refuse before OCSP
//! could answer); then the connection's own task calls [`CertStatus::check`]
//! before any request is served, and a refusal drops the connection. Each
//! connection has its own tokio task, so a slow responder holds up only the
//! connections waiting on it, never the TLS thread pool.
//!
//! The responder is `OT_TLS_CLIENT_OCSP_URL` if set, else the certificate's
//! authority information access (id-ad-ocsp) URL. An answer must be signed
//! by the issuing CA, or by a responder certificate that CA issued directly
//! with id-kp-OCSPSigning; name our certificate; carry our nonce if it
//! carries one; and be current. Good and revoked answers are cached until
//! their nextUpdate (at most an hour); a browser's parallel connections
//! share one query.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ot_store::audit::AuditEvent;
use rustls::pki_types::{CertificateDer, SignatureVerificationAlgorithm, UnixTime};
use rustls::server::danger::ClientCertVerifier;
use serde_json::json;

use crate::ocsp;

/// Where audit events go (the audit record, in the server).
pub type AuditSink = Arc<dyn Fn(AuditEvent) + Send + Sync>;

/// How long a query may take, connection to last byte.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// After a responder fails to answer, the lists are used without asking it
/// for this long (so a dead responder doesn't cost every sign-in 5 s).
const DOWN_FOR: Duration = Duration::from_secs(30);
/// The largest answer read.
const MAX_ANSWER: usize = 64 * 1024;
/// How long an answer is kept at most, whatever its nextUpdate says.
const KEEP_AT_MOST: Duration = Duration::from_secs(3600);
/// How long an answer without a nextUpdate is kept.
const KEEP_WITHOUT_NEXT_UPDATE: Duration = Duration::from_secs(300);
/// Cached answers kept at most (expired ones go first, then the oldest).
const MAX_CACHED: usize = 10_000;

struct Cached {
    revoked: bool,
    until: SystemTime,
    at: Instant,
}

/// Why OCSP gave no answer to go by.
enum NoAnswer {
    /// The responder couldn't be reached or answered with an HTTP error.
    Down(String),
    /// It answered, but not usably (or `unknown`).
    Unusable(String),
}

pub struct CertStatus {
    /// `OT_TLS_CLIENT_CA`'s certificates (possible issuers).
    cas: Vec<CertificateDer<'static>>,
    /// `OT_TLS_CLIENT_OCSP_URL`: asked instead of the certificate's own.
    url: Option<String>,
    /// The verifier with the revocation lists; replaced when they change.
    crl: RwLock<Option<Arc<dyn ClientCertVerifier>>>,
    cache: Mutex<HashMap<Vec<u8>, Cached>>,
    /// One query at a time per certificate: the rest wait and use its answer.
    flights: Mutex<HashMap<Vec<u8>, Arc<tokio::sync::Mutex<()>>>>,
    /// Responders that recently failed to answer.
    down: Mutex<HashMap<String, Instant>>,
    http: reqwest::Client,
    /// The address each responder was last reached at, logged when it changes.
    peers: ot_core::netlog::PeerLog,
    algs: &'static [&'static dyn SignatureVerificationAlgorithm],
    audit: Option<AuditSink>,
}

/// The certificate being checked, for logs and the audit record.
struct Who {
    peer: SocketAddr,
    actor: String,
    subject: String,
    serial: String,
}

impl Who {
    fn event(&self, op: &str, detail: serde_json::Value) -> AuditEvent {
        let mut d = json!({ "subject": self.subject, "serial": self.serial });
        if let (serde_json::Value::Object(d), serde_json::Value::Object(extra)) = (&mut d, detail) {
            d.extend(extra);
        }
        AuditEvent::new(&self.actor, op)
            .ip(self.peer.ip().to_string())
            .detail(d)
            .failure()
    }
}

impl CertStatus {
    /// For the control plane's TLS settings (with their CRLs) and
    /// `OT_TLS_CLIENT_OCSP_URL`.
    pub fn new(
        tls: &ot_source::tls::ServerTls,
        url: Option<String>,
        audit: Option<AuditSink>,
    ) -> anyhow::Result<Self> {
        let url = url.map(|u| u.trim().to_owned()).filter(|u| !u.is_empty());
        if let Some(u) = &url {
            anyhow::ensure!(
                http_url(u),
                "OT_TLS_CLIENT_OCSP_URL {u:?} is not an http:// or https:// URL"
            );
        }
        let provider = rustls::crypto::CryptoProvider::get_default()
            .cloned()
            .unwrap_or_else(|| Arc::new(rustls::crypto::default_fips_provider()));
        Ok(Self {
            cas: tls.client_ca_certs()?,
            url,
            crl: RwLock::new(tls.client_crl_verifier()?),
            cache: Mutex::default(),
            flights: Mutex::default(),
            down: Mutex::default(),
            http: http_client(),
            peers: Default::default(),
            algs: provider.signature_verification_algorithms.all,
            audit,
        })
    }

    /// Use these revocation lists from now on.
    pub fn set_crls(&self, v: Option<Arc<dyn ClientCertVerifier>>) {
        match self.crl.write() {
            Ok(mut g) => *g = v,
            Err(e) => *e.into_inner() = v,
        }
    }

    /// Whether the presented chain's certificate may be used; the reason
    /// if not (already logged and audited).
    pub async fn check(
        &self,
        chain: &[CertificateDer<'_>],
        peer: SocketAddr,
    ) -> Result<(), String> {
        let Some(leaf_der) = chain.first() else {
            return Ok(());
        };
        let parsed = ocsp::Cert::parse(leaf_der);
        let who = Who {
            peer,
            actor: format!(
                "cert:{}",
                ot_source::tls::common_name(leaf_der).unwrap_or_default()
            ),
            subject: ot_source::tls::subject(leaf_der).unwrap_or_default(),
            serial: parsed.as_ref().map(|c| hex(c.serial)).unwrap_or_default(),
        };
        let Ok(leaf) = parsed else {
            return self.by_crl(chain, &who, None, None);
        };
        let issuer = self.issuer(&leaf, &chain[1..]);
        let url = self.url.clone().or_else(|| {
            leaf.ocsp_urls
                .iter()
                .find(|u| http_url(u))
                .map(|u| (*u).to_owned())
        });
        let (Some(issuer), Some(url)) = (issuer, url) else {
            return self.by_crl(chain, &who, None, None);
        };
        let Ok(issuer_cert) = ocsp::Cert::parse(issuer) else {
            return self.by_crl(chain, &who, None, None);
        };
        let id = ocsp::CertId::new(&issuer_cert, &leaf);
        let key = id.encode();
        if let Some(revoked) = self.cached(&key) {
            return self.decided(revoked, &who, &url);
        }
        let flight = {
            let mut f = self.flights.lock().unwrap_or_else(|e| e.into_inner());
            f.entry(key.clone()).or_default().clone()
        };
        let result = {
            let _one_at_a_time = flight.lock().await;
            self.ask_or_fall_back(chain, &who, &url, &id, issuer, &key)
                .await
        };
        let mut f = self.flights.lock().unwrap_or_else(|e| e.into_inner());
        // Ours and the map's: nobody else waits on it.
        if Arc::strong_count(&flight) <= 2 {
            f.remove(&key);
        }
        result
    }

    async fn ask_or_fall_back(
        &self,
        chain: &[CertificateDer<'_>],
        who: &Who,
        url: &str,
        id: &ocsp::CertId,
        issuer: &[u8],
        key: &[u8],
    ) -> Result<(), String> {
        // Another connection may have asked while this one waited.
        if let Some(revoked) = self.cached(key) {
            return self.decided(revoked, who, url);
        }
        if self.is_down(url) {
            return self.by_crl(chain, who, Some(url), Some("responder down (not asked)"));
        }
        match self.ask(url, id, issuer).await {
            Ok(a) if a.status == ocsp::Status::Unknown => {
                // Not a revocation: the responder just doesn't vouch for
                // the certificate. The lists may; without them it's refused.
                self.by_crl(chain, who, Some(url), Some("unknown"))
            }
            Ok(a) => {
                let revoked = a.status == ocsp::Status::Revoked;
                self.remember(key, revoked, &a);
                self.decided(revoked, who, url)
            }
            Err(NoAnswer::Down(e)) => {
                self.mark_down(url);
                self.by_crl(chain, who, Some(url), Some(&format!("unreachable: {e}")))
            }
            Err(NoAnswer::Unusable(e)) => {
                self.by_crl(chain, who, Some(url), Some(&format!("unusable: {e}")))
            }
        }
    }

    /// The certificate's issuer, among the presented chain and the client
    /// CAs: the one named as its issuer whose key signed it.
    fn issuer<'x>(
        &'x self,
        leaf: &ocsp::Cert<'_>,
        presented: &'x [CertificateDer<'_>],
    ) -> Option<&'x [u8]> {
        presented
            .iter()
            .map(|c| c.as_ref())
            .chain(self.cas.iter().map(|c| c.as_ref()))
            .find(|c| {
                ocsp::Cert::parse(c).is_ok_and(|i| i.subject == leaf.issuer)
                    && ocsp::signed_by(self.algs, c, leaf.sig_alg, leaf.tbs, leaf.signature)
            })
    }

    /// Query the responder and check its answer.
    async fn ask(
        &self,
        url: &str,
        id: &ocsp::CertId,
        issuer: &[u8],
    ) -> Result<ocsp::Answer, NoAnswer> {
        let nonce = crate::fips::random_bytes::<16>();
        let body = self.fetch(url, ocsp::request(id, &nonce)).await?;
        ocsp::check_response(
            &body,
            &ocsp::Expect {
                id,
                issuer,
                nonce: &nonce,
                algs: self.algs,
                now: UnixTime::now(),
            },
        )
        .map_err(|e| NoAnswer::Unusable(e.to_owned()))
    }

    async fn fetch(&self, url: &str, request: Vec<u8>) -> Result<Vec<u8>, NoAnswer> {
        let down = |e: reqwest::Error| NoAnswer::Down(e.without_url().to_string());
        let mut resp = self
            .http
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/ocsp-request")
            .header(reqwest::header::ACCEPT, "application/ocsp-response")
            .body(request)
            .send()
            .await
            .map_err(down)?;
        if let Some(peer) = resp.remote_addr()
            && self.peers.changed(url, peer)
        {
            tracing::info!(component = "ocsp", responder = %ot_core::secrets::redact_url(url), peer_addr = %peer, "OCSP responder reached");
        }
        if !resp.status().is_success() {
            return Err(NoAnswer::Down(format!("HTTP {}", resp.status().as_u16())));
        }
        let too_big = || NoAnswer::Unusable(format!("the answer is over {MAX_ANSWER} bytes"));
        if resp.content_length().is_some_and(|l| l > MAX_ANSWER as u64) {
            return Err(too_big());
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(down)? {
            if body.len() + chunk.len() > MAX_ANSWER {
                return Err(too_big());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    /// An OCSP answer (fresh or cached): good is let in, revoked refused.
    fn decided(&self, revoked: bool, who: &Who, url: &str) -> Result<(), String> {
        if !revoked {
            return Ok(());
        }
        let reason = "revoked (OCSP)".to_owned();
        tracing::warn!(peer = %who.peer, subject = %who.subject, serial = %who.serial, responder = url, "client certificate revoked (OCSP)");
        self.refused(who, &reason, "ocsp", Some(url));
        Err(reason)
    }

    /// No OCSP answer to go by (`outcome` says why, if OCSP was tried):
    /// the revocation lists decide, and without them the certificate is
    /// refused.
    fn by_crl(
        &self,
        chain: &[CertificateDer<'_>],
        who: &Who,
        url: Option<&str>,
        outcome: Option<&str>,
    ) -> Result<(), String> {
        let verifier = self
            .crl
            .read()
            .map_or_else(|e| e.into_inner().clone(), |v| v.clone());
        let result = match verifier {
            None => Err(("none", "no OCSP answer and no revocation list".to_owned())),
            Some(v) => v
                .verify_client_cert(&chain[0], &chain[1..], UnixTime::now())
                .map(|_| ())
                .map_err(|e| ("crl", format!("revocation list: {e}"))),
        };
        if let Some(outcome) = outcome {
            let fallback = match &result {
                Ok(()) => "accepted by the revocation list",
                Err(("none", _)) => "refused: no revocation list",
                Err(_) => "refused by the revocation list",
            };
            tracing::warn!(peer = %who.peer, subject = %who.subject, serial = %who.serial, responder = url, outcome, fallback, "no usable OCSP answer; the revocation lists decide");
            self.emit(who.event(
                "ocsp_unavailable",
                json!({ "outcome": outcome, "responder": url, "fallback": fallback }),
            ));
        }
        match result {
            Ok(()) => Ok(()),
            Err((checked, reason)) => {
                self.refused(who, &reason, checked, url);
                Err(reason)
            }
        }
    }

    fn refused(&self, who: &Who, reason: &str, checked: &str, url: Option<&str>) {
        self.emit(who.event(
            "certificate_refused",
            json!({ "reason": reason, "checked": checked, "responder": url }),
        ));
    }

    fn emit(&self, e: AuditEvent) {
        if let Some(sink) = &self.audit {
            sink(e);
        }
    }

    fn cached(&self, key: &[u8]) -> Option<bool> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match cache.get(key) {
            Some(c) if c.until > SystemTime::now() => Some(c.revoked),
            Some(_) => {
                cache.remove(key);
                None
            }
            None => None,
        }
    }

    fn remember(&self, key: &[u8], revoked: bool, a: &ocsp::Answer) {
        let now = SystemTime::now();
        let keep = match a.next_update {
            Some(next) => {
                let next = UNIX_EPOCH + Duration::from_secs(u64::try_from(next).unwrap_or(0));
                next.duration_since(now)
                    .unwrap_or_default()
                    .min(KEEP_AT_MOST)
            }
            None => KEEP_WITHOUT_NEXT_UPDATE,
        };
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= MAX_CACHED {
            cache.retain(|_, c| c.until > now);
        }
        while cache.len() >= MAX_CACHED {
            let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, c)| c.at)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            cache.remove(&oldest);
        }
        cache.insert(
            key.to_vec(),
            Cached {
                revoked,
                until: now + keep,
                at: Instant::now(),
            },
        );
    }

    fn is_down(&self, url: &str) -> bool {
        let mut down = self.down.lock().unwrap_or_else(|e| e.into_inner());
        down.retain(|_, at| at.elapsed() < DOWN_FOR);
        down.contains_key(url)
    }

    fn mark_down(&self, url: &str) {
        let mut down = self.down.lock().unwrap_or_else(|e| e.into_inner());
        down.insert(url.to_owned(), Instant::now());
    }
}

/// The control plane's TLS settings for the handshake: the same, without
/// the revocation lists, which [`CertStatus`] consults instead.
pub fn handshake_tls(tls: &ot_source::tls::ServerTls) -> ot_source::tls::ServerTls {
    ot_source::tls::ServerTls {
        client_crl_files: Vec::new(),
        ..tls.clone()
    }
}

fn http_url(u: &str) -> bool {
    reqwest::Url::parse(u).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
}

/// The client OCSP queries go through: 5 s, no redirects, this build's name.
fn http_client() -> reqwest::Client {
    let builder = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("opentrack/", env!("CARGO_PKG_VERSION")));
    // https:// responders: the FIPS provider with the system roots, as feed
    // transports use. Without system roots (a minimal image), reqwest's own
    // rustls setup, which also uses the process's installed provider
    // (FIPS); most responders are plain http:// anyway.
    let builder = match ot_source::tls::ClientTls::default().client_config() {
        Ok(c) => builder.tls_backend_preconfigured(c),
        Err(_) => builder,
    };
    builder.build().unwrap_or_default()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests;
