//! Sign-in and roles, modelled on OpenStare's: local accounts with
//! PBKDF2 passwords (FIPS), SAML single sign-on, a signed session cookie, and
//! API tokens for machines. Beside OpenStare it can also accept
//! OpenStare's own sign-in, so users sign in once.
//!
//! Every API request passes [`layer`]: it finds who is calling (a client
//! certificate, an API token, the session cookie, or OpenStare's session),
//! checks the role the path needs ([`policy`]), and sets the actor the
//! decision log records. It also holds each client to its request rate
//! ([`rate`]) and refuses changes forged from another site ([`csrf`]). The role always comes from the account as it is
//! now, so a role change or a deactivation takes effect at once.

pub mod access;
pub mod api;
pub mod consent;
pub mod csrf;
pub mod maintenance;
mod openstare;
pub mod password;
pub mod policy;
pub mod rate;
#[cfg(feature = "saml")]
mod saml;
#[cfg(feature = "saml")]
pub use saml::openssl_fips;
pub mod sessions;
pub mod settings;
pub mod stig;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use anyhow::Context;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::Common;
use crate::control::AppState;
pub use settings::AuthSettings;

/// The session cookie. Not `session`: that is OpenStare's, on the same host.
pub const COOKIE: &str = "ot_session";
/// OpenStare's session cookie, accepted when OpenTrack trusts OpenStare.
pub const OPENSTARE_COOKIE: &str = "session";
/// The header the handlers read the actor from; set here, never by clients.
pub const ACTOR_HEADER: &str = "x-opentrack-actor";

/// What an account may do; each includes the ones before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Sees tracks, sources and settings.
    Viewer,
    /// Also manages tracks: pair, merge, split, delete, group, designate, undo.
    TrackManager,
    /// Also configures OpenTrack: sources, settings, plugins, accounts.
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::TrackManager => "track_manager",
            Self::Admin => "admin",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "viewer" => Some(Self::Viewer),
            "track_manager" => Some(Self::TrackManager),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

/// How a caller proved who it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    Session,
    ApiToken,
    ClientCert,
    Openstare,
    /// Sign-in is turned off (`OT_AUTH=off`): everyone is an admin.
    Disabled,
}

/// The caller of a request, in its extensions.
#[derive(Debug, Clone, Serialize)]
pub struct AuthUser {
    pub id: String,
    pub email: String,
    pub name: String,
    pub role: Role,
    pub via: Via,
    /// The session token's id, to sign it out.
    #[serde(skip)]
    pub jti: Option<(String, i64)>,
    /// A password session that must change its password before anything
    /// else (a temporary or expired password).
    #[serde(skip)]
    pub must_change: bool,
    /// A session begun by single sign-on (SAML), not a password.
    #[serde(skip)]
    pub sso: bool,
}

/// A session or API token's payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Claims {
    /// The account id.
    sub: String,
    email: String,
    jti: String,
    iat: i64,
    exp: i64,
    /// `session` or `api`.
    kind: String,
    /// A session begun by single sign-on (SAML). Absent from older tokens.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    sso: bool,
}

/// A client certificate's subject, put in the request by the TLS listener.
#[derive(Debug, Clone)]
pub struct PeerCert {
    pub common_name: String,
}

/// In the requests of a listener that does not serve the admin routes:
/// with `OT_ADMIN_BIND` they are served only on the admin listener (SC-7,
/// SC-2), and the main listener answers them 404.
#[derive(Debug, Clone, Copy)]
pub struct AdminElsewhere;

