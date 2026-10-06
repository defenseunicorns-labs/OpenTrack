//! SAML 2.0 single sign-on, ported from OpenStare's: OpenTrack is the
//! service provider. `/auth/saml/login` sends the browser to the identity
//! provider with a signed-request id in the relay state; the provider
//! posts its signed assertion to `/auth/saml/acs`, which checks it (the
//! signature, the conditions, that it answers our request, that it was not
//! used before), finds or makes the account, and starts a session.
//!
//! Only the HTTP-Redirect binding for requests and the POST binding for
//! responses; no identity-provider-initiated sign-on.

use std::collections::HashMap;
use std::io::Write;
use std::str::FromStr;
use std::sync::Mutex;

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Form, State as AxumState};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use base64::Engine;
use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use samael::metadata::EntityDescriptor;
use samael::schema::Assertion;
use samael::service_provider::{ServiceProvider, ServiceProviderBuilder};
use samael::traits::ToXml;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::api::{Client, How};
use super::{AuthSettings, Role, settings::SamlSettings};
use crate::control::{ApiError, AppState};

const HTTP_REDIRECT: &str = "urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect";
/// How long a sign-on may take between our request and the answer.
const RELAY_MINUTES: i64 = 5;

/// Assertions already used, until they expire (a replayed one is refused).
#[derive(Default)]
pub struct State {
    seen: Mutex<HashMap<String, DateTime<Utc>>>,
}

impl State {
    fn first_use(&self, id: &str, until: DateTime<Utc>) -> bool {
        let Ok(mut m) = self.seen.lock() else {
            return false;
        };
        let now = Utc::now();
        m.retain(|_, t| *t > now);
        if m.contains_key(id) {
            return false;
        }
        m.insert(id.to_owned(), until);
        true
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/saml/login", get(login))
        .route("/auth/saml/acs", post(acs))
        .route("/auth/saml/metadata", get(metadata))
        .route("/auth/saml/parse-metadata", post(parse_metadata))
}

fn base_url(s: &AppState) -> Result<String, ApiError> {
    s.auth.public_url.clone().ok_or_else(|| {
        ApiError::bad_request(
            "set OT_PUBLIC_URL (where browsers reach OpenTrack) for single sign-on",
        )
    })
}

fn service_provider(cfg: &SamlSettings, base: &str) -> Result<ServiceProvider, String> {
    let idp = EntityDescriptor::from_str(&cfg.idp_metadata_xml)
        .map_err(|e| format!("the identity provider's metadata: {e}"))?;
    ServiceProviderBuilder::default()
        .entity_id(format!("{base}/api/v1/auth/saml/metadata"))
        .acs_url(format!("{base}/api/v1/auth/saml/acs"))
        .idp_metadata(idp)
        .allow_idp_initiated(false)
        .build()
        .map_err(|e| format!("building the service provider: {e}"))
}

#[derive(Serialize, Deserialize)]
struct Relay {
    rid: String,
    exp: i64,
}

fn mint_relay(rid: &str, secret: &[u8]) -> Result<String, String> {
    let claims = Relay {
        rid: rid.to_owned(),
        exp: (Utc::now() + chrono::Duration::minutes(RELAY_MINUTES)).timestamp(),
    };
    jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .map_err(|e| e.to_string())
}

/// The largest SAMLResponse taken (base64); real ones are a few KB.
const MAX_RESPONSE_B64: usize = 256 * 1024;

/// Refuse a SAMLResponse before any XML parser sees it: too large, not
/// base64, or carrying a DTD (`<!DOCTYPE`, `<!ENTITY`), which no IdP sends
/// and which entity and parser attacks need (ASD V-222608).
fn check_response(b64_response: &str) -> Result<(), String> {
    use base64::Engine;
    if b64_response.len() > MAX_RESPONSE_B64 {
        return Err("the SAMLResponse is too large".into());
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64_response.trim())
        .map_err(|_| "the SAMLResponse is not base64".to_owned())?;
    let upper = String::from_utf8_lossy(&raw).to_ascii_uppercase();
    if upper.contains("<!DOCTYPE") || upper.contains("<!ENTITY") {
        return Err("the SAMLResponse carries a DTD".into());
    }
    Ok(())
}

fn verify_relay(token: &str, secret: &[u8]) -> Option<String> {
    let mut v = Validation::new(Algorithm::HS256);
    v.validate_exp = true;
    jsonwebtoken::decode::<Relay>(token, &DecodingKey::from_secret(secret), &v)
        .ok()
        .map(|d| d.claims.rid)
}

fn deflate_base64(bytes: &[u8]) -> std::io::Result<String> {
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(bytes)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(e.finish()?))
}

