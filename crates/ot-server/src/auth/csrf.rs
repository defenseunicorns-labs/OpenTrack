//! Cross-site request forgery (SC-23): a change must come from OpenTrack's
//! own page, not another site's form or script riding on the browser's
//! credentials.
//!
//! Two checks, on every state-changing request (not GET, HEAD or OPTIONS):
//! - the `Origin` a browser sends (or, without it, the `Referer`) must be
//!   this server: the `Host` it was asked for, the host a proxy says it was
//!   asked for (`X-Forwarded-Host`), or `OT_PUBLIC_URL`. A script or `curl`
//!   sends neither, and passes.
//! - a change made with a cookie (the session's, or OpenStare's) must carry
//!   [`HEADER`], which the page adds to every request. Another site can't
//!   set it: a form can't set headers, and a cross-site script needs CORS,
//!   which OpenTrack never allows.
//!
//! An API token is sent by the caller itself, never by the browser on its
//! own, so a token needs only the first check; so does a client certificate
//! (a browser does send that on its own, but always with an `Origin` on a
//! cross-site change). SAML's assertion consumer is a cross-site form post
//! by design: the identity provider's page sends it.

use axum::http::{HeaderMap, Method};

/// The header OpenTrack's page sends with every API request.
pub const HEADER: &str = "x-opentrack-csrf";

/// Paths another site posts to by design (SAML's POST binding).
const CROSS_SITE_POSTS: &[&str] = &["/auth/saml/acs"];

/// Why a change was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The browser says the request came from another site.
    ForeignOrigin(String),
    /// A cookie-borne change without [`HEADER`].
    NoHeader,
}

impl Refusal {
    pub fn message(&self) -> &'static str {
        match self {
            Self::ForeignOrigin(_) => "this change came from another site",
            Self::NoHeader => "this change did not come from OpenTrack's page",
        }
    }
}

/// Whether a request changes something.
pub fn changes(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Check a request under /api/v1 (`path` without the prefix).
/// `cookie_borne`: the caller was identified by a cookie.
pub fn check(
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    public_url: Option<&str>,
    cookie_borne: bool,
) -> Result<(), Refusal> {
    if !changes(method) || CROSS_SITE_POSTS.contains(&path.trim_end_matches('/')) {
        return Ok(());
    }
    if let Some(origin) = foreign_origin(headers, public_url) {
        return Err(Refusal::ForeignOrigin(origin));
    }
    if cookie_borne && !headers.contains_key(HEADER) {
        return Err(Refusal::NoHeader);
    }
    Ok(())
}

/// The origin a browser says the request came from, when it isn't this
/// server. `Origin: null` (a sandboxed or privacy-stripped page) is never
/// this server's page.
fn foreign_origin(headers: &HeaderMap, public_url: Option<&str>) -> Option<String> {
    let text = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).map(str::trim);
    let origin = match text("origin") {
        Some(o) => o.to_owned(),
        None => origin_of(text("referer")?)?,
    };
    let Some(host) = authority(&origin) else {
        return Some(origin);
    };
    let first = |v: &str| {
        v.split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
    };
    let ours = [
        text("host").map(first),
        text("x-forwarded-host").map(first),
        public_url.and_then(authority),
    ];
    let same = ours.iter().flatten().any(|h| strip_default_port(h) == host);
    (!same).then_some(origin)
}

/// `scheme://host[:port]` of a URL.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let auth = rest.split(['/', '?', '#']).next()?;
    Some(format!("{scheme}://{auth}"))
}

/// The host and port of an origin or URL, lower case, without the scheme's
/// default port.
fn authority(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let auth = rest.split(['/', '?', '#']).next()?;
    // No user info in an origin; a URL with it is not ours.
    if auth.is_empty() || auth.contains('@') {
        return None;
    }
    let auth = auth.to_ascii_lowercase();
    let default = match scheme.to_ascii_lowercase().as_str() {
        "https" => ":443",
        "http" => ":80",
        _ => return None,
    };
    Some(auth.strip_suffix(default).unwrap_or(&auth).to_owned())
}

/// A `Host` header's value without a default port (the scheme isn't known,
/// so either).
fn strip_default_port(host: &str) -> &str {
    host.strip_suffix(":443")
        .or_else(|| host.strip_suffix(":80"))
        .unwrap_or(host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn a_change_from_another_site_is_refused() {
        let post = Method::POST;
        let at = |h: &[(&'static str, &'static str)], cookie| {
            check(&post, "/sources", &headers(h), None, cookie)
        };
        let host = ("host", "ot.example.org");
        // The page: same origin, with the header.
        assert_eq!(
            at(
                &[host, ("origin", "https://ot.example.org"), (HEADER, "1")],
                true
            ),
            Ok(())
        );
        // Another site's form: its origin, no header.
        assert!(matches!(
            at(&[host, ("origin", "https://evil.example")], true),
            Err(Refusal::ForeignOrigin(_))
        ));
        // Even with a token or a certificate.
        assert!(at(&[host, ("origin", "https://evil.example")], false).is_err());
        assert!(at(&[host, ("origin", "null")], false).is_err());
        assert!(at(&[host, ("referer", "https://evil.example/x")], false).is_err());
        // A cookie without the header, even with no origin at all.
        assert_eq!(at(&[host], true), Err(Refusal::NoHeader));
        // A script: no origin, a token.
        assert_eq!(at(&[host], false), Ok(()));
        // Reads are never checked.
        assert_eq!(
            check(
                &Method::GET,
                "/tracks",
                &headers(&[("origin", "https://evil.example")]),
                None,
                true
            ),
            Ok(())
        );
    }

    #[test]
    fn this_server_is_its_host_its_proxy_host_or_its_public_url() {
        let ok = |h: &[(&'static str, &'static str)], public: Option<&str>| {
            foreign_origin(&headers(h), public).is_none()
        };
        assert!(ok(
            &[
                ("host", "ot.example.org:443"),
                ("origin", "https://ot.example.org")
            ],
            None
        ));
        assert!(ok(
            &[
                ("host", "127.0.0.1:8090"),
                ("origin", "http://127.0.0.1:8090")
            ],
            None
        ));
        assert!(!ok(
            &[
                ("host", "127.0.0.1:8090"),
                ("origin", "http://127.0.0.1:8091")
            ],
            None
        ));
        assert!(ok(
            &[
                ("host", "opentrack:8090"),
                ("x-forwarded-host", "ot.example.org"),
                ("origin", "https://OT.example.org")
            ],
            None
        ));
        assert!(ok(
            &[
                ("host", "opentrack:8090"),
                ("origin", "https://ot.example.org")
            ],
            Some("https://ot.example.org/")
        ));
        assert!(ok(
            &[
                ("host", "ot.example.org"),
                ("referer", "https://ot.example.org/sources?x=1")
            ],
            None
        ));
        assert!(!ok(
            &[
                ("host", "ot.example.org"),
                ("origin", "https://ot.example.org.evil.example")
            ],
            None
        ));
    }

    #[test]
    fn samls_consumer_takes_the_identity_providers_post() {
        let h = headers(&[
            ("host", "ot.example.org"),
            ("origin", "https://idp.example"),
        ]);
        assert_eq!(
            check(&Method::POST, "/auth/saml/acs", &h, None, false),
            Ok(())
        );
        assert!(check(&Method::POST, "/auth/login", &h, None, false).is_err());
    }
}
