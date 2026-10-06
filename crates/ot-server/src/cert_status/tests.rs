//! A test CA, client certificates naming an in-process OCSP responder whose
//! answers each test writes, and revocation lists.

use std::sync::atomic::{AtomicUsize, Ordering};

use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, CustomExtension, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, SigningKey,
};

use super::*;
use crate::ocsp::{OCTET_STRING, OID, SEQUENCE, tlv};

/// ecdsa-with-SHA256, what rcgen's default keys sign with.
const ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const AD_OCSP: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01];
const OCSP_BASIC: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01, 0x01];
const OCSP_NONCE: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01, 0x02];
const GOOD: &[u8] = &[0x80, 0x00];
const UNKNOWN: &[u8] = &[0x82, 0x00];

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn gtime(t: i64) -> Vec<u8> {
    let s = chrono::DateTime::from_timestamp(t, 0)
        .unwrap()
        .format("%Y%m%d%H%M%SZ")
        .to_string();
    tlv(0x18, &[s.as_bytes()])
}

fn revoked() -> Vec<u8> {
    tlv(0xa1, &[&gtime(now() - 86_400)])
}

fn copy(key: &KeyPair) -> KeyPair {
    KeyPair::try_from(key.serialize_der()).unwrap()
}

struct Pki {
    dir: tempfile::TempDir,
    ca: CertifiedIssuer<'static, KeyPair>,
}

impl Pki {
    fn new() -> Self {
        let _ = crate::fips::init();
        let dir = tempfile::tempdir().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(DnType::CommonName, "Test CA");
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
        std::fs::write(dir.path().join("ca.pem"), ca.pem()).unwrap();
        let pki = Self { dir, ca };
        // Lists revoking someone else, and revoking serial 0xbad.
        pki.crl("crl-ok.pem", 0x999);
        pki.crl("crl-revoked.pem", 0xbad);
        pki
    }

    fn path(&self, name: &str) -> String {
        self.dir.path().join(name).display().to_string()
    }

    fn crl(&self, name: &str, revoked: u64) {
        use rcgen::{CertificateRevocationListParams, KeyIdMethod, RevokedCertParams};
        let list = CertificateRevocationListParams {
            this_update: rcgen::date_time_ymd(2020, 1, 1),
            next_update: rcgen::date_time_ymd(2099, 1, 1),
            crl_number: 1u64.into(),
            issuing_distribution_point: None,
            revoked_certs: vec![RevokedCertParams {
                serial_number: revoked.into(),
                revocation_time: rcgen::date_time_ymd(2020, 1, 1),
                reason_code: None,
                invalidity_date: None,
            }],
            key_identifier_method: KeyIdMethod::Sha256,
        }
        .signed_by(&self.ca)
        .unwrap();
        std::fs::write(self.path(name), list.pem().unwrap()).unwrap();
    }

    /// A certificate from the CA, with an AIA OCSP URL if given.
    fn issue(
        &self,
        cn: &str,
        serial: u64,
        aia: Option<&str>,
        eku: ExtendedKeyUsagePurpose,
    ) -> (CertificateDer<'static>, KeyPair) {
        let mut params = CertificateParams::new(vec!["localhost".to_owned()]).unwrap();
        params.serial_number = Some(serial.into());
        params.distinguished_name.push(DnType::CommonName, cn);
        params.extended_key_usages = vec![eku];
        if let Some(url) = aia {
            let ad = tlv(
                SEQUENCE,
                &[&tlv(OID, &[AD_OCSP]), &tlv(0x86, &[url.as_bytes()])],
            );
            params
                .custom_extensions
                .push(CustomExtension::from_oid_content(
                    &[1, 3, 6, 1, 5, 5, 7, 1, 1],
                    tlv(SEQUENCE, &[&ad]),
                ));
        }
        let key = KeyPair::generate().unwrap();
        let cert = params.signed_by(&key, &self.ca).unwrap();
        (cert.der().clone(), key)
    }