/// The identity provider's sign-in page, with our request.
fn redirect_url(s: &AppState, cfg: &SamlSettings) -> Result<String, String> {
    let base = base_url(s).map_err(|e| e.message)?;
    let sp = service_provider(cfg, &base)?;
    let sso = sp
        .sso_binding_location(HTTP_REDIRECT)
        .ok_or("the identity provider has no HTTP-Redirect sign-on")?;
    let mut authn = sp
        .make_authentication_request(&sso)
        .map_err(|e| format!("the sign-on request: {e}"))?;
    // samael's id is 32 random bits from a non-FIPS generator; the response
    // must answer this id, so make it 128 bits from the FIPS module.
    authn.id = format!("id-{}", crate::fips::random_hex::<16>());
    let xml = authn.to_string().map_err(|e| e.to_string())?;
    let relay = mint_relay(&authn.id, &s.auth.secret)?;
    let request = deflate_base64(xml.as_bytes()).map_err(|e| e.to_string())?;
    url::Url::parse_with_params(
        &sso,
        &[("SAMLRequest", request.as_str()), ("RelayState", &relay)],
    )
    .map(|u| u.to_string())
    .map_err(|e| e.to_string())
}

/// FIPS: xmlsec checks signatures with OpenSSL, which must be using only
/// its FIPS provider (the image configures it; see docs/security/fips.md).
pub fn openssl_fips() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = samael::openssl_fips_enabled();
        if on {
            tracing::info!("OpenSSL (SAML signatures) is in FIPS mode");
        } else {
            tracing::warn!("{NOT_FIPS}: load its FIPS provider (docs/security/fips.md)");
        }
        on
    })
}

const NOT_FIPS: &str = "OpenSSL is not in FIPS mode, so SAML sign-in is off";

async fn login(AxumState(s): AxumState<AppState>) -> Response {
    let cfg = s.auth.settings().saml;
    if !cfg.enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !openssl_fips() {
        tracing::error!("{NOT_FIPS}");
        return (StatusCode::SERVICE_UNAVAILABLE, NOT_FIPS).into_response();
    }
    match redirect_url(&s, &cfg).and_then(|u| HeaderValue::from_str(&u).map_err(|e| e.to_string()))
    {
        Ok(loc) => (StatusCode::FOUND, [(header::LOCATION, loc)]).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "SAML sign-on could not start");
            (StatusCode::INTERNAL_SERVER_ERROR, e).into_response()
        }
    }
}

#[derive(Deserialize)]
struct AcsForm {
    #[serde(rename = "SAMLResponse")]
    response: Option<String>,
    #[serde(rename = "RelayState")]
    relay: Option<String>,
}

/// Back to the sign-in page with a message (which check failed stays in
/// the log, not the page).
fn refused(why: &str) -> Response {
    tracing::info!(reason = why, "SAML sign-on refused");
    (
        StatusCode::FOUND,
        [(
            header::LOCATION,
            HeaderValue::from_static("/login?sso=failed"),
        )],
    )
        .into_response()
}

