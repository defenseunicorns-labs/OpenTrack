//! Sign-in endpoints, an account's own profile and password, and the
//! admin's accounts, API tokens and sign-in settings. Every admin change
//! goes in the decision log.

use std::net::SocketAddr;
use std::sync::OnceLock;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use ot_store::AuditEvent;
use ot_store::sqlite::now_ms;

use super::sessions::ended;
use super::{AdminElsewhere, AuthSettings, AuthUser, Role, Via, hash_password, verify_password};
use super::{password, stig};
use crate::api::actor;
use crate::control::{ApiError, AppState};

pub fn routes() -> Router<AppState> {
    let r = Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/public", get(public))
        .route("/auth/me", get(me))
        .route("/auth/consent", post(consent))
        .route("/auth/password", post(change_password))
        .route("/auth/users", get(list_users).post(create_user))
        .route("/auth/users/{id}", put(update_user).delete(delete_user))
        .route("/auth/users/{id}/password", post(reset_password))
        .route("/auth/users/{id}/revoke", post(revoke_sessions))
        .route("/auth/users/{id}/unlock", post(unlock_user))
        .route("/auth/api-tokens", get(list_tokens).post(create_token))
        .route(
            "/auth/api-tokens/{jti}",
            axum::routing::delete(revoke_token),
        )
        .route("/auth/settings", get(get_settings).put(put_settings))
        .merge(super::sessions::routes());
    #[cfg(feature = "saml")]
    let r = r.merge(super::saml::routes());
    r
}

fn unauthorized(m: &str) -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": m }))).into_response()
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// A hash to check unknown emails against, so a wrong email takes as long
/// as a wrong password.
fn dummy_hash() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| hash_password("not a password of anyone's").unwrap_or_default())
}

#[derive(Deserialize)]
struct Login {
    email: String,
    password: String,
}

/// Where a sign-in comes from: the address and browser, for its session
/// and the audit record.
#[derive(Debug, Clone, Default)]
pub(super) struct Client {
    pub ip: String,
    pub user_agent: Option<String>,
}

impl Client {
    pub(super) fn new(peer: Option<&ConnectInfo<SocketAddr>>, headers: &HeaderMap) -> Self {
        Self {
            ip: super::client_ip(peer).to_string(),
            user_agent: headers
                .get(header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.chars().take(512).collect()),
        }
    }
}

/// How a session begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum How {
    Password,
    #[cfg_attr(not(feature = "saml"), allow(dead_code))]
    Saml,
    /// A new session for the browser that just changed its password.
    PasswordChange,
}

impl How {
    fn as_str(self) -> &'static str {
        match self {
            Self::Password => "password",
            Self::Saml => "saml",
            Self::PasswordChange => "password_change",
        }
    }
}

/// An email as the audit record keeps an attempt's (bounded).
fn attempted(email: &str) -> String {
    email.trim().chars().take(254).collect()
}

/// A refused sign-in, recorded before it is answered (if the record cannot
/// be written, the answer is an error all the same).
fn failed_login(email: &str, client: &Client, reason: &str, via: How) -> AuditEvent {
    AuditEvent::new(attempted(email), "login")
        .failure()
        .ip(&client.ip)
        .detail(json!({ "reason": reason, "via": via.as_str() }))
}

/// The account as the UI shows it: how it signed in, whether it must
/// change its password, and (for a session) the previous sign-in and the
/// failed ones since.
/// Accept the notice-and-consent banner (AC-8), for this session (or, for
/// certificate and OpenStare access, this account for a while); audited.
async fn consent(
    State(s): State<AppState>,
    Extension(u): Extension<AuthUser>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    s.auth.consent.accept(&u);
    let c = Client::new(peer.as_deref(), &headers);
    let (email, ip, via) = (u.email.clone(), c.ip, u.via);
    s.with_db(move |db| {
        db.audit(
            &AuditEvent::new(&email, "consent_accepted")
                .ip(&ip)
                .detail(json!({ "via": via })),
        )
    })
    .await?;
    Ok(Json(json!({ "consent_required": false })))
}

/// `admin_api`: whether this listener serves the admin routes (not when
/// they are on the admin listener, `OT_ADMIN_BIND`), so the page shows
/// admin areas only where they work.
pub(super) async fn me_value(
    s: &AppState,
    u: &AuthUser,
    admin_api: bool,
) -> Result<Value, ApiError> {
    let session = u.via == Via::Session;
    let (id, jti) = (u.id.clone(), u.jti.as_ref().map(|j| j.0.clone()));
    let (user, row) = if session {
        s.with_db(move |db| {
            let row = match &jti {
                Some(j) => db.session(j)?,
                None => None,
            };
            Ok((db.user(&id)?, row))
        })
        .await?
    } else {
        (None, None)
    };
    let has_password = user.as_ref().is_some_and(|u| u.has_password);
    let mut v = json!({
        "id": u.id, "email": u.email, "name": u.name, "role": u.role, "via": u.via,
        "can_change_password": session && has_password,
        "must_change_password": false,
        "consent_required": s.auth.consent.required(s, u).await,
        "admin_api": admin_api,
    });
    if let Some(user) = &user {
        v["must_change_password"] = json!(has_password && user.must_change_password);
        if has_password {
            v["password_expires_at_ms"] = json!(
                user.password_changed_at_ms
                    .map(|t| t + stig::PASSWORD_MAX_AGE_MS)
            );
        }
        v["idle_timeout_ms"] = json!(stig::idle_ms(u.role));
        v["password_policy"] = stig::password_policy();
    }
    if let Some(row) = row {
        v["session"] = json!(row.id);
        v["last_login"] = json!({
            "previous_at_ms": row.prev_login_at_ms,
            "failed_attempts": row.failed_before,
        });
    }
    Ok(v)
}

/// Sign in to a session for `user` (after a password, single sign-on, or
/// a password change). Refused, and the account turned off, when it has
/// been inactive too long. The audit record is written first: if it cannot
/// be, there is no session.
pub(super) async fn start_session(
    s: &AppState,
    user: ot_store::User,
    client: &Client,
    how: How,
    admin_api: bool,
) -> Result<(HeaderMap, Value), ApiError> {
    let settings = s.auth.settings();
    let now = now_ms();
    let inactive = &settings.inactivity;
    if how != How::PasswordChange
        && !inactive.exempts(&user.email)
        && user.last_activity_ms() < now - stig::INACTIVE_MS
    {
        super::maintenance::disable_inactive(s, inactive).await?;
        let e = failed_login(&user.email, client, "inactive", how);
        s.with_db(move |db| db.audit(&e)).await?;
        return Err(ApiError::unauthorized(GENERIC_FAILURE));
    }
    let ttl = s.auth.session_secs();
    let (token, jti, exp) = s
        .auth
        .issue_as(&user, "session", ttl, how == How::Saml)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let expired = how == How::Password
        && !user.must_change_password
        && user
            .password_changed_at_ms
            .is_some_and(|t| now - t > stig::PASSWORD_MAX_AGE_MS);
    let max = stig::SESSIONS_PER_ACCOUNT;
    let (id, email, c, j) = (
        user.id.clone(),
        user.email.clone(),
        client.clone(),
        jti.clone(),
    );
    let pushed_out = s
        .with_db(move |db| {
            let op = if how == How::PasswordChange {
                "session_start"
            } else {
                "login"
            };
            db.audit(
                &AuditEvent::new(&email, op)
                    .ip(&c.ip)
                    .detail(json!({ "session": j, "via": how.as_str() })),
            )?;
            let (prev, failed) = if how == How::PasswordChange {
                (None, 0)
            } else {
                db.note_login_success(&id)?
            };
            if expired {
                db.set_must_change_password(&id, true)?;
                db.audit(&AuditEvent::new(&email, "password_expired").ip(&c.ip))?;
            }
            db.create_session(&ot_store::auth::NewSession {
                id: &j,
                user_id: &id,
                expires_at_ms: exp,
                ip: Some(&c.ip),
                user_agent: c.user_agent.as_deref(),
                prev_login_at_ms: prev,
                failed_before: failed,
            })?;
            // Past the limit, the oldest sessions end.
            let mut out = Vec::new();
            for old in db.sessions(Some(&id), false, 1000)?.iter().skip(max) {
                if db.end_session(&old.id, "limit")? {
                    db.revoke_token(&old.id, old.expires_at_ms)?;
                    db.audit(&ended(&email, &old.id, "limit", old.ip.as_deref()))?;
                    out.push(old.id.clone());
                }
            }
            Ok(out)
        })
        .await?;
    for old in pushed_out {
        s.auth.activity.forget(&old);
    }
    tracing::info!(email = %user.email, via = how.as_str(), "signed in");
    let mut headers = HeaderMap::new();
    headers.insert(header::SET_COOKIE, s.auth.session_cookie(&token));
    let role = Role::parse(&user.role).unwrap_or(Role::Viewer);
    let au = AuthUser {
        id: user.id,
        email: user.email,
        name: user.name,
        role,
        via: Via::Session,
        jti: Some((jti, exp)),
        must_change: false,
        sso: how == How::Saml,
    };
    Ok((headers, me_value(s, &au, admin_api).await?))
}

/// What every refused password sign-in says, whatever the reason.
const GENERIC_FAILURE: &str = "wrong email or password";

