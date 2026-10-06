//! The role each API path needs, in one table: reading needs a viewer (the
//! maps' basemap tiles, `/basemap/{z}/{x}/{y}`, too),
//! managing tracks a track manager, and every other change an admin (a
//! new route that writes is an admin's until it is listed here).

use axum::http::Method;

use super::Role;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Anyone, signed in or not (the sign-in page's own calls).
    Public,
    /// Any signed-in account (its own profile and password).
    SignedIn,
    Role(Role),
}

/// Paths anyone may call.
const PUBLIC: &[&str] = &[
    "/auth/login",
    "/auth/logout",
    "/auth/public",
    "/auth/saml/login",
    "/auth/saml/acs",
    "/auth/saml/metadata",
    "/public/banner",
    "/public/warning-banner",
];

/// Paths any signed-in account may call, whatever its role.
const SIGNED_IN: &[&str] = &[
    "/auth/me",
    "/auth/password",
    "/auth/sessions",
    "/auth/consent",
];

/// Paths under which any signed-in account may call (its own sessions; the
/// handler lets only an admin touch another account's).
const SIGNED_IN_UNDER: &[&str] = &["/auth/sessions/"];

/// Reads that are an admin's: they show secrets (source credentials),
/// accounts or the audit record, or what a configuration import would
/// replace.
const ADMIN_READS: &[&str] = &[
    "/auth/",
    "/export/config",
    "/import/config",
    "/probe",
    "/audit",
];

/// What a person who hasn't accepted the notice-and-consent banner may
/// call: who they are, accepting it, and signing out.
const BEFORE_CONSENT: &[&str] = &["/auth/me", "/auth/consent", "/auth/logout", "/auth/public"];

/// Whether `path` answers before the banner is accepted.
pub fn allowed_before_consent(path: &str) -> bool {
    BEFORE_CONSENT.contains(&path.trim_end_matches('/'))
}

/// What a session that must change its password may still call.
const BEFORE_PASSWORD_CHANGE: &[&str] = &[
    "/auth/me",
    "/auth/consent",
    "/auth/password",
    "/auth/logout",
    "/auth/public",
    "/auth/sessions",
    "/public/banner",
    "/public/warning-banner",
];

/// Whether a session that must change its password may call `path`.
pub fn allowed_before_password_change(path: &str) -> bool {
    BEFORE_PASSWORD_CHANGE.contains(&path.trim_end_matches('/'))
}

/// Changes a track manager may make: the picture and its curation.
const TRACK_MANAGEMENT: &[&str] = &[
    "/tracks/",
    "/groups",
    "/registry",
    "/correlation/suggestions/",
    "/decisions/",
    "/history/",
];

fn under(path: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| {
        path == *p
            || path.starts_with(p)
                && (p.ends_with('/') || path.as_bytes().get(p.len()) == Some(&b'/'))
    })
}

pub fn need(method: &Method, path: &str) -> Need {
    let path = path.trim_end_matches('/');
    if PUBLIC.contains(&path) {
        return Need::Public;
    }
    if SIGNED_IN.contains(&path) || under(path, SIGNED_IN_UNDER) {
        return Need::SignedIn;
    }
    if under(path, ADMIN_READS) {
        return Need::Role(Role::Admin);
    }
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return Need::Role(Role::Viewer);
    }
    if under(path, TRACK_MANAGEMENT) {
        return Need::Role(Role::TrackManager);
    }
    Need::Role(Role::Admin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(m: Method, p: &str) -> Need {
        need(&m, p)
    }

    #[test]
    fn each_path_needs_its_role() {
        let (public, signed_in) = (Need::Public, Need::SignedIn);
        let viewer = Need::Role(Role::Viewer);
        let track_manager = Need::Role(Role::TrackManager);
        let admin = Need::Role(Role::Admin);
        assert_eq!(n(Method::POST, "/auth/login"), public);
        assert_eq!(n(Method::GET, "/public/banner"), public);
        assert_eq!(n(Method::GET, "/auth/me"), signed_in);
        assert_eq!(n(Method::POST, "/auth/password"), signed_in);
        assert_eq!(n(Method::GET, "/auth/sessions"), signed_in);
        assert_eq!(n(Method::DELETE, "/auth/sessions/abc"), signed_in);
        assert_eq!(n(Method::GET, "/audit"), admin);
        assert_eq!(n(Method::GET, "/audit/verify"), admin);
        assert_eq!(n(Method::POST, "/auth/users/u1/unlock"), admin);
        assert!(allowed_before_password_change("/auth/password"));
        assert!(!allowed_before_password_change("/tracks"));
        assert_eq!(n(Method::GET, "/auth/users"), admin);
        assert_eq!(n(Method::GET, "/export/config"), admin);
        assert_eq!(n(Method::GET, "/import/config"), admin);
        assert_eq!(n(Method::POST, "/import/config"), admin);
        assert_eq!(n(Method::GET, "/tracks"), viewer);
        assert_eq!(n(Method::GET, "/sources/ais"), viewer);
        // The maps' tiles: every signed-in role, as the maps themselves.
        assert_eq!(n(Method::GET, "/basemap/3/2/1"), viewer);
        assert!(!allowed_before_password_change("/basemap/3/2/1"));
        assert_eq!(n(Method::POST, "/tracks/pair"), track_manager);
        assert_eq!(n(Method::POST, "/tracks/OTK1/split"), track_manager);
        assert_eq!(n(Method::PUT, "/groups/3"), track_manager);
        assert_eq!(n(Method::POST, "/registry/import-sheet"), track_manager);
        assert_eq!(
            n(Method::POST, "/correlation/suggestions/4/accept"),
            track_manager
        );
        assert_eq!(n(Method::POST, "/decisions/9/undo"), track_manager);
        // Configuration is an admin's.
        assert_eq!(n(Method::PUT, "/correlation/settings"), admin);
        assert_eq!(n(Method::POST, "/sources"), admin);
        assert_eq!(n(Method::PUT, "/settings"), admin);
        assert_eq!(n(Method::PUT, "/sync/keys/BBB"), admin);
        assert_eq!(n(Method::DELETE, "/sync/keys/BBB"), admin);
        assert_eq!(n(Method::POST, "/plugins"), admin);
        assert_eq!(n(Method::POST, "/admin/purge"), admin);
        // A prefix is a whole path segment: /groupsx is not /groups.
        assert_eq!(n(Method::POST, "/groupsx"), admin);
        assert_eq!(n(Method::POST, "/some/new/route"), admin);
    }
}