/// Sign-in state shared by every request.
pub struct Auth {
    secret: Vec<u8>,
    /// `OT_AUTH=off`: no sign-in, every caller is an admin.
    pub disabled: bool,
    /// Served over TLS: cookies are `Secure`.
    pub secure_cookies: bool,
    /// Where browsers reach this server (SAML needs it).
    pub public_url: Option<String>,
    settings: RwLock<AuthSettings>,
    http: reqwest::Client,
    /// The address OpenStare was last reached at, logged when it changes.
    openstare_peer: ot_core::netlog::PeerLog,
    openstare_cache: Mutex<HashMap<String, (Instant, Option<AuthUser>)>>,
    attempts: Mutex<HashMap<IpAddr, (f64, Instant)>>,
    /// When each session was last used, saved in batches.
    pub activity: sessions::Activity,
    /// Who has accepted the notice-and-consent banner (AC-8).
    pub consent: consent::Consent,
    /// When API tokens, certificates and OpenStare identities were last
    /// audited as used.
    pub uses: access::Uses,
    /// Each client's API request rate ([`rate`]).
    pub rate: rate::Limiter,
    #[cfg(feature = "saml")]
    saml: saml::State,
}

/// Sign-in attempts per address: a burst of 5, then one a second.
const ATTEMPT_BURST: f64 = 5.0;

impl Auth {
    pub fn new(
        secret: Vec<u8>,
        disabled: bool,
        secure_cookies: bool,
        public_url: Option<String>,
        settings: AuthSettings,
    ) -> Self {
        Self {
            secret,
            disabled,
            secure_cookies,
            public_url: public_url.map(|u| u.trim_end_matches('/').to_owned()),
            settings: RwLock::new(settings),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            openstare_peer: Default::default(),
            openstare_cache: Mutex::new(HashMap::new()),
            attempts: Mutex::new(HashMap::new()),
            activity: sessions::Activity::default(),
            consent: consent::Consent::default(),
            uses: access::Uses::default(),
            rate: rate::Limiter::default(),
            #[cfg(feature = "saml")]
            saml: saml::State::default(),
        }
    }

    /// For tests: sign-in turned off.
    #[cfg(test)]
    pub fn off() -> Self {
        Self::new(b"test".to_vec(), true, false, None, AuthSettings::default())
    }

    pub fn settings(&self) -> AuthSettings {
        self.settings.read().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn set_settings(&self, s: AuthSettings) {
        if let Ok(mut w) = self.settings.write() {
            *w = s;
        }
        if let Ok(mut c) = self.openstare_cache.lock() {
            c.clear();
        }
    }

    fn session_secs(&self) -> i64 {
        stig::SESSION_HOURS * 3600
    }

    /// A signed token for an account; returns it with its id and expiry (ms).
    pub fn issue(
        &self,
        user: &ot_store::User,
        kind: &str,
        ttl_secs: i64,
    ) -> anyhow::Result<(String, String, i64)> {
        self.issue_as(user, kind, ttl_secs, false)
    }

    /// [`Auth::issue`], saying whether a session was begun by single sign-on.
    pub fn issue_as(
        &self,
        user: &ot_store::User,
        kind: &str,
        ttl_secs: i64,
        sso: bool,
    ) -> anyhow::Result<(String, String, i64)> {
        let now = chrono::Utc::now().timestamp();
        let jti = crate::fips::uuid_v4();
        let claims = Claims {
            sub: user.id.clone(),
            email: user.email.clone(),
            jti: jti.clone(),
            iat: now,
            exp: now + ttl_secs,
            kind: kind.to_owned(),
            sso,
        };
        let token = jsonwebtoken::encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(&self.secret),
        )?;
        Ok((token, jti, (now + ttl_secs) * 1000))
    }

    fn decode(&self, token: &str) -> Option<Claims> {
        let mut v = Validation::new(Algorithm::HS256);
        v.validate_exp = true;
        v.leeway = 5;
        jsonwebtoken::decode::<Claims>(token, &DecodingKey::from_secret(&self.secret), &v)
            .ok()
            .map(|d| d.claims)
    }