async fn login(
    State(s): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    elsewhere: Option<Extension<AdminElsewhere>>,
    headers: HeaderMap,
    Json(b): Json<Login>,
) -> Result<Response, ApiError> {
    if s.auth.disabled {
        return Err(ApiError::bad_request("sign-in is turned off (OT_AUTH=off)"));
    }
    let settings = s.auth.settings();
    if settings.disable_password_login {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "password sign-in is off: use single sign-on" })),
        )
            .into_response());
    }
    let client = Client::new(peer.as_ref().map(|p| &p.0), &headers);
    if !s
        .auth
        .allow_attempt(super::client_ip(peer.as_ref().map(|p| &p.0)))
    {
        tracing::warn!(ip = %client.ip, "sign-in attempts limited");
        return Ok((
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "too many attempts: wait a moment" })),
        )
            .into_response());
    }
    let email = b.email.trim().to_owned();
    let lookup = email.clone();
    let found = s.with_db(move |db| db.password_hash(&lookup)).await?;
    // Unknown, turned-off and password-less accounts are checked against a
    // dummy hash, so every refusal takes as long as a wrong password.
    let (user, hash, refusal) = match found {
        Some((u, Some(h))) if u.active => (Some(u), h, None),
        Some((u, _)) if !u.active || u.expired(now_ms()) => {
            (None, dummy_hash().to_owned(), Some("account_disabled"))
        }
        Some(_) => (None, dummy_hash().to_owned(), Some("no_password")),
        None => (None, dummy_hash().to_owned(), Some("unknown_account")),
    };
    let rehash = super::needs_rehash(&hash);
    let password = b.password.clone();
    let ok = blocking(move || verify_password(&password, &hash)).await?;
    let now = now_ms();
    let refuse = |e: AuditEvent| {
        let s = s.clone();
        async move {
            s.with_db(move |db| db.audit(&e)).await?;
            Ok::<_, ApiError>(unauthorized(GENERIC_FAILURE))
        }
    };
    let Some(user) = user else {
        let reason = refusal.unwrap_or("unknown_account");
        tracing::info!(%email, reason, "sign-in refused");
        return refuse(failed_login(&email, &client, reason, How::Password)).await;
    };
    if user.locked(now) {
        tracing::info!(%email, "sign-in refused: the account is locked");
        let id = user.id.clone();
        s.with_db(move |db| db.note_refused_login(&id)).await?;
        return refuse(failed_login(&email, &client, "locked", How::Password)).await;
    }
    if !ok {
        tracing::info!(%email, "sign-in refused");
        let (id, e1, c1) = (user.id.clone(), email.clone(), client.clone());
        s.with_db(move |db| {
            let f = db.note_login_failure(
                &id,
                stig::LOCKOUT_WINDOW_MS,
                i64::from(stig::LOCKOUT_FAILURES),
                stig::LOCK_MS,
            )?;
            db.audit(
                &failed_login(&e1, &c1, "bad_password", How::Password)
                    .detail(json!({ "consecutive": f.count })),
            )?;
            if f.locked_now {
                tracing::warn!(email = %e1, "account locked after failed sign-ins");
                db.audit(
                    &AuditEvent::new(&e1, "account_locked")
                        .ip(&c1.ip)
                        .detail(json!({
                            "user": id,
                            "failures": f.count,
                            "until": "an admin unlocks it",
                        })),
                )?;
            }
            Ok(())
        })
        .await?;
        return Ok(unauthorized(GENERIC_FAILURE));
    }
    // Hashes from before 0.4.0 (Argon2id) move to PBKDF2 in the FIPS module;
    // the password itself is unchanged, so its age and history are too.
    if rehash {
        let new = blocking(move || hash_password(&b.password))
            .await?
            .map_err(|e| ApiError::internal(e.to_string()))?;
        let id = user.id.clone();
        s.with_db(move |db| db.rehash_password(&id, &new)).await?;
        tracing::info!(user = %user.email, "password hash upgraded to PBKDF2");
    }
    let (headers, body) =
        start_session(&s, user, &client, How::Password, elsewhere.is_none()).await?;
    Ok((headers, Json(body)).into_response())
}

async fn logout(
    State(s): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
) -> Response {
    let user = super::identify(&s, &headers, None).await;
    if let Some(u) = user
        && let Some((jti, exp)) = u.jti.clone()
        && headers.get(header::AUTHORIZATION).is_none()
    {
        let ip = Client::new(peer.as_ref().map(|p| &p.0), &headers).ip;
        s.auth.activity.forget(&jti);
        let r = s
            .with_db(move |db| {
                db.revoke_token(&jti, exp)?;
                if db.end_session(&jti, "sign_out")? {
                    db.audit(
                        &AuditEvent::new(&u.email, "logout")
                            .ip(ip)
                            .detail(json!({ "session": jti })),
                    )?;
                }
                Ok(())
            })
            .await;
        if let Err(e) = r {
            tracing::error!(error = %e.message, "recording a sign-out failed");
        }
    }
    let mut h = HeaderMap::new();
    h.insert(header::SET_COOKIE, s.auth.clear_cookie());
    (StatusCode::NO_CONTENT, h).into_response()
}

/// What the sign-in page offers.
async fn public(State(s): State<AppState>) -> Json<Value> {
    let st = s.auth.settings();
    Json(json!({
        "auth": !s.auth.disabled,
        "password_login": !st.disable_password_login,
        "saml": { "enabled": st.saml.enabled && saml_ready(), "label": st.saml.button_label },
        "openstare": {
            "enabled": st.openstare.enabled,
            "login_url": st.openstare.login_url,
        },
    }))
}

async fn me(
    State(s): State<AppState>,
    Extension(u): Extension<AuthUser>,
    elsewhere: Option<Extension<AdminElsewhere>>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(me_value(&s, &u, elsewhere.is_none()).await?))
}

#[derive(Deserialize)]
struct ChangePassword {
    current: String,
    new: String,
}

/// Change your own password (the policy's rules, its minimum age unless
/// the change is required, and not one of the recent ones); every session
/// and API token of the account ends and this browser gets a new session.
async fn change_password(
    State(s): State<AppState>,
    Extension(u): Extension<AuthUser>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    elsewhere: Option<Extension<AdminElsewhere>>,
    headers: HeaderMap,
    Json(b): Json<ChangePassword>,
) -> Result<Response, ApiError> {
    if u.via != Via::Session {
        return Err(ApiError::bad_request(
            "only an account signed in with a password can change it here",
        ));
    }
    let client = Client::new(peer.as_ref().map(|p| &p.0), &headers);
    password::check(&b.new).map_err(ApiError::unprocessable)?;
    let email = u.email.clone();
    let Some((user, Some(hash))) = s.with_db(move |db| db.password_hash(&email)).await? else {
        return Err(ApiError::bad_request("this account has no password"));
    };
    let current = b.current.clone();
    if !blocking(move || verify_password(&current, &hash)).await? {
        let e = AuditEvent::new(&u.email, "change_password")
            .failure()
            .ip(&client.ip)
            .detail(json!({ "reason": "wrong_current_password" }));
        s.with_db(move |db| db.audit(&e)).await?;
        return Ok(unauthorized("the current password is wrong"));
    }
    password::check_change(&b.current, &b.new).map_err(ApiError::unprocessable)?;
    let forced = user.must_change_password;
    if !forced
        && user
            .password_changed_at_ms
            .is_some_and(|t| now_ms() - t < stig::PASSWORD_MIN_AGE_MS)
    {
        return Err(ApiError::unprocessable(format!(
            "a password can change once every {} hours: ask an admin to reset it if you must",
            stig::PASSWORD_MIN_AGE_HOURS
        )));
    }
    let id = user.id.clone();
    let recent = s
        .with_db(move |db| db.recent_password_hashes(&id, stig::PASSWORD_HISTORY))
        .await?;
    let candidate = b.new.clone();
    if blocking(move || recent.iter().any(|h| verify_password(&candidate, h))).await? {
        return Err(ApiError::unprocessable(format!(
            "choose a password other than your last {}",
            stig::PASSWORD_HISTORY
        )));
    }
    let new = blocking(move || hash_password(&b.new))
        .await?
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let (id, actor) = (user.id.clone(), u.email.clone());
    s.with_db(move |db| {
        db.set_password(&id, Some(&new), false, 24)?;
        db.revoke_user_tokens(&id)?;
        db.record(&ot_store::Decision {
            evidence: json!({ "user": id, "forced": forced }),
            ..ot_store::Decision::new(&actor, "change_password")
        })
    })
    .await?;
    // A fresh session for this browser: the old ones just ended.
    let user = s
        .with_db(move |db| db.user_by_email(&u.email))
        .await?
        .ok_or_else(|| ApiError::not_found("account"))?;
    let (headers, body) =
        start_session(&s, user, &client, How::PasswordChange, elsewhere.is_none()).await?;
    Ok((headers, Json(body)).into_response())
}