fn name_id(a: &Assertion) -> Option<String> {
    a.subject
        .as_ref()?
        .name_id
        .as_ref()
        .map(|n| n.value.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn attribute(a: &Assertion, name: &str) -> Vec<String> {
    a.attribute_statements
        .iter()
        .flatten()
        .flat_map(|st| st.attributes.iter())
        .filter(|at| at.name.as_deref() == Some(name) || at.friendly_name.as_deref() == Some(name))
        .flat_map(|at| at.values.iter())
        .filter_map(|v| v.value.clone())
        .collect()
}

/// The account a verified assertion signs in: an existing one by email
/// (a SAML-made account takes the role the provider gives it now), or a
/// new one with the mapped role.
/// The account a sign-on is for, made on first use; `Err` gives why it is
/// refused. Only `saml` accounts sign on this way: an account made another
/// way (a local one, with a role an admin gave it) is never taken over by
/// an identity provider that asserts its email.
fn account(
    db: &mut ot_store::Db,
    email: &str,
    role: Option<Role>,
) -> Result<Result<ot_store::User, &'static str>, ot_store::StoreError> {
    if let Some(u) = db.user_by_email(email)? {
        if u.origin != "saml" {
            return Ok(Err("a local account has this email"));
        }
        if !u.active {
            return Ok(Err("the account is turned off"));
        }
        let Some(r) = role else {
            return Ok(Err("no role for this user"));
        };
        if r.as_str() == u.role {
            return Ok(Ok(u));
        }
        let after = db.update_user(&u.id, None, Some(r.as_str()), None)?;
        db.record(&ot_store::Decision {
            before: Some(json!(u)),
            after: Some(json!(after)),
            ..ot_store::Decision::new("saml", "update_user")
                .reason("the identity provider gave another role")
        })?;
        return Ok(Ok(after));
    }
    let Some(role) = role else {
        return Ok(Err("no role for this user"));
    };
    let id = crate::fips::uuid_v4();
    let u = db.create_user(&ot_store::NewUser {
        id: &id,
        email,
        name: "",
        role: role.as_str(),
        password_hash: None,
        origin: "saml",
    })?;
    db.record(&ot_store::Decision {
        after: Some(json!(u)),
        ..ot_store::Decision::new("saml", "create_user").reason("first single sign-on")
    })?;
    Ok(Ok(u))
}

async fn acs_checked(s: &AppState, f: AcsForm, client: &Client) -> Result<Response, String> {
    let cfg = s.auth.settings().saml;
    if !cfg.enabled {
        return Err("SAML is off".into());
    }
    if !openssl_fips() {
        return Err(NOT_FIPS.into());
    }
    let (Some(response), Some(relay)) = (f.response, f.relay) else {
        return Err("no SAMLResponse or RelayState".into());
    };
    let Some(rid) = verify_relay(&relay, &s.auth.secret) else {
        return Err("the relay state is not ours or has expired".into());
    };
    let Ok(base) = base_url(s) else {
        return Err("no OT_PUBLIC_URL".into());
    };
    check_response(&response)?;
    let c = cfg.clone();
    let parsed = tokio::task::spawn_blocking(move || {
        let sp = service_provider(&c, &base)?;
        sp.parse_base64_response(&response, Some(&[rid.as_str()]))
            .map_err(|e| format!("the assertion: {e}"))
    })
    .await;
    let assertion = match parsed {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => return Err(e),
        Err(e) => return Err(e.to_string()),
    };
    let until = assertion
        .conditions
        .as_ref()
        .and_then(|c| c.not_on_or_after)
        .unwrap_or_else(|| Utc::now() + chrono::Duration::minutes(10));
    if !s.auth.saml.first_use(&assertion.id, until) {
        return Err("the assertion was used before".into());
    }
    let Some(email) = name_id(&assertion) else {
        return Err("the assertion names no one".into());
    };
    let values = attribute(&assertion, &cfg.role_attribute);
    let role = AuthSettings::map_role(&cfg.role_mapping, &values, cfg.default_role).map(|r| {
        if r == Role::Admin && !cfg.allow_admin {
            Role::TrackManager
        } else {
            r
        }
    });
    let e = email.clone();
    let user = match s.with_db(move |db| account(db, &e, role)).await {
        Ok(Ok(u)) => u,
        Ok(Err(why)) => return Err(why.into()),
        Err(e) => return Err(e.message),
    };
    // The body (and its `admin_api`) is not sent: the browser is redirected.
    match super::api::start_session(s, user, client, How::Saml, true).await {
        Ok((mut headers, _)) => {
            headers.insert(header::LOCATION, HeaderValue::from_static("/"));
            Ok((StatusCode::FOUND, headers).into_response())
        }
        Err(e) => Err(e.message),
    }
}

/// The assertion consumer service: sign in, or back to the sign-in page
/// with the refusal in the audit record.
async fn acs(
    AxumState(s): AxumState<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Form(f): Form<AcsForm>,
) -> Response {
    let client = Client::new(peer.as_ref().map(|p| &p.0), &headers);
    match acs_checked(&s, f, &client).await {
        Ok(r) => r,
        Err(why) => {
            let e = ot_store::AuditEvent::new("saml", "login")
                .failure()
                .ip(&client.ip)
                .detail(json!({ "reason": why, "via": "saml" }));
            if let Err(e) = s.with_db(move |db| db.audit(&e)).await {
                tracing::error!(error = %e.message, "recording a refused sign-on failed");
            }
            refused(&why)
        }
    }
}

/// Our service-provider metadata, for the identity provider's admin.
async fn metadata(AxumState(s): AxumState<AppState>) -> Result<Response, ApiError> {
    let base = base_url(&s)?;
    let cfg = s.auth.settings().saml;
    let xml = service_provider(&cfg, &base)
        .and_then(|sp| {
            sp.metadata()
                .map_err(|e| e.to_string())?
                .to_string()
                .map_err(|e| e.to_string())
        })
        .map_err(ApiError::unprocessable)?;
    let mut h = HeaderMap::new();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/samlmetadata+xml"),
    );
    Ok((h, xml).into_response())
}

