//! Destination logging for outbound connections (ASD STIG V-222470, AU-3).
//!
//! Every outbound connection logs, at INFO and on each (re)connect, the
//! component and the resolved remote address in the field `peer_addr`.
//! Where the caller holds the socket it logs the socket's peer address;
//! where a library hides the socket it logs the addresses the configured
//! host resolves to. These helpers keep the formatting the same everywhere.

use std::net::SocketAddr;

/// Addresses as one log value: `10.0.0.1:443, [::1]:443`.
pub fn addr_list(addrs: impl IntoIterator<Item = SocketAddr>) -> String {
    addrs
        .into_iter()
        .map(|a| a.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The host and port a URL connects to, for resolving: the scheme's known
/// default port, else `default_port`. IPv6 hosts come without brackets.
/// `None` when the URL does not parse or has no host. Never returns the
/// URL's credentials, path or query.
pub fn url_host_port(url: &str, default_port: u16) -> Option<(String, u16)> {
    let u = url::Url::parse(url).ok()?;
    let host = match u.host()? {
        url::Host::Domain(d) => d.to_owned(),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(ip) => ip.to_string(),
    };
    Some((host, u.port_or_known_default().unwrap_or(default_port)))
}

/// The last remote address a poller reached, so that it logs the address
/// when it changes rather than on every request.
#[derive(Debug, Default)]
pub struct LastPeer(Option<SocketAddr>);

impl LastPeer {
    /// Record `addr`; true when it differs from the last one (or is the first).
    pub fn changed(&mut self, addr: SocketAddr) -> bool {
        let changed = self.0 != Some(addr);
        self.0 = Some(addr);
        changed
    }

    /// Forget the last address, so the next one is logged again (after a
    /// failure, the next success is a reconnect).
    pub fn reset(&mut self) {
        self.0 = None;
    }
}

/// [`LastPeer`] for a client that talks to several destinations (OCSP
/// responders, identity providers), keyed by destination, shared between
/// tasks. Destinations are configured, so the map stays small.
#[derive(Debug, Default)]
pub struct PeerLog(std::sync::Mutex<std::collections::HashMap<String, SocketAddr>>);

impl PeerLog {
    /// Record that `destination` was reached at `addr`; true when that is
    /// new for the destination.
    pub fn changed(&self, destination: &str, addr: SocketAddr) -> bool {
        let mut map = self.0.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(destination.to_owned(), addr) != Some(addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addr_list_joins_v4_and_v6() {
        let a: SocketAddr = "10.0.0.1:443".parse().unwrap();
        let b: SocketAddr = "[::1]:443".parse().unwrap();
        assert_eq!(addr_list([a, b]), "10.0.0.1:443, [::1]:443");
        assert_eq!(addr_list([]), "");
    }

    #[test]
    fn url_host_port_uses_scheme_defaults_and_drops_credentials() {
        assert_eq!(
            url_host_port("https://user:secret@example.com/x?key=1", 0),
            Some(("example.com".into(), 443))
        );
        assert_eq!(
            url_host_port("nats://10.1.2.3", 4222),
            Some(("10.1.2.3".into(), 4222))
        );
        assert_eq!(
            url_host_port("redis://[::1]:6380/0", 6379),
            Some(("::1".into(), 6380))
        );
        assert_eq!(url_host_port("not a url", 1), None);
        assert_eq!(url_host_port("unix:/run/redis.sock", 6379), None);
    }

    #[test]
    fn last_peer_reports_only_changes() {
        let a: SocketAddr = "10.0.0.1:80".parse().unwrap();
        let b: SocketAddr = "10.0.0.2:80".parse().unwrap();
        let mut last = LastPeer::default();
        assert!(last.changed(a));
        assert!(!last.changed(a));
        assert!(last.changed(b));
        last.reset();
        assert!(last.changed(b));
    }

    #[test]
    fn peer_log_tracks_each_destination() {
        let a: SocketAddr = "10.0.0.1:80".parse().unwrap();
        let b: SocketAddr = "10.0.0.2:80".parse().unwrap();
        let log = PeerLog::default();
        assert!(log.changed("ocsp.example", a));
        assert!(!log.changed("ocsp.example", a));
        assert!(log.changed("idp.example", a));
        assert!(log.changed("ocsp.example", b));
        assert!(!log.changed("idp.example", a));
    }
}
