//! Destination logging for outbound connections (ASD STIG V-222470): the
//! shared formatting in [`ot_core::netlog`] plus name resolution for
//! clients whose library hides the socket.

pub use ot_core::netlog::{LastPeer, addr_list, url_host_port};

/// The addresses `host:port` resolves to now, as one log value. A
/// resolution failure is returned as text, since this is for logging only.
pub async fn resolve(host: &str, port: u16) -> String {
    match tokio::net::lookup_host((host, port)).await {
        Ok(addrs) => addr_list(addrs),
        Err(e) => format!("unresolved ({e})"),
    }
}

/// [`resolve`] for a URL's host and port (see [`url_host_port`]).
pub async fn resolve_url(url: &str, default_port: u16) -> String {
    match url_host_port(url, default_port) {
        Some((host, port)) => resolve(&host, port).await,
        None => "unresolved (no host)".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolves_a_local_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(
            resolve("127.0.0.1", port).await,
            format!("127.0.0.1:{port}")
        );
        assert_eq!(
            resolve_url(&format!("http://user:pw@127.0.0.1:{port}/x?key=k"), 0).await,
            format!("127.0.0.1:{port}")
        );
        assert!(resolve_url("nonsense", 1).await.starts_with("unresolved"));
    }
}