#[derive(Deserialize)]
struct Metadata {
    xml: String,
}

/// What an identity provider's metadata says: its entity id, its
/// HTTP-Redirect sign-in URL and its signing certificate (the settings show
/// these; sign-on itself works from the metadata).
pub(super) fn idp_fields(xml: &str) -> Result<(String, String, String), String> {
    let ed = EntityDescriptor::from_str(xml).map_err(|e| format!("not SAML metadata: {e}"))?;
    let idps = ed
        .idp_sso_descriptors
        .as_ref()
        .filter(|v| !v.is_empty())
        .ok_or("no identity provider in the metadata")?;
    let sso_url = idps
        .iter()
        .flat_map(|d| d.single_sign_on_services.iter())
        .find(|ep| ep.binding == HTTP_REDIRECT)
        .map(|ep| ep.location.clone())
        .ok_or("the metadata has no HTTP-Redirect sign-on")?;
    let cert = idps
        .iter()
        .flat_map(|d| d.key_descriptors.iter())
        .filter(|k| k.is_signing() || k.key_use.is_none())
        .filter_map(|k| k.key_info.x509_data.as_ref())
        .flat_map(|x| x.certificates.iter())
        .map(|c| c.trim().to_owned())
        .find(|c| !c.is_empty())
        .ok_or("the metadata has no signing certificate")?;
    Ok((ed.entity_id.unwrap_or_default(), sso_url, cert))
}