    fn client(&self, serial: u64, aia: Option<&str>) -> CertificateDer<'static> {
        self.issue("alice", serial, aia, ExtendedKeyUsagePurpose::ClientAuth)
            .0
    }

    fn id(&self, leaf: &[u8]) -> ocsp::CertId {
        ocsp::CertId::new(
            &ocsp::Cert::parse(self.ca.der()).unwrap(),
            &ocsp::Cert::parse(leaf).unwrap(),
        )
    }

    fn tls(&self, crl: Option<&str>) -> ot_source::tls::ServerTls {
        ot_source::tls::ServerTls {
            cert_file: self.path("server.pem"),
            key_file: self.path("server.key"),
            client_ca_file: Some(self.path("ca.pem")),
            client_cert_optional: true,
            client_crl_files: crl.map(|c| vec![self.path(c)]).unwrap_or_default(),
        }
    }
}

/// What a test responder answers.
struct Reply {
    signer: KeyPair,
    certs: Vec<Vec<u8>>,
    id: Vec<u8>,
    status: Vec<u8>,
    this_update: i64,
    next_update: Option<i64>,
    /// `None`: echo the request's; `Some(v)`: send `v`.
    nonce: Option<Vec<u8>>,
}

impl Reply {
    fn new(pki: &Pki, leaf: &[u8], status: &[u8]) -> Reply {
        Reply {
            signer: copy(pki.ca.key()),
            certs: vec![],
            id: pki.id(leaf).encode(),
            status: status.to_vec(),
            this_update: now() - 60,
            next_update: Some(now() + 600),
            nonce: None,
        }
    }

    fn encode(&self, request: &[u8]) -> Vec<u8> {
        let mut single = vec![
            self.id.clone(),
            self.status.clone(),
            gtime(self.this_update),
        ];
        if let Some(n) = self.next_update {
            single.push(tlv(0xa0, &[&gtime(n)]));
        }
        let single: Vec<&[u8]> = single.iter().map(Vec::as_slice).collect();
        let responses = tlv(SEQUENCE, &[&tlv(SEQUENCE, &single)]);
        let nonce = self
            .nonce
            .clone()
            .or_else(|| ocsp::request_nonce(request))
            .unwrap_or_default();
        let ext = tlv(
            SEQUENCE,
            &[
                &tlv(OID, &[OCSP_NONCE]),
                &tlv(OCTET_STRING, &[&tlv(OCTET_STRING, &[&nonce])]),
            ],
        );
        let data = tlv(
            SEQUENCE,
            &[
                // responderID byKey (not used to pick the signer).
                &tlv(0xa2, &[&tlv(OCTET_STRING, &[&[7; 20]])]),
                &gtime(now()),
                &responses,
                &tlv(0xa1, &[&tlv(SEQUENCE, &[&ext])]),
            ],
        );
        let sig = self.signer.sign(&data).unwrap();
        let alg = tlv(SEQUENCE, &[&tlv(OID, &[ECDSA_SHA256])]);
        let mut basic = vec![data, alg, tlv(0x03, &[&[0], &sig])];
        if !self.certs.is_empty() {
            let certs: Vec<&[u8]> = self.certs.iter().map(Vec::as_slice).collect();
            basic.push(tlv(0xa0, &[&tlv(SEQUENCE, &certs)]));
        }
        let basic: Vec<&[u8]> = basic.iter().map(Vec::as_slice).collect();
        let bytes = tlv(
            SEQUENCE,
            &[
                &tlv(OID, &[OCSP_BASIC]),
                &tlv(OCTET_STRING, &[&tlv(SEQUENCE, &basic)]),
            ],
        );
        tlv(SEQUENCE, &[&[0x0a, 0x01, 0x00], &tlv(0xa0, &[&bytes])])
    }
}

type AnswerFn = Box<dyn Fn(&[u8]) -> (u16, Vec<u8>) + Send + Sync>;

/// An OCSP responder on 127.0.0.1 answering what the test says, counting
/// the queries.
struct Responder {
    url: String,
    hits: Arc<AtomicUsize>,
    answer: Arc<Mutex<AnswerFn>>,
}