async fn list_users(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let users = s.with_db(|db| db.users()).await?;
    Ok(Json(json!({ "users": users })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewUser {
    email: String,
    #[serde(default)]
    name: String,
    role: Role,
    /// Without one the account can only use single sign-on.
    #[serde(default)]
    password: Option<String>,
    /// A temporary or emergency account: turned off
    /// [`stig::TEMPORARY_ACCOUNT_HOURS`] after it is made.
    #[serde(default)]
    temporary: bool,
}

fn check_email(e: &str) -> Result<(), ApiError> {
    let e = e.trim();
    if e.is_empty() || e.len() > 254 || !e.contains('@') || e.contains(char::is_whitespace) {
        return Err(ApiError::unprocessable(format!(
            "{e:?} is not an email address"
        )));
    }
    Ok(())
}

async fn create_user(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<NewUser>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    check_email(&b.email)?;
    let hash = match b.password {
        Some(p) => {
            password::check(&p).map_err(ApiError::unprocessable)?;
            Some(
                blocking(move || hash_password(&p))
                    .await?
                    .map_err(|e| ApiError::internal(e.to_string()))?,
            )
        }
        None => None,
    };
    let actor = actor(&headers);
    let user = s
        .with_db(move |db| {
            let id = crate::fips::uuid_v4();
            let u = db.create_user(&ot_store::NewUser {
                id: &id,
                email: &b.email,
                name: &b.name,
                role: b.role.as_str(),
                password_hash: hash.as_deref(),
                origin: "local",
            })?;
            // An admin's password is a temporary one.
            if hash.is_some() {
                db.set_must_change_password(&id, true)?;
            }
            if b.temporary {
                db.set_account_expiry(
                    &id,
                    Some(now_ms() + stig::TEMPORARY_ACCOUNT_HOURS * 3_600_000),
                )?;
            }
            let u = db.user(&id)?.unwrap_or(u);
            db.record(&ot_store::Decision {
                after: Some(json!(u)),
                ..ot_store::Decision::new(&actor, "create_user")
            })?;
            Ok(u)
        })
        .await?;
    Ok((StatusCode::CREATED, Json(json!(user))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserChange {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    role: Option<Role>,
    #[serde(default)]
    active: Option<bool>,
    /// Make the account temporary (turned off
    /// [`stig::TEMPORARY_ACCOUNT_HOURS`] from now), or permanent.
    #[serde(default)]
    temporary: Option<bool>,
}

/// Refuse a change that would leave no active admin.
fn keeps_an_admin(
    db: &ot_store::Db,
    id: &str,
    role: Option<Role>,
    active: Option<bool>,
    deleting: bool,
) -> ot_store::sqlite::Result<bool> {
    let users = db.users()?;
    let admins = users
        .iter()
        .filter(|u| u.active && u.role == "admin")
        .filter(|u| {
            if u.id != id {
                return true;
            }
            !deleting && active != Some(false) && role.is_none_or(|r| r == Role::Admin)
        })
        .count();
    Ok(admins > 0)
}

async fn update_user(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(b): Json<UserChange>,
) -> Result<Json<Value>, ApiError> {
    let actor = actor(&headers);
    let user = s
        .with_db(move |db| {
            if !keeps_an_admin(db, &id, b.role, b.active, false)? {
                return Err(ot_store::StoreError::Conflict(
                    "that would leave no active admin".into(),
                ));
            }
            let before = db.user(&id)?;
            // The identity provider is authoritative for a `saml` account's
            // role: it is set again at every sign-on.
            if let (Some(u), Some(r)) = (&before, b.role)
                && u.origin == "saml"
                && u.role != r.as_str()
            {
                return Err(ot_store::StoreError::Conflict(
                    "a saml account's role comes from the identity provider's role mapping \
                     (Settings -> Security -> SAML); change it there"
                        .into(),
                ));
            }
            // Turned on again, an expired temporary account needs a new
            // term (or to become permanent) first.
            if b.active == Some(true)
                && b.temporary.is_none()
                && before.as_ref().is_some_and(|u| u.expired(now_ms()))
            {
                return Err(ot_store::StoreError::Conflict(
                    "this temporary account's time has run out: give it another term \
                     (temporary: true) or make it permanent (temporary: false)"
                        .into(),
                ));
            }
            let mut u =
                db.update_user(&id, b.name.as_deref(), b.role.map(Role::as_str), b.active)?;
            if let Some(t) = b.temporary {
                let until = t.then(|| now_ms() + stig::TEMPORARY_ACCOUNT_HOURS * 3_600_000);
                u = db.set_account_expiry(&id, until)?;
            }
            db.record(&ot_store::Decision {
                before: before.map(|b| json!(b)),
                after: Some(json!(u)),
                ..ot_store::Decision::new(&actor, "update_user")
            })?;
            Ok(u)
        })
        .await?;
    Ok(Json(json!(user)))
}

async fn delete_user(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Extension(me): Extension<AuthUser>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    if me.id == id {
        return Err(ApiError::conflict("you cannot delete your own account"));
    }
    let actor = actor(&headers);
    s.with_db(move |db| {
        if !keeps_an_admin(db, &id, None, None, true)? {
            return Err(ot_store::StoreError::Conflict(
                "that would leave no active admin".into(),
            ));
        }
        let before = db.user(&id)?;
        db.delete_user(&id)?;
        db.record(&ot_store::Decision {
            before: before.map(|b| json!(b)),
            ..ot_store::Decision::new(&actor, "delete_user")
        })
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Reset {
    /// `None` removes the password (single sign-on only).
    password: Option<String>,
}

async fn reset_password(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(b): Json<Reset>,
) -> Result<StatusCode, ApiError> {
    let hash = match b.password {
        Some(p) => {
            password::check(&p).map_err(ApiError::unprocessable)?;
            Some(
                blocking(move || hash_password(&p))
                    .await?
                    .map_err(|e| ApiError::internal(e.to_string()))?,
            )
        }
        None => None,
    };
    let actor = actor(&headers);
    s.with_db(move |db| {
        // A temporary password: changed at the next sign-in.
        db.set_password(&id, hash.as_deref(), true, 24)?;
        db.revoke_user_tokens(&id)?;
        db.record(&ot_store::Decision {
            evidence: json!({ "user": id, "temporary": hash.is_some() }),
            ..ot_store::Decision::new(&actor, "reset_password")
        })
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Unlock an account locked by failed sign-ins.
async fn unlock_user(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let actor = actor(&headers);
    let user = s
        .with_db(move |db| {
            let before = db.user(&id)?;
            let u = db.unlock_user(&id)?;
            db.record(&ot_store::Decision {
                evidence: json!({ "user": id, "email": u.email }),
                before: before.map(|b| json!(b)),
                ..ot_store::Decision::new(&actor, "unlock_user")
            })?;
            Ok(u)
        })
        .await?;
    Ok(Json(json!(user)))
}

async fn revoke_sessions(
    State(s): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let actor = actor(&headers);
    s.with_db(move |db| {
        db.revoke_user_tokens(&id)?;
        db.record(&ot_store::Decision {
            evidence: json!({ "user": id }),
            ..ot_store::Decision::new(&actor, "revoke_sessions")
        })
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_tokens(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let tokens = s.with_db(|db| db.api_tokens()).await?;
    Ok(Json(json!({ "tokens": tokens })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewToken {
    name: String,
    /// The account it acts as (default: yours).
    #[serde(default)]
    user_id: Option<String>,
    /// Days until it expires (default 365).
    #[serde(default)]
    days: Option<f64>,
}

/// Make an API token; it is shown this once.
async fn create_token(
    State(s): State<AppState>,
    Extension(me): Extension<AuthUser>,
    headers: HeaderMap,
    Json(b): Json<NewToken>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if b.name.trim().is_empty() {
        return Err(ApiError::unprocessable("give the token a name"));
    }
    let days = b.days.unwrap_or(365.0);
    if !(1.0 / 24.0..=3650.0).contains(&days) {
        return Err(ApiError::unprocessable("days: between 1 hour and 10 years"));
    }
    let id = b.user_id.unwrap_or(me.id);
    let lookup = id.clone();
    let user = s
        .with_db(move |db| db.user(&lookup))
        .await?
        .ok_or_else(|| ApiError::not_found(format!("account {id}")))?;
    let (token, jti, exp) = s
        .auth
        .issue(&user, "api", (days * 86_400.0) as i64)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    let actor = actor(&headers);
    let name = b.name.clone();
    let (j, uid) = (jti.clone(), user.id.clone());
    s.with_db(move |db| {
        db.add_api_token(&j, &name, &uid, &actor, exp)?;
        db.record(&ot_store::Decision {
            evidence: json!({ "token": j, "name": name, "user": uid }),
            ..ot_store::Decision::new(&actor, "create_api_token")
        })
    })
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(
            json!({ "token": token, "jti": jti, "name": b.name, "user": user.email, "expires_at_ms": exp }),
        ),
    ))
}

async fn revoke_token(
    State(s): State<AppState>,
    Path(jti): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let actor = actor(&headers);
    s.with_db(move |db| {
        db.revoke_api_token(&jti)?;
        db.record(&ot_store::Decision {
            evidence: json!({ "token": jti }),
            ..ot_store::Decision::new(&actor, "revoke_api_token")
        })
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_settings(State(s): State<AppState>) -> Json<Value> {
    let mut settings = s.auth.settings();
    // Shown as the metadata says, whatever older settings stored.
    let _ = settings.saml.read_metadata();
    let mut v = json!(settings);
    v["build"] = json!({ "saml": cfg!(feature = "saml"), "public_url": s.auth.public_url });
    Json(v)
}

async fn put_settings(
    State(s): State<AppState>,
    Extension(me): Extension<AuthUser>,
    headers: HeaderMap,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    if let Some(o) = body.as_object_mut() {
        o.remove("build");
    }
    let mut new: AuthSettings =
        serde_json::from_value(body).map_err(|e| ApiError::unprocessable(e.to_string()))?;
    // The identity provider's entity id, sign-in URL and certificate are
    // the metadata's: to change them, paste new metadata.
    if let Err(e) = new.saml.read_metadata()
        && new.saml.enabled
    {
        return Err(ApiError::unprocessable(format!("saml: {e}")));
    }
    new.validate().map_err(ApiError::unprocessable)?;
    // Turning password sign-in off from a password session would lock you
    // out if single sign-on then fails: make sure there is a way back. A
    // SAML session is that way back, while the same save keeps SAML on.
    if new.disable_password_login && me.via == Via::Session && !(me.sso && new.saml.enabled) {
        return Err(ApiError::conflict(if me.sso {
            "keep SAML on while turning password sign-in off from a SAML session"
        } else {
            "sign in with single sign-on before turning password sign-in off"
        }));
    }
    let before = s.auth.settings();
    let (actor, saved) = (actor(&headers), new.clone());
    s.with_db(move |db| {
        db.put_auth_settings(&json!(saved))?;
        db.record(&ot_store::Decision {
            before: Some(json!(before)),
            after: Some(json!(saved)),
            ..ot_store::Decision::new(&actor, "auth_settings")
        })
    })
    .await?;
    s.auth.set_settings(new);
    Ok(get_settings(State(s)).await)
}

/// SAML is built in and OpenSSL (which checks its signatures) is in FIPS mode.
fn saml_ready() -> bool {
    #[cfg(feature = "saml")]
    return super::saml::openssl_fips();
    #[cfg(not(feature = "saml"))]
    false
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::{Auth, AuthSettings};
    use crate::config::Common;

    /// The router with sign-in on, over an in-memory database holding one
    /// admin; `None` (test skipped) without `OT_TEST_REDIS_URL`.
    async fn app() -> Option<axum::Router> {
        app_and_state().await.map(|(r, _)| r)
    }

    /// The router and its state (to reach the database).
    async fn app_and_state() -> Option<(axum::Router, AppState)> {
        let url = std::env::var("OT_TEST_REDIS_URL").ok()?;
        let ns = format!("ot-auth-test-{}", chrono::Utc::now().timestamp_micros());
        let common = Common {
            sqlite: ":memory:".into(),
            redis: url.clone(),
            redis_ca: None,
            redis_cert: None,
            redis_key: None,
            redis_namespace: ns.clone(),
            site: ot_core::SiteCode::new("TST").unwrap(),
            nats: crate::config::NatsArgs {
                url: "nats://127.0.0.1:9".into(),
                creds: None,
                token: None,
                user: None,
                password: None,
                ca: None,
                cert: None,
                key: None,
                stream: "TRACKS".into(),
                tracks_subject: "tracks".into(),
                max_age_hours: 24.0,
            },
            profiles_dir: "profiles/trackers".into(),
            obs_window_secs: 600,
            shared_db: Default::default(),
        };
        let mut db = ot_store::Db::open_in_memory().unwrap();
        crate::auth::bootstrap_admin(&common, &mut db, Some("root@x.org"), Some(ROOT)).unwrap();
        let state = AppState {
            db: Arc::new(Mutex::new(db)),
            redis: ot_store::RedisStore::connect(&url, ot_store::Keys::new(ns))
                .await
                .unwrap(),
            nats: common.connect_nats().await.unwrap(),
            auth: Arc::new(Auth::new(
                vec![1; 32],
                false,
                false,
                None,
                AuthSettings::default(),
            )),
            common,
        };
        Some((crate::control::router(state.clone(), None), state))
    }

    const ROOT: &str = "Root-Pass-2026-xyz!";
    const VIEW: &str = "Viewer-Temp-2026!";
    const VIEW2: &str = "Seen-Anew-4-Viewing#";

    /// A request with optional cookie or bearer token; the status, the
    /// JSON body and any session cookie set.
    async fn call(
        app: &axum::Router,
        method: &str,
        uri: &str,
        auth: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value, Option<String>) {
        call_idle(app, method, uri, auth, body, None).await
    }

    /// As [`call`], with the browser saying its user has been idle.
    async fn call_idle(
        app: &axum::Router,
        method: &str,
        uri: &str,
        auth: Option<&str>,
        body: Option<Value>,
        idle_ms: Option<i64>,
    ) -> (StatusCode, Value, Option<String>) {
        // Each request from its own address, so the per-address limit on
        // sign-in attempts stays out of the way.
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let peer = std::net::SocketAddr::from(([10, 9, (n >> 8) as u8, n as u8], 40000));
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .extension(ConnectInfo(peer));
        if let Some(ms) = idle_ms {
            req = req.header(super::super::sessions::IDLE_HEADER, ms.to_string());
        }
        match auth {
            Some(a) if a.starts_with("Bearer ") => req = req.header("authorization", a),
            // As the page does, which sends the CSRF header with every call.
            Some(c) => {
                req = req
                    .header("cookie", c)
                    .header(super::super::csrf::HEADER, "1")
            }
            None => {}
        }
        // Clients cannot say who they are this way.
        req = req.header("x-opentrack-actor", "spoofed");
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = res.status();
        let cookie = res
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::to_owned);
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            cookie,
        )
    }

    #[tokio::test]
    async fn sign_in_roles_tokens_and_sign_out() {
        let Some(app) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        // Nothing but the sign-in page's calls without signing in.
        let (st, ..) = call(&app, "GET", "/api/v1/tracks", None, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (st, body, _) = call(&app, "GET", "/api/v1/auth/public", None, None).await;
        assert_eq!(
            (st, body["password_login"].as_bool()),
            (StatusCode::OK, Some(true))
        );

        let login = |e: &str, p: &str| json!({ "email": e, "password": p });
        let (st, ..) = call(
            &app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(login("root@x.org", "Nope-nope-nope-1!")),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (st, ..) = call(
            &app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(login("who@x.org", ROOT)),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (st, body, cookie) = call(
            &app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(login("ROOT@x.org", ROOT)),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["role"], "admin");
        // The failed attempt since the previous sign-in (none) is reported.
        assert_eq!(body["last_login"]["failed_attempts"], 1);
        assert_eq!(body["must_change_password"], false);
        let admin = cookie.expect("a session cookie");
        assert!(admin.starts_with("ot_session="));
        let (st, body, _) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
        assert_eq!(
            (st, body["email"].as_str()),
            (StatusCode::OK, Some("root@x.org"))
        );

        // A viewer reads but changes nothing; a track manager manages tracks.
        let (st, viewer, _) = call(
            &app,
            "POST",
            "/api/v1/auth/users",
            Some(&admin),
            Some(json!({ "email": "v@x.org", "role": "viewer", "password": VIEW })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{viewer}");
        // A weak password is refused, whoever sets it.
        let (st, body, _) = call(
            &app,
            "POST",
            "/api/v1/auth/users",
            Some(&admin),
            Some(json!({ "email": "w@x.org", "role": "viewer", "password": "viewpass1" })),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        let (_, body, vcookie) = call(
            &app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(login("v@x.org", VIEW)),
        )
        .await;
        let vcookie = vcookie.unwrap();
        // An admin's password is temporary: nothing else until it changes.
        assert_eq!(body["must_change_password"], true, "{body}");
        let (st, body, _) = call(&app, "GET", "/api/v1/tracks", Some(&vcookie), None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        assert_eq!(body["code"], "password_change_required");
        let change = |cur: &str, new: &str| json!({ "current": cur, "new": new });
        let (st, body, _) = call(
            &app,
            "POST",
            "/api/v1/auth/password",
            Some(&vcookie),
            Some(change(VIEW, VIEW)),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "the same one: {body}");
        let (st, body, vcookie) = call(
            &app,
            "POST",
            "/api/v1/auth/password",
            Some(&vcookie),
            Some(change(VIEW, VIEW2)),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["must_change_password"], false);
        let vcookie = vcookie.unwrap();
        let (st, ..) = call(&app, "GET", "/api/v1/tracks", Some(&vcookie), None).await;
        assert_eq!(st, StatusCode::OK);
        // Not again within a day.
        let (st, body, _) = call(
            &app,
            "POST",
            "/api/v1/auth/password",
            Some(&vcookie),
            Some(change(VIEW2, "Another-Fresh-One-77!")),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(body["error"].as_str().unwrap().contains("24"), "{body}");
        let (st, body, _) = call(
            &app,
            "POST",
            "/api/v1/tracks/delete",
            Some(&vcookie),
            Some(json!({})),
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
        let (st, ..) = call(&app, "GET", "/api/v1/auth/users", Some(&vcookie), None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);

        // The decision log names the account, not what the client claimed.
        let (st, ..) = call(
            &app,
            "POST",
            "/api/v1/auth/users",
            Some(&admin),
            Some(json!({ "email": "tm@x.org", "role": "track_manager" })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        let (_, body, _) = call(
            &app,
            "GET",
            "/api/v1/decisions?op=create_user",
            Some(&admin),
            None,
        )
        .await;
        let text = body.to_string();
        assert!(
            text.contains("root@x.org") && !text.contains("spoofed"),
            "{text}"
        );

        // An API token acts as its account until revoked.
        let (st, tok, _) = call(
            &app,
            "POST",
            "/api/v1/auth/api-tokens",
            Some(&admin),
            Some(json!({ "name": "script", "user_id": viewer["id"] })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{tok}");
        let bearer = format!("Bearer {}", tok["token"].as_str().unwrap());
        let (st, body, _) = call(&app, "GET", "/api/v1/auth/me", Some(&bearer), None).await;
        assert_eq!(
            (st, body["via"].as_str()),
            (StatusCode::OK, Some("api_token"))
        );
        // A session cookie is not an API token, nor the other way round.
        let (st, ..) = call(
            &app,
            "GET",
            "/api/v1/auth/me",
            Some(&format!("ot_session={}", tok["token"].as_str().unwrap())),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let uri = format!("/api/v1/auth/api-tokens/{}", tok["jti"].as_str().unwrap());
        let (st, ..) = call(&app, "DELETE", &uri, Some(&admin), None).await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        let (st, ..) = call(&app, "GET", "/api/v1/auth/me", Some(&bearer), None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);

        // Deactivating an account ends its sessions at once; the last
        // admin cannot be demoted.
        let uri = format!("/api/v1/auth/users/{}", viewer["id"].as_str().unwrap());
        let (st, ..) = call(
            &app,
            "PUT",
            &uri,
            Some(&admin),
            Some(json!({ "active": false })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (st, ..) = call(&app, "GET", "/api/v1/tracks", Some(&vcookie), None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let (_, me, _) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
        let uri = format!("/api/v1/auth/users/{}", me["id"].as_str().unwrap());
        let (st, ..) = call(
            &app,
            "PUT",
            &uri,
            Some(&admin),
            Some(json!({ "role": "viewer" })),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);

        // Signing out ends the session.
        let (st, _, cleared) = call(&app, "POST", "/api/v1/auth/logout", Some(&admin), None).await;
        assert_eq!(
            (st, cleared.as_deref()),
            (StatusCode::NO_CONTENT, Some("ot_session="))
        );
        let (st, ..) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn only_admins_see_a_sources_secrets() {
        let Some((app, state)) = app_and_state().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let spec = json!({
            "id": "feed", "name": "Feed",
            "transport": {
                "kind": "mqtt", "url": "mqtts://broker.example.org:8883",
                "topics": ["t"], "username": "ot", "password": "hunter2-secret"
            },
        });
        state
            .db
            .lock()
            .unwrap()
            .put_source(
                &ot_store::SourceWrite {
                    id: "feed",
                    name: "Feed",
                    transport: "mqtt",
                    codec: "json",
                    priority: 0,
                    spec: &spec,
                },
                "root@x.org",
            )
            .unwrap();
        let (st, admin) = sign_in(&app, "root@x.org", ROOT).await;
        assert_eq!(st, StatusCode::OK);
        let admin = admin.unwrap();
        let (st, viewer, _) = call(
            &app,
            "POST",
            "/api/v1/auth/users",
            Some(&admin),
            Some(json!({ "email": "v@x.org", "role": "viewer", "password": VIEW })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{viewer}");
        let (_, tok, _) = call(
            &app,
            "POST",
            "/api/v1/auth/api-tokens",
            Some(&admin),
            Some(json!({ "name": "monitor", "user_id": viewer["id"] })),
        )
        .await;
        let bearer = format!("Bearer {}", tok["token"].as_str().unwrap());
        let secret = |v: &Value| v.to_string().contains("hunter2-secret");
        for uri in [
            "/api/v1/sources",
            "/api/v1/sources/feed",
            "/api/v1/sources/feed/revisions",
        ] {
            let (st, body, _) = call(&app, "GET", uri, Some(&bearer), None).await;
            assert_eq!(st, StatusCode::OK, "{uri}: {body}");
            assert!(
                !secret(&body),
                "a viewer sees the password at {uri}: {body}"
            );
            assert!(
                body.to_string().contains(ot_source::secrets::HIDDEN),
                "{uri}"
            );
            let (_, body, _) = call(&app, "GET", uri, Some(&admin), None).await;
            assert!(secret(&body), "an admin sees it at {uri}");
        }
        // The decision log doesn't carry specifications at all.
        let (_, body, _) = call(
            &app,
            "GET",
            "/api/v1/decisions?op=create_source",
            Some(&bearer),
            None,
        )
        .await;
        assert!(
            body["decisions"].as_array().is_some_and(|d| !d.is_empty()),
            "{body}"
        );
        assert!(!secret(&body));
    }

    async fn sign_in(
        app: &axum::Router,
        email: &str,
        password: &str,
    ) -> (StatusCode, Option<String>) {
        let (st, _, c) = call(
            app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(json!({ "email": email, "password": password })),
        )
        .await;
        (st, c)
    }

    #[tokio::test]
    async fn with_an_admin_listener_the_main_one_does_not_serve_admin_routes() {
        let Some((admin_app, _)) = app_and_state().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        // As main.rs builds the main listener's router with OT_ADMIN_BIND.
        let main = admin_app
            .clone()
            .layer(Extension(crate::auth::AdminElsewhere));
        let (st, v, root) = call(
            &main,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(json!({ "email": "root@x.org", "password": ROOT })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["admin_api"], json!(false), "{v}");
        let root = root.unwrap();
        let (_, v, _) = call(&main, "GET", "/api/v1/auth/me", Some(&root), None).await;
        assert_eq!(v["admin_api"], json!(false), "{v}");
        // Admin reads and changes: not here, for an admin.
        for (method, uri, body) in [
            ("GET", "/api/v1/auth/users", None),
            ("GET", "/api/v1/audit", None),
            (
                "POST",
                "/api/v1/auth/users",
                Some(json!({ "email": "no@x.org", "role": "viewer", "password": VIEW })),
            ),
        ] {
            let (st, v, _) = call(&main, method, uri, Some(&root), body).await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{method} {uri}: {v}");
            assert!(
                v["error"].as_str().unwrap().contains("admin listener"),
                "{v}"
            );
        }
        // Reads, and the account's own things, still are.
        let (st, v, _) = call(&main, "GET", "/api/v1/sources", Some(&root), None).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        let (st, ..) = call(&main, "GET", "/api/v1/auth/sessions", Some(&root), None).await;
        assert_eq!(st, StatusCode::OK);
        // Unauthenticated and under-privileged callers are refused as ever.
        let (st, ..) = call(&main, "GET", "/api/v1/auth/users", None, None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);

        // The admin listener serves them, with the same session.
        let (st, v, _) = call(&admin_app, "GET", "/api/v1/auth/me", Some(&root), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["admin_api"], json!(true), "{v}");
        let (st, v, _) = call(
            &admin_app,
            "POST",
            "/api/v1/auth/users",
            Some(&root),
            Some(json!({ "email": "al@x.org", "role": "viewer", "password": VIEW })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{v}");
        let (st, _, _) = call(&admin_app, "GET", "/api/v1/auth/users", Some(&root), None).await;
        assert_eq!(st, StatusCode::OK);

        // A viewer on the main listener: a password change answers
        // admin_api false too, and an admin route is still a 403 for it.
        let (_, viewer) = sign_in(&main, "al@x.org", VIEW).await;
        let (st, v, _) = call(
            &main,
            "POST",
            "/api/v1/auth/password",
            viewer.as_deref(),
            Some(json!({ "current": VIEW, "new": VIEW2 })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["admin_api"], json!(false), "{v}");
        let (_, viewer) = sign_in(&main, "al@x.org", VIEW2).await;
        let (st, ..) = call(&main, "GET", "/api/v1/auth/users", viewer.as_deref(), None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, ..) = call(&main, "GET", "/api/v1/sources", viewer.as_deref(), None).await;
        assert_eq!(st, StatusCode::OK);
    }

    #[tokio::test]
    async fn the_warning_must_be_accepted_on_the_server_before_the_api_answers() {
        let Some(app) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (_, admin) = sign_in(&app, "root@x.org", ROOT).await;
        let admin = admin.unwrap();
        let (st, token, _) = call(
            &app,
            "POST",
            "/api/v1/auth/api-tokens",
            Some(&admin),
            Some(json!({ "name": "script" })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{token}");
        let bearer = format!("Bearer {}", token["token"].as_str().unwrap());
        let warning = json!({ "warning": { "enabled": true, "text": "You are accessing a U.S. Government information system." } });
        let (st, body, _) =
            call(&app, "PUT", "/api/v1/settings", Some(&admin), Some(warning)).await;
        assert_eq!(st, StatusCode::OK, "{body}");

        // The session that turned it on must accept it too; until then only
        // its own sign-in calls answer.
        let (st, body, _) = call(&app, "GET", "/api/v1/sources", Some(&admin), None).await;
        assert_eq!(
            (st, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("consent_required"))
        );
        let (st, me, _) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
        assert_eq!(
            (st, me["consent_required"].as_bool()),
            (StatusCode::OK, Some(true))
        );
        // A program's API token is not asked.
        let (st, ..) = call(&app, "GET", "/api/v1/sources", Some(&bearer), None).await;
        assert_eq!(st, StatusCode::OK);

        let (st, ..) = call(&app, "POST", "/api/v1/auth/consent", Some(&admin), None).await;
        assert_eq!(st, StatusCode::OK);
        let (st, ..) = call(&app, "GET", "/api/v1/sources", Some(&admin), None).await;
        assert_eq!(st, StatusCode::OK);
        let (_, me, _) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
        assert_eq!(me["consent_required"], false);
        let (_, audit, _) = call(
            &app,
            "GET",
            "/api/v1/audit?op=consent_accepted",
            Some(&admin),
            None,
        )
        .await;
        assert!(audit.to_string().contains("consent_accepted"), "{audit}");

        // Each sign-in is asked again.
        let (_, again) = sign_in(&app, "root@x.org", ROOT).await;
        let (st, ..) = call(&app, "GET", "/api/v1/sources", again.as_deref(), None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn viewers_list_track_decisions_but_not_account_ones() {
        let Some(app) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (_, admin) = sign_in(&app, "root@x.org", ROOT).await;
        let admin = admin.unwrap();
        let (st, v, _) = call(
            &app,
            "POST",
            "/api/v1/auth/users",
            Some(&admin),
            Some(json!({ "email": "dv@x.org", "role": "viewer", "password": VIEW })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{v}");
        let (_, viewer) = sign_in(&app, "dv@x.org", VIEW).await;
        let viewer = viewer.unwrap();
        let (st, ..) = call(
            &app,
            "POST",
            "/api/v1/auth/password",
            Some(&viewer),
            Some(json!({ "current": VIEW, "new": VIEW2 })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (_, viewer) = sign_in(&app, "dv@x.org", VIEW2).await;
        let viewer = viewer.unwrap();
        let (st, ..) = call(
            &app,
            "GET",
            "/api/v1/decisions?op=merge,split,update_source",
            Some(&viewer),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        for ops in [
            "create_user",
            "create_api_token",
            "merge,unlock_user",
            "auth_settings",
        ] {
            let uri = format!("/api/v1/decisions?op={ops}");
            let (st, ..) = call(&app, "GET", &uri, Some(&viewer), None).await;
            assert_eq!(st, StatusCode::FORBIDDEN, "{ops}");
        }
        let (st, body, _) = call(
            &app,
            "GET",
            "/api/v1/decisions?op=create_user",
            Some(&admin),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
    }

    #[tokio::test]
    async fn refusals_security_reads_token_use_and_decision_addresses_are_audited() {
        let Some(app) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (_, admin) = sign_in(&app, "root@x.org", ROOT).await;
        let admin = admin.unwrap();
        let ops = |body: &Value| -> Vec<(String, String)> {
            body["rows"]
                .as_array()
                .or_else(|| body["records"].as_array())
                .or_else(|| body.as_array())
                .map(|rows| {
                    rows.iter()
                        .map(|r| {
                            (
                                r["op"].as_str().unwrap_or("").to_owned(),
                                r["outcome"].to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        // A viewer reaching for an admin's page.
        let (st, v, _) = call(
            &app,
            "POST",
            "/api/v1/auth/users",
            Some(&admin),
            Some(json!({ "email": "rv@x.org", "role": "viewer", "password": VIEW })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{v}");
        let (_, viewer) = sign_in(&app, "rv@x.org", VIEW).await;
        let viewer = viewer.unwrap();
        call(
            &app,
            "POST",
            "/api/v1/auth/password",
            Some(&viewer),
            Some(json!({ "current": VIEW, "new": VIEW2 })),
        )
        .await;
        let (_, viewer) = sign_in(&app, "rv@x.org", VIEW2).await;
        let viewer = viewer.unwrap();
        let (st, ..) = call(&app, "GET", "/api/v1/auth/users", Some(&viewer), None).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // A credential that identifies no one.
        let (st, ..) = call(
            &app,
            "GET",
            "/api/v1/sources",
            Some("Bearer not-a-token"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        // A refused change: the same account twice.
        let (st, ..) = call(
            &app,
            "POST",
            "/api/v1/auth/users",
            Some(&admin),
            Some(json!({ "email": "rv@x.org", "role": "viewer", "password": VIEW })),
        )
        .await;
        assert!(st.is_client_error(), "{st}");
        // An API token's use, audited once.
        let (_, tok, _) = call(
            &app,
            "POST",
            "/api/v1/auth/api-tokens",
            Some(&admin),
            Some(json!({ "name": "script" })),
        )
        .await;
        let bearer = format!("Bearer {}", tok["token"].as_str().unwrap());
        for _ in 0..3 {
            let (st, ..) = call(&app, "GET", "/api/v1/sources", Some(&bearer), None).await;
            assert_eq!(st, StatusCode::OK);
        }
        // An admin reading the accounts.
        let (st, ..) = call(&app, "GET", "/api/v1/auth/users", Some(&admin), None).await;
        assert_eq!(st, StatusCode::OK);
        let (st, body, _) = call(&app, "GET", "/api/v1/audit?limit=200", Some(&admin), None).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let got = ops(&body);
        let has = |op: &str, outcome: &str| {
            got.iter()
                .filter(|(o, out)| o == op && out.contains(outcome))
                .count()
        };
        assert_eq!(has("access_denied", "failure"), 1, "{got:?}");
        assert_eq!(has("access_refused", "failure"), 1, "{got:?}");
        assert!(has("change_refused", "failure") >= 1, "{got:?}");
        assert!(has("read_security_object", "success") >= 1, "{got:?}");
        let token_uses = body.to_string().matches("api_token").count();
        assert!(token_uses >= 1, "{body}");
        assert_eq!(
            got.iter().filter(|(o, _)| o == "login").count(),
            // Three password sign-ins, and the token's first use.
            4,
            "{got:?}"
        );
        // A decision's audit copy carries the client address.
        let by_admin = body["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["op"] == "create_user" && r["actor"] == "root@x.org")
            .unwrap();
        assert!(
            by_admin["ip"].as_str().is_some_and(|ip| !ip.is_empty()),
            "{by_admin}"
        );
    }

    #[tokio::test]
    async fn a_temporary_account_expires_72_hours_after_it_is_made() {
        let Some(app) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (_, admin) = sign_in(&app, "root@x.org", ROOT).await;
        let admin = admin.unwrap();
        let before = now_ms();
        let (st, u, _) = call(&app, "POST", "/api/v1/auth/users", Some(&admin),
            Some(json!({ "email": "tmp@x.org", "role": "viewer", "password": VIEW, "temporary": true }))).await;
        assert_eq!(st, StatusCode::CREATED, "{u}");
        let exp = u["expires_at_ms"].as_i64().unwrap();
        let hours = stig::TEMPORARY_ACCOUNT_HOURS * 3_600_000;
        assert!(exp >= before + hours && exp <= now_ms() + hours, "{u}");
        // Made permanent.
        let uri = format!("/api/v1/auth/users/{}", u["id"].as_str().unwrap());
        let (st, u, _) = call(
            &app,
            "PUT",
            &uri,
            Some(&admin),
            Some(json!({ "temporary": false })),
        )
        .await;
        assert_eq!(
            (st, u["expires_at_ms"].is_null()),
            (StatusCode::OK, true),
            "{u}"
        );
    }

    #[tokio::test]
    async fn lockout_sessions_idle_timeout_and_the_audit_record() {
        let Some(app) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (st, admin) = sign_in(&app, "root@x.org", ROOT).await;
        assert_eq!(st, StatusCode::OK);
        let admin = admin.unwrap();
        // Sessions ride a strict same-site cookie.
        let raw = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "email": "root@x.org", "password": ROOT }).to_string(),
            ))
            .unwrap();
        let res = app.clone().oneshot(raw).await.unwrap();
        let set = res.headers()["set-cookie"].to_str().unwrap().to_owned();
        assert!(
            set.contains("SameSite=Strict") && set.contains("HttpOnly"),
            "{set}"
        );
        assert_eq!(res.headers()["cache-control"], "no-store");
        assert!(res.headers().contains_key("content-security-policy"));

        // Three wrong passwords lock the account; the right one then fails
        // too, with the same answer; an admin unlocks it.
        let (st, v, _) = call(
            &app,
            "POST",
            "/api/v1/auth/users",
            Some(&admin),
            Some(json!({ "email": "l@x.org", "role": "viewer", "password": VIEW })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        for _ in 0..3 {
            let (st, _) = sign_in(&app, "l@x.org", "Wrong-Wrong-Wrong-1").await;
            assert_eq!(st, StatusCode::UNAUTHORIZED);
        }
        let (st, _, _) = call(
            &app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(json!({ "email": "l@x.org", "password": VIEW })),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "locked");
        let (_, users, _) = call(&app, "GET", "/api/v1/auth/users", Some(&admin), None).await;
        let locked = users["users"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["email"] == "l@x.org")
            .unwrap();
        // Locked until an admin unlocks it, not for a while (CP SRG V-233165).
        assert_eq!(
            locked["locked_until_ms"].as_i64(),
            Some(ot_store::auth::LOCKED_UNTIL_UNLOCKED)
        );
        let uri = format!("/api/v1/auth/users/{}/unlock", v["id"].as_str().unwrap());
        let (st, ..) = call(&app, "POST", &uri, Some(&admin), None).await;
        assert_eq!(st, StatusCode::OK);
        let (st, lcookie) = sign_in(&app, "l@x.org", VIEW).await;
        assert_eq!(st, StatusCode::OK);
        let (_, me, _) = call(&app, "GET", "/api/v1/auth/me", lcookie.as_deref(), None).await;
        assert_eq!(me["last_login"]["failed_attempts"], 4, "{me}");
        let (st, _, lcookie) = call(
            &app,
            "POST",
            "/api/v1/auth/password",
            lcookie.as_deref(),
            Some(json!({ "current": VIEW, "new": VIEW2 })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);

        // At most three sessions: a fourth sign-in ends the oldest.
        let mut cookies = Vec::new();
        for _ in 0..3 {
            cookies.push(sign_in(&app, "root@x.org", ROOT).await.1.unwrap());
        }
        let (st, ..) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "pushed out by newer sessions");
        let admin = cookies.pop().unwrap();
        let (_, mine, _) = call(&app, "GET", "/api/v1/auth/sessions", Some(&admin), None).await;
        let live = mine["sessions"].as_array().unwrap();
        assert_eq!(live.len(), 3, "{mine}");
        // Ending one of your own sessions.
        let other = live.iter().find(|x| x["id"] != mine["current"]).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let (st, ..) = call(
            &app,
            "DELETE",
            &format!("/api/v1/auth/sessions/{other}"),
            Some(&admin),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
        // A viewer sees only its own, and cannot end an admin's.
        let (st, ..) = call(
            &app,
            "GET",
            "/api/v1/auth/sessions?all=true",
            lcookie.as_deref(),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, ..) = call(
            &app,
            "DELETE",
            &format!(
                "/api/v1/auth/sessions/{}",
                mine["current"].as_str().unwrap()
            ),
            lcookie.as_deref(),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NOT_FOUND);

        // A browser idle past the timeout (an admin's: 10 minutes) is
        // signed out, even while its page polls.
        let (st, ..) = call_idle(
            &app,
            "GET",
            "/api/v1/status",
            Some(&admin),
            None,
            Some(11 * 60_000),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::SERVICE_UNAVAILABLE,
            "idle, but not for longer than the last use (and no NATS here)"
        );
        let (st, ..) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
        assert_eq!(st, StatusCode::OK);

        // The audit record has the story, and its chain holds.
        let (st, audit, _) = call(
            &app,
            "GET",
            "/api/v1/audit?op=login,account_locked,unlock_user,session_end&limit=500",
            Some(&admin),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{audit}");
        let rows = audit["rows"].as_array().unwrap();
        let count = |op: &str, outcome: &str| {
            rows.iter()
                .filter(|r| r["op"] == op && r["outcome"] == outcome)
                .count()
        };
        assert_eq!(count("login", "failure"), 4, "three wrong, one locked");
        assert_eq!(count("account_locked", "success"), 1);
        assert_eq!(count("unlock_user", "success"), 1);
        assert!(count("session_end", "success") >= 2, "pushed out, ended");
        let (st, v, _) = call(&app, "GET", "/api/v1/audit/verify", Some(&admin), None).await;
        assert_eq!((st, v["ok"].as_bool()), (StatusCode::OK, Some(true)), "{v}");
        let (st, ..) = call(&app, "GET", "/api/v1/audit", lcookie.as_deref(), None).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "admins only");
    }

    #[tokio::test]
    async fn an_idle_session_ends() {
        let Some((app, state)) = app_and_state().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (_, admin) = sign_in(&app, "root@x.org", ROOT).await;
        let admin = admin.unwrap();
        let (st, me, _) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
        assert_eq!(st, StatusCode::OK);
        let session = me["session"].as_str().unwrap().to_owned();
        assert_eq!(me["idle_timeout_ms"], 10 * 60_000, "an admin's");
        // Its user walks away: the last use was 11 minutes ago, and the
        // page's polling since says so.
        let at = ot_store::sqlite::now_ms() - 11 * 60_000;
        let id = session.clone();
        state
            .with_db(move |db| {
                db.connection()
                    .execute(
                        "UPDATE sessions SET last_seen_ms = ?2 WHERE id = ?1",
                        (id.as_str(), at),
                    )
                    .map_err(ot_store::StoreError::from)?;
                Ok(())
            })
            .await
            .unwrap();
        state.auth.activity.forget(&session);
        let (st, ..) = call_idle(
            &app,
            "GET",
            "/api/v1/status",
            Some(&admin),
            None,
            Some(11 * 60_000),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        let id = session.clone();
        let row = state
            .with_db(move |db| db.session(&id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.end_reason.as_deref(), Some("idle"));
        let timeouts = state
            .with_db(|db| {
                db.audit_rows(&ot_store::AuditFilter {
                    ops: vec!["session_timeout".into()],
                    limit: 10,
                    ..Default::default()
                })
            })
            .await
            .unwrap();
        assert_eq!(timeouts.len(), 1);
        assert_eq!(timeouts[0].actor, "root@x.org");
    }

    #[tokio::test]
    async fn a_loosened_policy_saved_before_it_was_fixed_does_not_apply() {
        let Some((app, state)) = app_and_state().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        // 0.4.0 (alpha) saved these; the node loads them as it starts.
        let old = json!({
            "session_hours": 720.0,
            "password": {"min_length": 8, "require_upper": false, "require_lower": false,
                         "require_digit": false, "require_special": false, "history": 0},
            "lockout": {"max_failures": 0, "window_minutes": 1.0, "lock_minutes": 0.0},
            "sessions": {"idle_minutes": 1440.0, "admin_idle_minutes": null, "max_per_account": 0},
            "inactivity": {"disable_after_days": 0.0, "exempt": []},
            "audit": {"retention_days": 7.0},
        });
        let saved = old.clone();
        state
            .with_db(move |db| db.put_auth_settings(&saved))
            .await
            .unwrap();
        let stored = state.with_db(|db| db.auth_settings()).await.unwrap();
        let loaded: AuthSettings = serde_json::from_value(stored).unwrap();
        state.auth.set_settings(loaded);

        let (_, admin) = sign_in(&app, "root@x.org", ROOT).await;
        let admin = admin.unwrap();
        let (_, me, _) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
        assert_eq!(me["idle_timeout_ms"], 10 * 60_000);
        assert_eq!(me["password_policy"]["min_length"], 15);
        // A 10-character password is refused.
        let (st, body, _) = call(
            &app,
            "POST",
            "/api/v1/auth/users",
            Some(&admin),
            Some(json!({ "email": "w@x.org", "role": "viewer", "password": "abcdefghij" })),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        let (st, _, _) = call(
            &app,
            "POST",
            "/api/v1/auth/users",
            Some(&admin),
            Some(json!({ "email": "w@x.org", "role": "viewer", "password": VIEW })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        // Three failures lock it.
        for _ in 0..3 {
            let (st, _) = sign_in(&app, "w@x.org", "Wrong-Wrong-Wrong-1").await;
            assert_eq!(st, StatusCode::UNAUTHORIZED);
        }
        let (st, _) = sign_in(&app, "w@x.org", VIEW).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "locked");
        // Saving the old document again is accepted; the retired settings
        // are neither kept nor shown.
        let (st, body, _) = call(
            &app,
            "PUT",
            "/api/v1/auth/settings",
            Some(&admin),
            Some(old),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let stored = state.with_db(|db| db.auth_settings()).await.unwrap();
        for k in ["session_hours", "password", "lockout", "sessions", "audit"] {
            assert!(body.get(k).is_none() && stored.get(k).is_none(), "{k}");
        }
        assert_eq!(stored["inactivity"], json!({"exempt": []}));
    }

    /// An identity provider's metadata (the certificate is not checked
    /// until a sign-on).
    #[cfg(feature = "saml")]
    const IDP_METADATA: &str = r#"<md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" entityID="https://idp.example.org">
  <md:IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <md:KeyDescriptor use="signing"><ds:KeyInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#"><ds:X509Data><ds:X509Certificate>MIIBexample</ds:X509Certificate></ds:X509Data></ds:KeyInfo></md:KeyDescriptor>
    <md:SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect" Location="https://idp.example.org/sso"/>
  </md:IDPSSODescriptor>
</md:EntityDescriptor>"#;

    /// A `saml` account of `role`, and a session it began by single sign-on.
    #[cfg(feature = "saml")]
    async fn saml_session(state: &AppState, email: &str, role: &str) -> (String, String) {
        let (e, r) = (email.to_owned(), role.to_owned());
        let user = state
            .with_db(move |db| {
                db.create_user(&ot_store::NewUser {
                    id: &crate::fips::uuid_v4(),
                    email: &e,
                    name: "",
                    role: &r,
                    password_hash: None,
                    origin: "saml",
                })
            })
            .await
            .unwrap();
        let id = user.id.clone();
        let (headers, _) = start_session(state, user, &Client::default(), How::Saml, true)
            .await
            .unwrap();
        let cookie = headers[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        (id, cookie)
    }

    #[cfg(feature = "saml")]
    #[tokio::test]
    async fn a_saml_admin_turns_password_sign_in_off() {
        let Some((app, state)) = app_and_state().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (_, root) = sign_in(&app, "root@x.org", ROOT).await;
        let root = root.unwrap();
        let (st, mut settings, _) =
            call(&app, "GET", "/api/v1/auth/settings", Some(&root), None).await;
        assert_eq!(st, StatusCode::OK);
        settings["saml"]["enabled"] = json!(true);
        settings["saml"]["idp_metadata_xml"] = json!(IDP_METADATA);
        settings["disable_password_login"] = json!(true);
        // From a password session: refused, SSO is not proven.
        let (st, body, _) = call(
            &app,
            "PUT",
            "/api/v1/auth/settings",
            Some(&root),
            Some(settings.clone()),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("sign in with single sign-on")
        );
        // From a SAML session: done.
        let (_, sso) = saml_session(&state, "sso-admin@x.org", "admin").await;
        let (st, me, _) = call(&app, "GET", "/api/v1/auth/me", Some(&sso), None).await;
        assert_eq!((st, me["via"].as_str()), (StatusCode::OK, Some("session")));
        let (st, body, _) = call(
            &app,
            "PUT",
            "/api/v1/auth/settings",
            Some(&sso),
            Some(settings.clone()),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["disable_password_login"], true);
        let (st, ..) = sign_in(&app, "root@x.org", ROOT).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "password sign-in is off");
        // But not while turning SAML itself off (OpenStare on, so the
        // settings alone would pass).
        settings["saml"]["enabled"] = json!(false);
        settings["openstare"]["enabled"] = json!(true);
        let (st, body, _) = call(
            &app,
            "PUT",
            "/api/v1/auth/settings",
            Some(&sso),
            Some(settings),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
        assert!(body["error"].as_str().unwrap().contains("keep SAML on"));
    }

    #[cfg(feature = "saml")]
    #[tokio::test]
    async fn the_identity_provider_owns_a_saml_accounts_role() {
        let Some((app, state)) = app_and_state().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (_, root) = sign_in(&app, "root@x.org", ROOT).await;
        let root = root.unwrap();
        let (id, _) = saml_session(&state, "sso-viewer@x.org", "viewer").await;
        let uri = format!("/api/v1/auth/users/{id}");
        let (st, body, _) = call(
            &app,
            "PUT",
            &uri,
            Some(&root),
            Some(json!({ "role": "admin" })),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("identity provider"),
            "{body}"
        );
        // Its name, its state, and its role unchanged are fine.
        let (st, body, _) = call(
            &app,
            "PUT",
            &uri,
            Some(&root),
            Some(json!({ "name": "Vee", "role": "viewer", "active": true })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(
            (body["name"].as_str(), body["role"].as_str()),
            (Some("Vee"), Some("viewer"))
        );
    }

    #[cfg(feature = "saml")]
    #[tokio::test]
    async fn the_identity_providers_fields_come_from_its_metadata() {
        let Some((app, state)) = app_and_state().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (_, root) = sign_in(&app, "root@x.org", ROOT).await;
        let root = root.unwrap();
        let (_, mut settings, _) =
            call(&app, "GET", "/api/v1/auth/settings", Some(&root), None).await;
        settings["saml"]["enabled"] = json!(true);
        settings["saml"]["idp_metadata_xml"] = json!(IDP_METADATA);
        // Edited fields are not what signs on: they are replaced.
        settings["saml"]["idp_entity_id"] = json!("https://elsewhere.example.org");
        settings["saml"]["sso_url"] = json!("");
        let (st, body, _) = call(
            &app,
            "PUT",
            "/api/v1/auth/settings",
            Some(&root),
            Some(settings.clone()),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        let saml = &body["saml"];
        assert_eq!(saml["idp_entity_id"], "https://idp.example.org");
        assert_eq!(saml["sso_url"], "https://idp.example.org/sso");
        assert_eq!(saml["signing_cert"], "MIIBexample");
        let stored = state.with_db(|db| db.auth_settings()).await.unwrap();
        assert_eq!(stored["saml"]["idp_entity_id"], "https://idp.example.org");
        // Settings saved before this, with fields that differ, show the
        // metadata's.
        let mut old: AuthSettings = serde_json::from_value(stored).unwrap();
        old.saml.idp_entity_id = "https://stale.example.org".into();
        state.auth.set_settings(old);
        let (_, body, _) = call(&app, "GET", "/api/v1/auth/settings", Some(&root), None).await;
        assert_eq!(body["saml"]["idp_entity_id"], "https://idp.example.org");
        // Metadata that cannot be read is refused while SAML is on.
        settings["saml"]["idp_metadata_xml"] = json!("<nope/>");
        let (st, body, _) = call(
            &app,
            "PUT",
            "/api/v1/auth/settings",
            Some(&root),
            Some(settings.clone()),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert!(
            body["error"].as_str().unwrap().starts_with("saml: "),
            "{body}"
        );
        // Off, it is kept, and the three are empty.
        settings["saml"]["enabled"] = json!(false);
        let (st, body, _) = call(
            &app,
            "PUT",
            "/api/v1/auth/settings",
            Some(&root),
            Some(settings),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["saml"]["idp_entity_id"], "");
    }

    /// A stand-in tile server on 127.0.0.1: `/t/{z}/{x}/{y}` answers by
    /// zoom (5: HTML, 6: 404, 7: 500, 8: too big, 9: a redirect), else a
    /// "PNG" naming what it was asked for.
    async fn tile_server() -> std::net::SocketAddr {
        use axum::extract::RawQuery;
        async fn t(
            Path((z, x, y)): Path<(u32, String, String)>,
            RawQuery(q): RawQuery,
            headers: HeaderMap,
        ) -> Response {
            let png = [(axum::http::header::CONTENT_TYPE, "image/png")];
            match z {
                5 => ([(axum::http::header::CONTENT_TYPE, "text/html")], "<html>").into_response(),
                6 => StatusCode::NOT_FOUND.into_response(),
                7 => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                8 => (png, vec![0u8; 4 * 1024 * 1024 + 1]).into_response(),
                9 => (
                    StatusCode::FOUND,
                    [(axum::http::header::LOCATION, "/t/1/0/0.png")],
                )
                    .into_response(),
                _ => {
                    let ua = headers
                        .get("user-agent")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned();
                    (
                        png,
                        format!("tile {z}/{x}/{y}?{} {ua}", q.unwrap_or_default()),
                    )
                        .into_response()
                }
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/t/{z}/{x}/{y}", axum::routing::get(t));
        tokio::spawn(async move { axum::serve(listener, app).await.ok() });
        addr
    }

    /// A GET with its status, headers and raw body.
    async fn get_raw(
        app: &axum::Router,
        uri: &str,
        auth: Option<&str>,
    ) -> (StatusCode, HeaderMap, bytes::Bytes) {
        let mut req =
            Request::builder()
                .uri(uri)
                .extension(ConnectInfo(std::net::SocketAddr::from((
                    [10, 8, 0, 1],
                    40000,
                ))));
        match auth {
            Some(a) if a.starts_with("Bearer ") => req = req.header("authorization", a),
            Some(c) => req = req.header("cookie", c),
            None => {}
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (status, headers) = (res.status(), res.headers().clone());
        let body = res.into_body().collect().await.unwrap().to_bytes();
        (status, headers, body)
    }

    #[tokio::test]
    async fn basemap_tiles_for_every_role_and_the_url_for_admins_only() {
        let Some(app) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (st, admin) = sign_in(&app, "root@x.org", ROOT).await;
        assert_eq!(st, StatusCode::OK);
        let admin = admin.unwrap();
        let (st, viewer, _) = call(
            &app,
            "POST",
            "/api/v1/auth/users",
            Some(&admin),
            Some(json!({ "email": "v@x.org", "role": "viewer", "password": VIEW })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{viewer}");
        let (st, tok, _) = call(
            &app,
            "POST",
            "/api/v1/auth/api-tokens",
            Some(&admin),
            Some(json!({ "name": "map", "user_id": viewer["id"] })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{tok}");
        let viewer = format!("Bearer {}", tok["token"].as_str().unwrap());

        // Off: no tiles, and the settings say so.
        let (st, ..) = get_raw(&app, "/api/v1/basemap/0/0/0", Some(&viewer)).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
        let (_, body, _) = call(&app, "GET", "/api/v1/settings", Some(&viewer), None).await;
        assert_eq!(body["basemap_tiles"], false, "{body}");

        // Checked on save.
        let (st, body, _) = call(
            &app,
            "PUT",
            "/api/v1/settings",
            Some(&admin),
            Some(json!({ "basemap_tiles_url": "https://{s}.t.example/{z}/{x}/{y}.png" })),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body["error"].as_str().unwrap().contains("{s}"), "{body}");

        let addr = tile_server().await;
        let url = format!("http://{addr}/t/{{z}}/{{x}}/{{y}}.png?key=sekrit");
        let (st, body, _) = call(
            &app,
            "PUT",
            "/api/v1/settings",
            Some(&admin),
            Some(json!({ "site_name": "Garden Island", "basemap_tiles_url": url })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["settings"]["basemap_tiles_url"], url.as_str());
        // An admin reads the URL; a viewer only that tiles are on.
        let (_, body, _) = call(&app, "GET", "/api/v1/settings", Some(&admin), None).await;
        assert_eq!(
            (
                body["settings"]["basemap_tiles_url"].as_str(),
                body["basemap_tiles"].as_bool()
            ),
            (Some(url.as_str()), Some(true))
        );
        let (_, body, _) = call(&app, "GET", "/api/v1/settings", Some(&viewer), None).await;
        assert_eq!(
            (
                body["settings"]["basemap_tiles_url"].as_str(),
                body["basemap_tiles"].as_bool()
            ),
            (Some(""), Some(true)),
            "{body}"
        );
        assert_eq!(body["settings"]["site_name"], "Garden Island");
        // Nor through the decision log, which viewers read.
        let (st, body, _) = call(
            &app,
            "GET",
            "/api/v1/decisions?op=app_settings",
            Some(&viewer),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(!body.to_string().contains("sekrit"), "{body}");
        // The configuration export, an admin's, keeps it.
        let (st, _, file) = get_raw(&app, "/api/v1/export/config", Some(&admin)).await;
        assert_eq!(st, StatusCode::OK);
        assert!(String::from_utf8_lossy(&file).contains("key=sekrit"));

        // A viewer's tile, fetched with the coordinates filled in.
        let (st, h, body) = get_raw(&app, "/api/v1/basemap/3/2/1", Some(&viewer)).await;
        assert_eq!(st, StatusCode::OK, "{body:?}");
        let body = String::from_utf8_lossy(&body);
        assert!(
            body.starts_with("tile 3/2/1.png?key=sekrit OpenTrack/"),
            "{body}"
        );
        assert_eq!(h["content-type"], "image/png");
        assert_eq!(h["cache-control"], "private, max-age=86400");
        assert_eq!(h["x-content-type-options"], "nosniff");
        assert!(!h.contains_key("pragma"));
        // Signed in only.
        let (st, ..) = get_raw(&app, "/api/v1/basemap/3/2/1", None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        // Coordinates in bounds.
        for bad in ["25/0/0", "2/4/0", "2/0/4", "1/a/0"] {
            let (st, h, _) = get_raw(&app, &format!("/api/v1/basemap/{bad}"), Some(&viewer)).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}");
            assert_eq!(h["cache-control"], "no-store", "an error is not kept");
        }
        // The tile server's answers: 404 passes through; anything but an
        // image, an error, a tile over 4 MiB or a redirect is a 502.
        for (z, want) in [
            (5, StatusCode::BAD_GATEWAY),
            (6, StatusCode::NOT_FOUND),
            (7, StatusCode::BAD_GATEWAY),
            (8, StatusCode::BAD_GATEWAY),
            (9, StatusCode::BAD_GATEWAY),
        ] {
            let (st, _, body) =
                get_raw(&app, &format!("/api/v1/basemap/{z}/0/0"), Some(&viewer)).await;
            assert_eq!(st, want, "zoom {z}: {body:?}");
            assert!(!String::from_utf8_lossy(&body).contains("sekrit"));
        }

        // Off again: 404.
        let (st, _, _) = call(
            &app,
            "PUT",
            "/api/v1/settings",
            Some(&admin),
            Some(json!({ "basemap_tiles_url": "" })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let (st, ..) = get_raw(&app, "/api/v1/basemap/3/2/1", Some(&viewer)).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    /// A request with exactly these headers, from one address.
    async fn send(
        app: &axum::Router,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .extension(ConnectInfo(std::net::SocketAddr::from((
                [10, 7, 0, 1],
                40000,
            ))));
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                Body::from(b.to_string())
            }
            None => Body::empty(),
        };
        let res = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let (status, headers) = (res.status(), res.headers().clone());
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            headers,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn audit_ops(body: &Value) -> Vec<(String, String)> {
        body["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["op"].as_str().unwrap_or("").to_owned(),
                    format!("{} {}", r["outcome"], r["detail"]),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn a_cookie_borne_change_must_come_from_the_page() {
        let Some(app) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (st, _, cookie) = call(
            &app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(json!({ "email": "root@x.org", "password": ROOT })),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let admin = cookie.unwrap();
        let (_, tok, _) = call(
            &app,
            "POST",
            "/api/v1/auth/api-tokens",
            Some(&admin),
            Some(json!({ "name": "script" })),
        )
        .await;
        let bearer = format!("Bearer {}", tok["token"].as_str().unwrap());
        let host = ("host", "ot.test:8090");
        let csrf = (super::super::csrf::HEADER, "1");
        let change = Some(json!({ "name": "another" }));
        let path = "/api/v1/auth/api-tokens";

        // The session cookie alone: another site's form could send that.
        let c = [("cookie", admin.as_str()), host];
        let (st, _, body) = send(&app, "POST", path, &c, change.clone()).await;
        assert_eq!(
            (st, body["code"].as_str()),
            (StatusCode::FORBIDDEN, Some("csrf"))
        );
        // With the header but from another site.
        let c = [
            ("cookie", admin.as_str()),
            host,
            csrf,
            ("origin", "http://evil.test"),
        ];
        let (st, ..) = send(&app, "POST", path, &c, change.clone()).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // The page: its own origin and the header.
        let c = [
            ("cookie", admin.as_str()),
            host,
            csrf,
            ("origin", "http://ot.test:8090"),
        ];
        let (st, _, body) = send(&app, "POST", path, &c, change.clone()).await;
        assert_eq!(st, StatusCode::CREATED, "{body}");
        // Reads need neither.
        let c = [
            ("cookie", admin.as_str()),
            host,
            ("origin", "http://evil.test"),
        ];
        let (st, ..) = send(&app, "GET", "/api/v1/auth/me", &c, None).await;
        assert_eq!(st, StatusCode::OK);
        // A script's token needs no header, but not from another site's page.
        let c = [("authorization", bearer.as_str()), host];
        let (st, ..) = send(&app, "POST", path, &c, change.clone()).await;
        assert_eq!(st, StatusCode::CREATED);
        let c = [
            ("authorization", bearer.as_str()),
            host,
            ("origin", "http://evil.test"),
        ];
        let (st, ..) = send(&app, "POST", path, &c, change.clone()).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // Nor sign in from another site's page (login CSRF).
        let c = [host, ("origin", "http://evil.test")];
        let login = Some(json!({ "email": "root@x.org", "password": ROOT }));
        let (st, ..) = send(&app, "POST", "/api/v1/auth/login", &c, login.clone()).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // The sign-in page itself sends its origin.
        let c = [host, ("origin", "http://ot.test:8090")];
        let (st, ..) = send(&app, "POST", "/api/v1/auth/login", &c, login).await;
        assert_eq!(st, StatusCode::OK);

        let (_, body, _) = call(&app, "GET", "/api/v1/audit?limit=200", Some(&admin), None).await;
        let csrf_denials = audit_ops(&body)
            .into_iter()
            .filter(|(op, d)| op == "access_denied" && d.contains("failure") && d.contains("csrf"))
            .count();
        assert_eq!(csrf_denials, 4, "{body}");
    }

    #[tokio::test]
    async fn a_flood_is_answered_429_and_audited_once() {
        let Some(app) = app().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        let (_, _, cookie) = call(
            &app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(json!({ "email": "root@x.org", "password": ROOT })),
        )
        .await;
        let admin = cookie.unwrap();
        let (_, tok, _) = call(
            &app,
            "POST",
            "/api/v1/auth/api-tokens",
            Some(&admin),
            Some(json!({ "name": "flood" })),
        )
        .await;
        let bearer = format!("Bearer {}", tok["token"].as_str().unwrap());
        let c = [("authorization", bearer.as_str())];
        // The burst, and whatever the rate refills while it is sent.
        let mut allowed = 0;
        let refused = loop {
            let (st, h, _) = send(&app, "GET", "/api/v1/auth/me", &c, None).await;
            if st == StatusCode::TOO_MANY_REQUESTS {
                break h;
            }
            assert_eq!(st, StatusCode::OK);
            allowed += 1;
            assert!(allowed < 1_000, "never limited");
        };
        assert!(allowed >= super::super::rate::BURST as usize, "{allowed}");
        assert_eq!(refused["retry-after"], "1");
        for _ in 0..5 {
            let (st, ..) = send(&app, "GET", "/api/v1/auth/me", &c, None).await;
            assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
        }
        // Each token is a client of its own: the account's session goes on.
        let (st, body, _) = call(&app, "GET", "/api/v1/audit?limit=200", Some(&admin), None).await;
        assert_eq!(st, StatusCode::OK);
        let limited = audit_ops(&body)
            .into_iter()
            .filter(|(op, d)| op == "rate_limited" && d.contains("failure"))
            .count();
        assert_eq!(limited, 1, "{body}");
    }
}