    /// Whether a sign-in attempt from `ip` may go ahead (a token bucket).
    pub fn allow_attempt(&self, ip: IpAddr) -> bool {
        let Ok(mut m) = self.attempts.lock() else {
            return true;
        };
        let now = Instant::now();
        if m.len() > 10_000 {
            m.retain(|_, (_, t)| now.duration_since(*t) < Duration::from_secs(60));
        }
        let (tokens, at) = m.entry(ip).or_insert((ATTEMPT_BURST, now));
        *tokens = (*tokens + now.duration_since(*at).as_secs_f64()).min(ATTEMPT_BURST);
        *at = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// The `Set-Cookie` value for a new session. `SameSite=Strict`: the
    /// SAML POST binding still works, as the cookie is set on the
    /// cross-site POST's response and only the page load it redirects to
    /// goes without it; the page's own API calls are same-site.
    /// The session cookie: a browser-session cookie (no `Max-Age` or
    /// `Expires`), so closing the browser drops it (ASD V-222578); the
    /// session's idle and absolute limits are kept on the server.
    pub fn session_cookie(&self, token: &str) -> HeaderValue {
        let secure = if self.secure_cookies { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/{secure}"
        ))
        .unwrap_or_else(|_| HeaderValue::from_static(""))
    }

    pub fn clear_cookie(&self) -> HeaderValue {
        if self.secure_cookies {
            HeaderValue::from_static(
                "ot_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0; Secure",
            )
        } else {
            HeaderValue::from_static("ot_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")
        }
    }
}

/// The key session tokens are signed with: `OT_SESSION_SECRET`, or one
/// made on first start and kept beside the database (readable by this user
/// only), so sessions survive a restart.
pub fn load_secret(common: &Common, env: Option<&str>) -> anyhow::Result<Vec<u8>> {
    if let Some(s) = env.map(str::trim).filter(|s| !s.is_empty()) {
        anyhow::ensure!(
            s.len() >= 32,
            "OT_SESSION_SECRET must be at least 32 characters"
        );
        return Ok(s.as_bytes().to_vec());
    }
    let dir = common
        .sqlite
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let path = dir.join("session.key");
    if let Ok(s) = std::fs::read_to_string(&path) {
        let s = s.trim();
        if s.len() >= 32 {
            return Ok(s.as_bytes().to_vec());
        }
    }
    std::fs::create_dir_all(dir)?;
    let key = crate::fips::random_hex::<32>();
    write_private(&path, &key).with_context(|| format!("writing {}", path.display()))?;
    tracing::info!(path = %path.display(), "made the session signing key");
    Ok(key.into_bytes())
}

/// Write a file only its owner can read.
pub fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(text.as_bytes())
}

/// PBKDF2-HMAC-SHA256 in the FIPS module (see [`crate::fips`]): slow on
/// purpose, so run it off the async runtime.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    Ok(crate::fips::hash_password(password))
}