/// Read an identity provider's metadata, to show what it says before saving.
async fn parse_metadata(Json(b): Json<Metadata>) -> Result<Json<Value>, ApiError> {
    let (entity_id, sso_url, cert) = idp_fields(&b.xml).map_err(ApiError::unprocessable)?;
    Ok(Json(json!({
        "idp_metadata_xml": b.xml,
        "idp_entity_id": entity_id,
        "sso_url": sso_url,
        "signing_cert": cert,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_with_a_dtd_or_too_large_are_refused_before_parsing() {
        use base64::Engine;
        let enc = |x: &str| base64::engine::general_purpose::STANDARD.encode(x);
        assert!(check_response(&enc("<samlp:Response ID=\"r\"></samlp:Response>")).is_ok());
        let dtd = "<?xml version=\"1.0\"?><!DOCTYPE r [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><r>&x;</r>";
        assert!(check_response(&enc(dtd)).unwrap_err().contains("DTD"));
        assert!(
            check_response(&enc("<!doctype r><r/>")).is_err(),
            "case-insensitive"
        );
        assert!(check_response(&enc("<r><!ENTITY a 'b'></r>")).is_err());
        assert!(check_response("not base64!").is_err());
        assert!(
            check_response(&"A".repeat(MAX_RESPONSE_B64 + 4))
                .unwrap_err()
                .contains("too large")
        );
    }

    const K1: &[u8] = b"k1k1k1k1k1k1k1k1k1k1k1k1k1k1k1k1";
    const K2: &[u8] = b"k2k2k2k2k2k2k2k2k2k2k2k2k2k2k2k2";

    #[test]
    fn relay_state_is_ours() {
        let t = mint_relay("id-1", K1).unwrap();
        assert_eq!(verify_relay(&t, K1).as_deref(), Some("id-1"));
        assert!(verify_relay(&t, K2).is_none());
    }

    #[test]
    fn an_assertion_is_used_once() {
        let st = State::default();
        let until = Utc::now() + chrono::Duration::minutes(5);
        assert!(st.first_use("a1", until));
        assert!(!st.first_use("a1", until));
        assert!(st.first_use("a2", until));
    }

    #[test]
    fn single_sign_on_finds_or_makes_the_account_and_never_takes_a_local_one() {
        let mut db = ot_store::Db::open_in_memory().unwrap();
        // No role: refused, nothing made.
        assert!(account(&mut db, "n@x.org", None).unwrap().is_err());
        assert_eq!(db.user_count().unwrap(), 0);
        let u = account(&mut db, "n@x.org", Some(Role::Viewer))
            .unwrap()
            .unwrap();
        assert_eq!((u.origin.as_str(), u.role.as_str()), ("saml", "viewer"));
        assert!(!u.has_password);
        // The provider's role holds on the next sign-on.
        let u = account(&mut db, "n@x.org", Some(Role::TrackManager))
            .unwrap()
            .unwrap();
        assert_eq!(u.role, "track_manager");
        // No role now: refused, the account is kept.
        assert!(account(&mut db, "n@x.org", None).unwrap().is_err());
        // A local account is never signed on by the identity provider, even
        // with a role it maps (so SAML can't reach a local admin).
        db.create_user(&ot_store::NewUser {
            id: "l1",
            email: "l@x.org",
            name: "",
            role: "admin",
            password_hash: None,
            origin: "local",
        })
        .unwrap();
        assert_eq!(
            account(&mut db, "l@x.org", Some(Role::Viewer)).unwrap(),
            Err("a local account has this email")
        );
        assert_eq!(db.user_by_email("l@x.org").unwrap().unwrap().role, "admin");
        // Nor is a turned-off SAML account.
        let n = db.user_by_email("n@x.org").unwrap().unwrap();
        db.update_user(&n.id, None, None, Some(false)).unwrap();
        assert!(
            account(&mut db, "n@x.org", Some(Role::Viewer))
                .unwrap()
                .is_err()
        );
    }
    /// A response signed by an identity provider verifies through the same
    /// path the ACS uses, and a tampered one does not. The image runs this
    /// path with OpenSSL's FIPS provider only (docs/security/fips.md).
    #[test]
    fn a_signed_response_verifies_and_a_tampered_one_does_not() {
        use samael::idp::{CertificateParams, IdentityProvider, KeyType, Rsa};
        let idp = IdentityProvider::generate_new(KeyType::Rsa(Rsa::Rsa2048)).unwrap();
        let cert = idp
            .create_certificate(&CertificateParams {
                common_name: "https://idp.example.org",
                issuer_name: "https://idp.example.org",
                days_until_expiration: 30,
            })
            .unwrap();
        let cert_b64 = base64::engine::general_purpose::STANDARD.encode(cert.der_data());
        let metadata = format!(
            r#"<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" entityID="https://idp.example.org">
  <md:IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <md:KeyDescriptor use="signing"><ds:KeyInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#"><ds:X509Data><ds:X509Certificate>{cert_b64}</ds:X509Certificate></ds:X509Data></ds:KeyInfo></md:KeyDescriptor>
    <md:SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect" Location="https://idp.example.org/sso"/>
  </md:IDPSSODescriptor>
</md:EntityDescriptor>"#
        );
        let cfg = SamlSettings {
            enabled: true,
            idp_metadata_xml: metadata,
            ..SamlSettings::default()
        };
        let base = "https://ot.example.org";
        let sp = service_provider(&cfg, base).unwrap();
        let rid = format!("id-{}", crate::fips::random_hex::<16>());
        // samael's template leaves out the confirmation's expiry, which the
        // service provider requires; add it, then sign as an IdP would.
        let mut response = samael::idp::response_builder::build_response_template(
            &cert,
            "ann@example.org",
            &format!("{base}/api/v1/auth/saml/metadata"),
            "https://idp.example.org",
            &format!("{base}/api/v1/auth/saml/acs"),
            &rid,
            &[],
        );
        for c in response
            .assertion
            .as_mut()
            .and_then(|a| a.subject.as_mut())
            .and_then(|s| s.subject_confirmations.as_mut())
            .into_iter()
            .flatten()
        {
            if let Some(d) = c.subject_confirmation_data.as_mut() {
                d.not_on_or_after = Some(Utc::now() + chrono::Duration::minutes(5));
            }
        }
        use samael::crypto::CryptoProvider;
        let signed = samael::crypto::Crypto::sign_xml(
            response.to_string().unwrap(),
            idp.export_private_key_der().unwrap().as_slice(),
        )
        .unwrap();
        let b64 = |x: &str| base64::engine::general_purpose::STANDARD.encode(x);
        let a = sp
            .parse_base64_response(&b64(&signed), Some(&[rid.as_str()]))
            .expect("a signed response verifies");
        assert_eq!(name_id(&a).as_deref(), Some("ann@example.org"));
        let forged = signed.replace("ann@example.org", "eve@example.org");
        assert!(
            sp.parse_base64_response(&b64(&forged), Some(&[rid.as_str()]))
                .is_err()
        );
        // Answering another request is refused too.
        assert!(
            sp.parse_base64_response(&b64(&signed), Some(&["id-other"]))
                .is_err()
        );
    }
}