impl Responder {
    async fn start() -> Self {
        let hits = Arc::new(AtomicUsize::new(0));
        let answer: Arc<Mutex<AnswerFn>> =
            Arc::new(Mutex::new(Box::new(|_: &[u8]| (500, Vec::new()))));
        let (h, a) = (hits.clone(), answer.clone());
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move |body: bytes::Bytes| {
                let (h, a) = (h.clone(), a.clone());
                async move {
                    h.fetch_add(1, Ordering::SeqCst);
                    // Slow enough that parallel checks overlap.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let (code, body) = (a.lock().unwrap())(&body);
                    (axum::http::StatusCode::from_u16(code).unwrap(), body)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        Self { url, hits, answer }
    }

    fn answer(&self, f: impl Fn(&[u8]) -> (u16, Vec<u8>) + Send + Sync + 'static) {
        *self.answer.lock().unwrap() = Box::new(f);
    }

    fn reply(&self, r: Reply) {
        self.answer(move |req| (200, r.encode(req)));
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

type Events = Arc<Mutex<Vec<AuditEvent>>>;

fn checker(pki: &Pki, crl: Option<&str>, url: Option<&str>) -> (CertStatus, Events) {
    let events: Events = Arc::default();
    let e = events.clone();
    let sink: AuditSink = Arc::new(move |ev| e.lock().unwrap().push(ev));
    let s = CertStatus::new(&pki.tls(crl), url.map(str::to_owned), Some(sink)).unwrap();
    (s, events)
}

fn peer() -> SocketAddr {
    "192.0.2.7:5000".parse().unwrap()
}

fn ops(events: &Events) -> Vec<(String, serde_json::Value)> {
    events
        .lock()
        .unwrap()
        .iter()
        .map(|e| (e.op.clone(), e.detail.clone()))
        .collect()
}

#[tokio::test]
async fn a_good_answer_is_accepted_and_cached() {
    let (pki, r) = (Pki::new(), Responder::start().await);
    let leaf = pki.client(0xa1, Some(&r.url));
    r.reply(Reply::new(&pki, &leaf, GOOD));
    let (s, events) = checker(&pki, None, None);
    assert_eq!(s.check(std::slice::from_ref(&leaf), peer()).await, Ok(()));
    assert_eq!(s.check(&[leaf], peer()).await, Ok(()));
    assert_eq!(r.hits(), 1, "the second check used the cached answer");
    assert!(ops(&events).is_empty());
}

#[tokio::test]
async fn parallel_connections_make_one_query() {
    let (pki, r) = (Pki::new(), Responder::start().await);
    let leaf = pki.client(0xa1, Some(&r.url));
    r.reply(Reply::new(&pki, &leaf, GOOD));
    let s = Arc::new(checker(&pki, None, None).0);
    let checks: Vec<_> = (0..6)
        .map(|_| {
            let (s, leaf) = (s.clone(), leaf.clone());
            tokio::spawn(async move { s.check(&[leaf], peer()).await })
        })
        .collect();
    for c in checks {
        assert_eq!(c.await.unwrap(), Ok(()));
    }
    assert_eq!(r.hits(), 1);
}

#[tokio::test]
async fn a_revoked_answer_is_refused_audited_and_cached() {
    let (pki, r) = (Pki::new(), Responder::start().await);
    let leaf = pki.client(0xa1, Some(&r.url));
    r.reply(Reply::new(&pki, &leaf, &revoked()));
    // A list saying it's fine doesn't matter: OCSP answered.
    let (s, events) = checker(&pki, Some("crl-ok.pem"), None);
    assert!(s.check(std::slice::from_ref(&leaf), peer()).await.is_err());
    assert!(s.check(&[leaf], peer()).await.is_err());
    assert_eq!(r.hits(), 1);
    let ops = ops(&events);
    assert_eq!(ops.len(), 2);
    let (op, d) = &ops[0];
    assert_eq!(op, "certificate_refused");
    assert_eq!(d["checked"], "ocsp");
    assert_eq!(d["serial"], "00a1");
    assert_eq!(d["responder"], r.url.as_str());
    assert!(d["subject"].as_str().unwrap().contains("CN=alice"));
    let e = &events.lock().unwrap()[0];
    assert_eq!((e.actor.as_str(), e.success), ("cert:alice", false));
    assert_eq!(e.ip.as_deref(), Some("192.0.2.7"));
}

#[tokio::test]
async fn unknown_falls_back_to_the_lists() {
    let (pki, r) = (Pki::new(), Responder::start().await);
    let leaf = pki.client(0xa1, Some(&r.url));
    r.reply(Reply::new(&pki, &leaf, UNKNOWN));
    let (s, events) = checker(&pki, Some("crl-ok.pem"), None);
    assert_eq!(s.check(std::slice::from_ref(&leaf), peer()).await, Ok(()));
    let got = ops(&events);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, "ocsp_unavailable");
    assert_eq!(got[0].1["outcome"], "unknown");
    assert_eq!(got[0].1["fallback"], "accepted by the revocation list");

    // Without lists, unknown is refused.
    let (s, events) = checker(&pki, None, None);
    assert!(s.check(&[leaf], peer()).await.is_err());
    let got = ops(&events);
    assert_eq!(got[1].0, "certificate_refused");
    assert_eq!(got[1].1["checked"], "none");
}

#[tokio::test]
async fn a_responder_that_is_down_falls_back_to_the_lists() {
    let (pki, r) = (Pki::new(), Responder::start().await);
    r.answer(|_| (503, Vec::new()));
    let good = pki.client(0xa1, Some(&r.url));
    let bad = pki.client(0xbad, Some(&r.url));
    let (s, events) = checker(&pki, Some("crl-revoked.pem"), None);
    assert_eq!(s.check(&[good], peer()).await, Ok(()));
    assert_eq!(r.hits(), 1);
    // Marked down: the next certificate goes straight to the lists, which
    // revoke it.
    let refused = s.check(&[bad], peer()).await.unwrap_err();
    assert!(refused.contains("revocation list"), "{refused}");
    assert_eq!(r.hits(), 1, "a responder that is down isn't asked again");
    let got = ops(&events);
    assert_eq!(got[0].0, "ocsp_unavailable");
    assert!(got[0].1["outcome"].as_str().unwrap().contains("HTTP 503"));
    assert_eq!(got[1].1["outcome"], "responder down (not asked)");
    assert_eq!(got[2].0, "certificate_refused");
    assert_eq!(got[2].1["checked"], "crl");

    // Nobody listening at all.
    let leaf = pki.client(0xa1, Some("http://127.0.0.1:1/"));
    let (s, events) = checker(&pki, Some("crl-ok.pem"), None);
    assert_eq!(s.check(&[leaf], peer()).await, Ok(()));
    let outcome = ops(&events)[0].1["outcome"].clone();
    assert!(
        outcome.as_str().unwrap().starts_with("unreachable"),
        "{outcome}"
    );
}

#[tokio::test]
async fn without_ocsp_or_lists_a_certificate_is_refused() {
    let pki = Pki::new();
    let leaf = pki.client(0xa1, None);
    let (s, events) = checker(&pki, None, None);
    assert_eq!(
        s.check(std::slice::from_ref(&leaf), peer()).await,
        Err("no OCSP answer and no revocation list".to_owned())
    );
    assert_eq!(ops(&events)[0].1["checked"], "none");
    // With lists, a certificate without a responder is theirs to decide,
    // quietly (not an OCSP failure).
    let (s, events) = checker(&pki, Some("crl-ok.pem"), None);
    assert_eq!(s.check(&[leaf], peer()).await, Ok(()));
    assert!(ops(&events).is_empty());
}

#[tokio::test]
async fn a_delegated_responder_needs_ocsp_signing() {
    let (pki, r) = (Pki::new(), Responder::start().await);
    let leaf = pki.client(0xa1, Some(&r.url));
    for (eku, ok) in [
        (ExtendedKeyUsagePurpose::OcspSigning, true),
        (ExtendedKeyUsagePurpose::ClientAuth, false),
    ] {
        let (cert, key) = pki.issue("responder", 0x0c5, None, eku);
        let mut reply = Reply::new(&pki, &leaf, GOOD);
        reply.signer = key;
        reply.certs = vec![cert.to_vec()];
        r.reply(reply);
        let (s, events) = checker(&pki, None, None);
        assert_eq!(
            s.check(std::slice::from_ref(&leaf), peer()).await.is_ok(),
            ok
        );
        if !ok {
            let o = ops(&events)[0].1["outcome"].as_str().unwrap().to_owned();
            assert!(o.contains("signature"), "{o}");
        }
    }
}

#[tokio::test]
async fn unusable_answers_fall_back_to_the_lists() {
    let (pki, r) = (Pki::new(), Responder::start().await);
    let leaf = pki.client(0xa1, Some(&r.url));
    let other = pki.client(0xa2, None);
    type Spoil = fn(&mut Reply, &Pki, &[u8]);
    let cases: [(&str, Spoil); 6] = [
        // A key that is neither the CA's nor a delegated responder's.
        ("signature", |r, _, _| {
            r.signer = KeyPair::generate().unwrap()
        }),
        ("CertID mismatch", |r, pki, other| {
            r.id = pki.id(other).encode()
        }),
        ("nextUpdate", |r, _, _| {
            r.this_update = now() - 7200;
            r.next_update = Some(now() - 3600);
        }),
        ("over an hour old", |r, _, _| {
            r.this_update = now() - 7200;
            r.next_update = None;
        }),
        ("in the future", |r, _, _| r.this_update = now() + 3600),
        ("nonce", |r, _, _| r.nonce = Some(vec![1; 16])),
    ];
    for (want, spoil) in cases {
        let mut reply = Reply::new(&pki, &leaf, GOOD);
        spoil(&mut reply, &pki, &other);
        r.reply(reply);
        // Accepted by the list, refused without one; never on OCSP's word.
        let (s, events) = checker(&pki, Some("crl-ok.pem"), None);
        assert_eq!(
            s.check(std::slice::from_ref(&leaf), peer()).await,
            Ok(()),
            "{want}"
        );
        let o = ops(&events)[0].1["outcome"].as_str().unwrap().to_owned();
        assert!(o.starts_with("unusable") && o.contains(want), "{want}: {o}");
        let (s, _) = checker(&pki, None, None);
        assert!(
            s.check(std::slice::from_ref(&leaf), peer()).await.is_err(),
            "{want}"
        );
    }
    // A responder's error status is no answer either.
    r.answer(|_| (200, vec![0x30, 0x03, 0x0a, 0x01, 0x03]));
    let (s, events) = checker(&pki, None, None);
    assert!(s.check(&[leaf], peer()).await.is_err());
    let o = ops(&events)[0].1["outcome"].as_str().unwrap().to_owned();
    assert!(o.contains("tryLater"), "{o}");
}

#[tokio::test]
async fn the_configured_responder_overrides_the_certificates() {
    let (pki, r) = (Pki::new(), Responder::start().await);
    let leaf = pki.client(0xa1, Some("http://127.0.0.1:1/"));
    r.reply(Reply::new(&pki, &leaf, GOOD));
    let (s, events) = checker(&pki, None, Some(&r.url));
    assert_eq!(s.check(&[leaf], peer()).await, Ok(()));
    assert_eq!(r.hits(), 1);
    assert!(ops(&events).is_empty());
    assert!(CertStatus::new(&pki.tls(None), Some("ldap://x".into()), None).is_err());
}

#[tokio::test]
async fn the_handshake_leaves_revocation_to_the_status_check() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let pki = Pki::new();
    // The server's own certificate, from the same CA.
    let (server, key) = pki.issue("localhost", 0x5e, None, ExtendedKeyUsagePurpose::ServerAuth);
    let pem = {
        use base64::Engine;
        let b = base64::engine::general_purpose::STANDARD.encode(&server);
        format!("-----BEGIN CERTIFICATE-----\n{b}\n-----END CERTIFICATE-----\n")
    };
    std::fs::write(pki.path("server.pem"), pem).unwrap();
    std::fs::write(pki.path("server.key"), key.serialize_pem()).unwrap();

    let tls = pki.tls(Some("crl-revoked.pem"));
    let handshake = handshake_tls(&tls);
    assert!(handshake.client_crl_files.is_empty());
    assert!(tls.client_crl_verifier().unwrap().is_some());
    let status = Arc::new(CertStatus::new(&tls, None, None).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route("/", axum::routing::get(|| async { "hello" }));
    let acceptor = crate::https::Acceptor::new(handshake.acceptor().unwrap());
    tokio::spawn(crate::https::serve(
        listener,
        app,
        acceptor,
        Some(status),
        std::future::pending(),
    ));

    let get = |serial: u64| {
        let (cert, key) = pki.issue("alice", serial, None, ExtendedKeyUsagePurpose::ClientAuth);
        let mut roots = rustls::RootCertStore::empty();
        roots.add(pki.ca.der().clone()).unwrap();
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_client_auth_cert(
                vec![cert],
                rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der()).unwrap(),
            )
            .unwrap();
        async move {
            let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
            let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
            // The handshake completes either way: no lists in it.
            let mut tls = tokio_rustls::TlsConnector::from(Arc::new(config))
                .connect(name, tcp)
                .await
                .unwrap();
            let _ = tls
                .write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
                .await;
            let mut out = Vec::new();
            let _ = tls.read_to_end(&mut out).await;
            String::from_utf8_lossy(&out).into_owned()
        }
    };
    assert!(get(0xa1).await.contains("hello"));
    assert!(!get(0xbad).await.contains("hello"), "revoked by the list");
}