/// Checks a password against its stored hash. Hashes from before 0.4.0 are
/// Argon2id; they still verify (and are rehashed with PBKDF2 at that
/// sign-in, see [`needs_rehash`]), so no one is locked out by the change.
pub fn verify_password(password: &str, hash: &str) -> bool {
    if let Some(ok) = crate::fips::verify_password(password, hash) {
        return ok;
    }
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    PasswordHash::new(hash).is_ok_and(|h| {
        argon2::Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}

pub use crate::fips::needs_rehash;

pub const MIN_PASSWORD: usize = 8;

pub fn check_password(p: &str) -> Result<(), String> {
    if p.chars().count() < MIN_PASSWORD {
        return Err(format!(
            "a password needs at least {MIN_PASSWORD} characters"
        ));
    }
    Ok(())
}

/// The first account: an admin from `OT_ADMIN_EMAIL` and
/// `OT_ADMIN_PASSWORD`, or, without them, `admin@opentrack.local` with a
/// made-up password written beside the database (never logged).
pub fn bootstrap_admin(
    common: &Common,
    db: &mut ot_store::Db,
    email: Option<&str>,
    password: Option<&str>,
) -> anyhow::Result<()> {
    if db.user_count()? > 0 {
        return Ok(());
    }
    let (email, password, file) = match (email, password) {
        (Some(e), Some(p)) if !e.trim().is_empty() => {
            password::check(p).map_err(|e| anyhow::anyhow!("OT_ADMIN_PASSWORD: {e}"))?;
            (e.trim().to_owned(), p.to_owned(), None)
        }
        _ => {
            let p = password::generate();
            let dir = common
                .sqlite
                .parent()
                .filter(|d| !d.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            std::fs::create_dir_all(dir)?;
            let path = dir.join("initial-admin.txt");
            write_private(
                &path,
                &format!(
                    "OpenTrack's first account. Sign in and choose a new password (you will be asked), then delete this file.\n\nemail:    admin@opentrack.local\npassword: {p}\n"
                ),
            )?;
            ("admin@opentrack.local".to_owned(), p, Some(path))
        }
    };
    let hash = hash_password(&password)?;
    let id = crate::fips::uuid_v4();
    db.create_user(&ot_store::NewUser {
        id: &id,
        email: &email,
        name: "Administrator",
        role: Role::Admin.as_str(),
        password_hash: Some(&hash),
        origin: "local",
    })?;
    if file.is_some() {
        // A made-up password is a temporary one.
        db.set_must_change_password(&id, true)?;
    }
    db.record(&ot_store::Decision {
        after: Some(json!({ "email": email, "role": "admin" })),
        ..ot_store::Decision::new("system", "create_user").reason("the first account")
    })?;
    match file {
        Some(path) => tracing::warn!(
            %email,
            path = %path.display(),
            "made the first admin account: its password is in this file"
        ),
        None => tracing::info!(%email, "made the first admin account"),
    }
    Ok(())
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
        .filter(|v| !v.is_empty())
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}

/// The account a token of ours names, if the token still stands (and, for
/// a session, its session is live: not ended, not idle too long).
/// `idle_ms`: how long the browser says its user has been idle.
async fn from_token(s: &AppState, token: &str, via: Via, idle_ms: i64) -> Option<AuthUser> {
    let claims = s.auth.decode(token)?;
    if (via == Via::ApiToken) != (claims.kind == "api") {
        return None;
    }
    let (sub, jti) = (claims.sub.clone(), claims.jti.clone());
    let session = via == Via::Session;
    let (user, revoked, row) = s
        .with_db(move |db| {
            let row = if session { db.session(&jti)? } else { None };
            Ok((db.user(&sub)?, db.token_revoked(&jti)?, row))
        })
        .await
        .ok()?;
    let user = user?;
    // A session's own row says when it began (to the millisecond), so a
    // session started just after "sign out everywhere" stands; an API
    // token's row is revoked with it, so its issue time only needs to be
    // before that second.
    let expired = user.expired(chrono::Utc::now().timestamp_millis());
    if !user.active
        || expired
        || revoked
        || (!session && claims.iat < user.tokens_valid_from_ms / 1000)
    {
        return None;
    }
    let role = Role::parse(&user.role)?;
    if session && !sessions::alive(s, &user, role, row.as_ref(), idle_ms).await {
        return None;
    }
    Some(AuthUser {
        role,
        must_change: session && user.must_change_password,
        id: user.id,
        email: user.email,
        name: user.name,
        via,
        sso: session && claims.sso,
        jti: Some((claims.jti, claims.exp * 1000)),
    })
}

/// The account a client certificate maps to (Settings → Security).
async fn from_cert(s: &AppState, cert: &PeerCert) -> Option<AuthUser> {
    let email = s
        .auth
        .settings()
        .client_certs
        .iter()
        .find(|m| m.common_name == cert.common_name)?
        .user
        .clone();
    let user = s.with_db(move |db| db.user_by_email(&email)).await.ok()??;
    if !user.active || user.expired(chrono::Utc::now().timestamp_millis()) {
        return None;
    }
    Some(AuthUser {
        role: Role::parse(&user.role)?,
        id: user.id,
        email: user.email,
        name: user.name,
        via: Via::ClientCert,
        jti: None,
        must_change: false,
        sso: false,
    })
}

/// Who is calling: a client certificate, an API token, our session
/// cookie, or (when trusted) OpenStare's session or token.
pub async fn identify(
    s: &AppState,
    headers: &HeaderMap,
    cert: Option<&PeerCert>,
) -> Option<AuthUser> {
    if s.auth.disabled {
        return Some(AuthUser {
            id: "anonymous".into(),
            email: "anonymous".into(),
            name: "Sign-in turned off".into(),
            role: Role::Admin,
            via: Via::Disabled,
            jti: None,
            must_change: false,
            sso: false,
        });
    }
    if let Some(cert) = cert
        && let Some(u) = from_cert(s, cert).await
    {
        return Some(u);
    }
    let bearer = bearer(headers);
    if let Some(t) = &bearer
        && let Some(u) = from_token(s, t, Via::ApiToken, 0).await
    {
        return Some(u);
    }
    if let Some(t) = cookie(headers, COOKIE)
        && let Some(u) = from_token(s, &t, Via::Session, sessions::client_idle_ms(headers)).await
    {
        return Some(u);
    }
    if s.auth.settings().openstare.enabled {
        let os_cookie = cookie(headers, OPENSTARE_COOKIE);
        if os_cookie.is_some() || bearer.is_some() {
            return openstare::identify(s, os_cookie.as_deref(), bearer.as_deref()).await;
        }
    }
    None
}

fn deny(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(json!({ "error": message }))).into_response()
}

/// The API's gate: find the caller, check the role the path needs, and
/// record the caller as the actor. Every request is logged (`access`), and
/// what the audit record needs is recorded ([`access`]).
pub async fn layer(State(s): State<AppState>, req: Request, next: Next) -> Response {
    let started = std::time::Instant::now();
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().cloned();
    let ip = client_ip(peer.as_ref()).to_string();
    let (method, path, headers) = (
        req.method().clone(),
        req.uri().path().to_owned(),
        req.headers().clone(),
    );
    let (res, actor) = access::CLIENT_IP
        .scope(ip.clone(), gate(&s, req, next, &method, &path, &ip))
        .await;
    access::log(
        actor.as_deref(),
        &method,
        &path,
        res.status(),
        &ip,
        &headers,
        started.elapsed().as_millis(),
    );
    res
}

async fn gate(
    s: &AppState,
    mut req: Request,
    next: Next,
    method: &axum::http::Method,
    path: &str,
    ip: &str,
) -> (Response, Option<String>) {
    // Only this layer says who the actor is.
    req.headers_mut().remove(ACTOR_HEADER);
    // Paths under /api/v1, as the policy names them.
    let api_path = path.strip_prefix("/api/v1").unwrap_or(path);
    let need = policy::need(method, api_path);
    let by_ip = format!("ip:{ip}");
    if need == policy::Need::Public {
        if let Some(r) = limited(s, &by_ip, "anonymous", method, api_path, ip).await {
            return (r, None);
        }
        // Signing in or out from another site's page (login CSRF).
        if let Err(r) = csrf::check(
            method,
            api_path,
            req.headers(),
            s.auth.public_url.as_deref(),
            false,
        ) {
            return (forged(s, "anonymous", &r, method, api_path, ip).await, None);
        }
        return (next.run(req).await, None);
    }
    let cert = req.extensions().get::<PeerCert>().cloned();
    let Some(user) = identify(s, req.headers(), cert.as_ref()).await else {
        if let Some(r) = limited(s, &by_ip, "anonymous", method, api_path, ip).await {
            return (r, None);
        }
        if access::credential_presented(req.headers(), cert.is_some()) {
            let actor = cert.as_ref().map_or("anonymous".to_owned(), |c| {
                format!("cert:{}", c.common_name)
            });
            access::refused(s, &actor, "access_refused", method, api_path, ip, json!({ "reason": "the credential identifies no one (invalid, expired, revoked or unknown)" })).await;
        }
        return (deny(StatusCode::UNAUTHORIZED, "sign in first"), None);
    };
    let email = Some(user.email.clone());
    if let Some(r) = limited(s, &rate_key(&user, ip), &user.email, method, api_path, ip).await {
        return (r, email);
    }
    let cookie_borne =
        user.via == Via::Session || (user.via == Via::Openstare && bearer(req.headers()).is_none());
    if let Err(r) = csrf::check(
        method,
        api_path,
        req.headers(),
        s.auth.public_url.as_deref(),
        cookie_borne,
    ) {
        return (
            forged(s, &user.email, &r, method, api_path, ip).await,
            email,
        );
    }
    // The notice-and-consent banner first, as the page shows it (AC-8).
    if !policy::allowed_before_consent(api_path) && s.auth.consent.required(s, &user).await {
        let r = (
            StatusCode::FORBIDDEN,
            axum::Json(json!({
                "error": "accept the notice first",
                "code": "consent_required",
            })),
        )
            .into_response();
        return (r, email);
    }
    if user.must_change && !policy::allowed_before_password_change(api_path) {
        let r = (
            StatusCode::FORBIDDEN,
            axum::Json(json!({
                "error": "change your password first",
                "code": "password_change_required",
            })),
        )
            .into_response();
        return (r, email);
    }
    if let policy::Need::Role(role) = need
        && user.role < role
    {
        access::refused(
            s,
            &user.email,
            "access_denied",
            method,
            api_path,
            ip,
            json!({ "role": user.role, "needs": role }),
        )
        .await;
        let r = deny(
            StatusCode::FORBIDDEN,
            &format!("this needs the {} role", role.as_str()),
        );
        return (r, email);
    }
    // After the role check, so a caller without the role is refused (403,
    // audited) as anywhere. For an admin this listener has no admin
    // interface at all: it is not here (404), not a permission decision.
    if need == policy::Need::Role(Role::Admin) && req.extensions().get::<AdminElsewhere>().is_some()
    {
        let r = deny(
            StatusCode::NOT_FOUND,
            "not served on this address: admin reads and changes are on the admin listener",
        );
        return (r, email);
    }
    if let Ok(v) = HeaderValue::from_str(&user.email) {
        req.headers_mut().insert(ACTOR_HEADER, v);
    }
    req.extensions_mut().insert(user.clone());
    let res = next.run(req).await;
    access::after(s, &user, method, api_path, res.status(), ip).await;
    (res, email)
}

/// Whose bucket a request comes from ([`rate`]): an account, all its
/// sessions together; each API token on its own; with sign-in off, the
/// address.
fn rate_key(user: &AuthUser, ip: &str) -> String {
    match (user.via, &user.jti) {
        (Via::Disabled, _) => format!("ip:{ip}"),
        (Via::ApiToken, Some((jti, _))) => format!("token:{jti}"),
        _ => format!("account:{}", user.id),
    }
}

/// A 429 when `client` is over its rate ([`rate`]); the first in a minute
/// is logged and audited (`rate_limited`), not every one.
async fn limited(
    s: &AppState,
    client: &str,
    actor: &str,
    method: &axum::http::Method,
    path: &str,
    ip: &str,
) -> Option<Response> {
    let rate::Verdict::Refuse { retry_after, note } = s.auth.rate.check(client) else {
        return None;
    };
    if note {
        tracing::warn!(client, actor, ip, "API rate limit reached");
        access::refused(
            s,
            actor,
            "rate_limited",
            method,
            path,
            ip,
            json!({ "client": client, "per_sec": rate::PER_SEC, "burst": rate::BURST }),
        )
        .await;
    }
    let mut r = deny(
        StatusCode::TOO_MANY_REQUESTS,
        "too many requests: slow down",
    );
    r.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
    Some(r)
}

/// Refuse a forged change ([`csrf`]), audited as `access_denied`.
async fn forged(
    s: &AppState,
    actor: &str,
    r: &csrf::Refusal,
    method: &axum::http::Method,
    path: &str,
    ip: &str,
) -> Response {
    let origin = match r {
        csrf::Refusal::ForeignOrigin(o) => Some(o.chars().take(256).collect::<String>()),
        csrf::Refusal::NoHeader => None,
    };
    access::refused(
        s,
        actor,
        "access_denied",
        method,
        path,
        ip,
        json!({ "reason": "csrf", "detail": r.message(), "origin": origin }),
    )
    .await;
    (
        StatusCode::FORBIDDEN,
        axum::Json(json!({ "error": r.message(), "code": "csrf" })),
    )
        .into_response()
}

/// The caller's address, when the server recorded it.
pub fn client_ip(req_ext: Option<&ConnectInfo<SocketAddr>>) -> IpAddr {
    req_ext.map_or(IpAddr::from([0, 0, 0, 0]), |c| c.0.ip())
}

pub type SharedAuth = Arc<Auth>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_cookie_ends_with_the_browser() {
        let c = Auth::off().session_cookie("tok");
        let c = c.to_str().unwrap();
        assert!(
            c.starts_with("ot_session=tok;")
                && c.contains("HttpOnly")
                && c.contains("SameSite=Strict")
        );
        assert!(!c.contains("Max-Age") && !c.contains("Expires"), "{c}");
    }

    #[test]
    fn passwords_hash_and_verify() {
        let h = hash_password("correct horse").unwrap();
        assert!(h.starts_with("$pbkdf2-sha256$"));
        assert!(verify_password("correct horse", &h));
        assert!(!verify_password("wrong horse", &h));
        assert!(!verify_password("x", "not a hash"));
        assert!(check_password("short").is_err());
    }

    #[test]
    fn tokens_carry_their_kind_and_are_signed() {
        let a = Auth::new(vec![7; 32], false, false, None, AuthSettings::default());
        let u = ot_store::User {
            id: "u1".into(),
            email: "a@b".into(),
            name: String::new(),
            role: "viewer".into(),
            active: true,
            origin: "local".into(),
            has_password: true,
            created_at_ms: 0,
            updated_at_ms: 0,
            last_login_at_ms: None,
            tokens_valid_from_ms: 0,
            password_changed_at_ms: None,
            must_change_password: false,
            failed_logins: 0,
            locked_until_ms: None,
            active_since_ms: None,
            disabled_reason: None,
            expires_at_ms: None,
        };
        let (t, jti, _) = a.issue(&u, "session", 60).unwrap();
        let c = a.decode(&t).unwrap();
        assert_eq!(
            (c.sub.as_str(), c.kind.as_str(), c.jti),
            ("u1", "session", jti)
        );
        assert!(!c.sso, "a password session");
        let (t, ..) = a.issue_as(&u, "session", 60, true).unwrap();
        assert!(a.decode(&t).unwrap().sso, "a single sign-on session");
        let other = Auth::new(vec![8; 32], false, false, None, AuthSettings::default());
        assert!(other.decode(&t).is_none(), "another key's token");
        let (expired, _, _) = a.issue(&u, "session", -60).unwrap();
        assert!(a.decode(&expired).is_none());
    }

    #[test]
    fn sign_in_attempts_are_limited_per_address() {
        let a = Auth::off();
        let ip = IpAddr::from([10, 0, 0, 1]);
        assert!((0..5).all(|_| a.allow_attempt(ip)));
        assert!(!a.allow_attempt(ip));
        assert!(a.allow_attempt(IpAddr::from([10, 0, 0, 2])));
    }

    #[test]
    fn cookies_are_found_among_others() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; ot_session=tok; session=os"),
        );
        assert_eq!(cookie(&h, COOKIE).as_deref(), Some("tok"));
        assert_eq!(cookie(&h, OPENSTARE_COOKIE).as_deref(), Some("os"));
        assert_eq!(cookie(&h, "b"), None);
    }
}
