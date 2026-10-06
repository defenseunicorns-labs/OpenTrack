//! TAK outputs as an admin configures them (Settings → TAK output), saved
//! with the instance settings under `tak`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use ot_source::tls::{ClientTls, ServerTls};
use ot_source::transport::{MulticastInterface, check_multicast};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default multicast group and port of TAK's situational awareness (SA)
/// multicast.
pub const DEFAULT_GROUP: Ipv4Addr = Ipv4Addr::new(239, 2, 3, 1);
pub const DEFAULT_PORT: u16 = 6969;
pub const DEFAULT_STALE_SECS: f64 = 60.0;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TakSettings {
    pub outputs: Vec<TakOutput>,
}

/// One TAK output: where the Cursor-on-Target stream goes, and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakOutput {
    /// Short name, unique among the outputs (letters, digits, `-`, `_`):
    /// it names the output in logs, metrics and status.
    pub id: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Seconds after each send that TAK drops a track it hears nothing more
    /// of. Each live track is sent again every half of this.
    #[serde(default = "stale")]
    pub stale_secs: f64,
    /// Put the track number and its sources in `<remarks>`.
    #[serde(default = "yes")]
    pub remarks: bool,
    pub delivery: Delivery,
}

fn yes() -> bool {
    true
}

fn stale() -> f64 {
    DEFAULT_STALE_SECS
}

fn default_group() -> String {
    DEFAULT_GROUP.to_string()
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

fn default_ttl() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Delivery {
    /// Connect to a TAK Server's streaming input (TCP, e.g. 8087, or TLS
    /// with a client certificate, e.g. 8089) and keep the connection up.
    TakServer {
        host: String,
        port: u16,
        /// TLS, with the CA and client certificate TAK Server asks for.
        /// Absent: plain TCP.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tls: Option<ClientTls>,
    },
    /// UDP datagrams, one event each, to a multicast group (or a unicast
    /// address).
    Multicast {
        #[serde(default = "default_group")]
        group: String,
        #[serde(default = "default_port")]
        port: u16,
        /// Multicast hops (1: this network only); the hop limit for IPv6.
        #[serde(default = "default_ttl")]
        ttl: u32,
        /// Where to send from (default: the system's choice): a local
        /// interface address for IPv4, an interface name or index for an
        /// IPv6 group.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interface: Option<String>,
    },
    /// Listen for ATAK/WinTAK clients (their TAK Server connection pointed
    /// here): each gets the live picture, then the stream.
    Listen {
        bind: String,
        /// TLS, optionally requiring client certificates. Absent: plain TCP.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tls: Option<ServerTls>,
    },
}

impl Delivery {
    pub fn kind(&self) -> &'static str {
        match self {
            Delivery::TakServer { .. } => "tak_server",
            Delivery::Multicast { .. } => "multicast",
            Delivery::Listen { .. } => "listen",
        }
    }

    /// Whether what goes out is encrypted.
    pub fn encrypted(&self) -> bool {
        match self {
            Delivery::TakServer { tls, .. } => tls.is_some(),
            Delivery::Listen { tls, .. } => tls.is_some(),
            Delivery::Multicast { .. } => false,
        }
    }
}

fn blank(s: &Option<String>) -> bool {
    s.as_deref().is_none_or(|s| s.trim().is_empty())
}

impl TakOutput {
    pub fn check(&self) -> Result<(), String> {
        let id = &self.id;
        if id.is_empty()
            || id.len() > 32
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(format!(
                "TAK output id {id:?}: 1 to 32 letters, digits, - or _"
            ));
        }
        if !(10.0..=86_400.0).contains(&self.stale_secs) {
            return Err(format!("TAK output {id}: stale_secs is 10 to 86400"));
        }
        let at = |e: String| format!("TAK output {id}: {e}");
        match &self.delivery {
            Delivery::TakServer { host, port, tls } => {
                if host.trim().is_empty() {
                    return Err(at("give the TAK Server's host".into()));
                }
                if *port == 0 {
                    return Err(at("give the TAK Server's port".into()));
                }
                if let Some(t) = tls {
                    t.check().map_err(at)?;
                }
            }
            Delivery::Multicast {
                group,
                port,
                ttl,
                interface,
            } => {
                let ip = group
                    .trim()
                    .parse::<IpAddr>()
                    .map_err(|_| at(format!("group {group:?} is not an IP address")))?;
                if *port == 0 {
                    return Err(at("give the port".into()));
                }
                if !(1..=255).contains(ttl) {
                    return Err(at("ttl is 1 to 255".into()));
                }
                let iface = if blank(interface) {
                    None
                } else {
                    Some(
                        interface
                            .as_deref()
                            .unwrap_or_default()
                            .parse::<MulticastInterface>()
                            .map_err(at)?,
                    )
                };
                if ip.is_multicast() {
                    check_multicast(ip, iface.as_ref()).map_err(at)?;
                } else {
                    // A unicast address: IPv4 may still pick the address
                    // to send from; IPv6 routing picks it.
                    match (ip, &iface) {
                        (_, None) | (IpAddr::V4(_), Some(MulticastInterface::Address(_))) => {}
                        (IpAddr::V4(_), Some(_)) => {
                            return Err(at("interface is an IPv4 address of this host".into()));
                        }
                        (IpAddr::V6(_), Some(_)) => {
                            return Err(at(
                                "interface applies to an IPv6 multicast group, not a unicast address"
                                    .into(),
                            ));
                        }
                    }
                }
            }
            Delivery::Listen { bind, tls } => {
                bind.trim()
                    .parse::<SocketAddr>()
                    .map_err(|_| at(format!("bind {bind:?} is not an address:port")))?;
                if let Some(t) = tls {
                    t.check().map_err(at)?;
                }
            }
        }
        Ok(())
    }
}

impl TakSettings {
    pub fn check(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for o in &self.outputs {
            o.check()?;
            if !seen.insert(o.id.as_str()) {
                return Err(format!("two TAK outputs are called {}", o.id));
            }
        }
        let mut binds = std::collections::HashSet::new();
        for o in self.outputs.iter().filter(|o| o.enabled) {
            if let Delivery::Listen { bind, .. } = &o.delivery
                && !binds.insert(bind.trim())
            {
                return Err(format!("two TAK outputs listen on {bind}"));
            }
        }
        Ok(())
    }
}

/// The TAK outputs the saved instance settings ask for.
pub fn tak_settings(saved: &Value) -> TakSettings {
    serde_json::from_value(saved["tak"].clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn outputs_parse_with_defaults_and_check() {
        let s: TakSettings = serde_json::from_value(json!({"outputs": [
            {"id": "tak", "delivery": {"kind": "tak_server", "host": "tak.example", "port": 8089,
                "tls": {"ca_file": "/etc/tak/ca.pem", "cert_file": "/etc/tak/ot.pem", "key_file": "/etc/tak/ot.key"}}},
            {"id": "sa", "delivery": {"kind": "multicast"}},
            {"id": "eud", "enabled": false, "stale_secs": 120, "delivery": {"kind": "listen", "bind": "0.0.0.0:8089"}},
        ]}))
        .unwrap();
        s.check().unwrap();
        assert_eq!(s.outputs[0].stale_secs, 60.0);
        assert!(s.outputs[0].enabled && s.outputs[0].remarks);
        assert_eq!(
            s.outputs[1].delivery,
            Delivery::Multicast {
                group: "239.2.3.1".into(),
                port: 6969,
                ttl: 1,
                interface: None
            }
        );
        assert!(s.outputs[0].delivery.encrypted());
        assert!(!s.outputs[2].delivery.encrypted());
        // Round trip.
        let back: TakSettings = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert_eq!(tak_settings(&json!({"tak": s})), s);
        assert_eq!(tak_settings(&json!({})), TakSettings::default());
    }

    #[test]
    fn bad_outputs_are_refused() {
        let out = |id: &str, delivery: Value| -> Result<(), String> {
            let o: TakOutput = serde_json::from_value(json!({"id": id, "delivery": delivery}))
                .map_err(|e| e.to_string())?;
            o.check()
        };
        assert!(
            out("a b", json!({"kind": "multicast"}))
                .unwrap_err()
                .contains("letters")
        );
        assert!(out("m", json!({"kind": "multicast", "group": "x"})).is_err());
        assert!(out("m", json!({"kind": "multicast", "ttl": 0})).is_err());
        assert!(out("m", json!({"kind": "multicast", "interface": "eth0"})).is_err());
        assert!(out("m", json!({"kind": "multicast", "interface": "10.0.0.2"})).is_ok());
        // IPv6: a multicast group, joined by interface name or index.
        let v6 = |g: &str, i: Option<&str>| {
            let mut d = json!({"kind": "multicast", "group": g});
            if let Some(i) = i {
                d["interface"] = json!(i);
            }
            out("m", d)
        };
        assert!(v6("ff15::6969", None).is_ok());
        assert!(v6("ff15::6969", Some("eth0")).is_ok());
        assert!(v6("ff15::6969", Some("2")).is_ok());
        assert!(v6("ff15::6969", Some("10.0.0.2")).is_err(), "IPv4 address");
        assert!(v6("ff15::6969", Some("fe80::1")).is_err(), "IPv6 address");
        assert!(v6("2001:db8::1", None).is_ok(), "unicast, as for IPv4");
        assert!(v6("2001:db8::1", Some("eth0")).is_err());
        assert!(v6("239.2.3.1", Some("eth0")).is_err());
        assert!(out("t", json!({"kind": "tak_server", "host": "", "port": 8087})).is_err());
        assert!(out("t", json!({"kind": "tak_server", "host": "h", "port": 0})).is_err());
        assert!(
            out(
                "t",
                json!({"kind": "tak_server", "host": "h", "port": 1, "tls": {"cert_file": "c"}})
            )
            .is_err()
        );
        assert!(out("l", json!({"kind": "listen", "bind": "nowhere"})).is_err());
        assert!(out("l", json!({"kind": "listen", "bind": "0.0.0.0:1", "tls": {"cert_file": "", "key_file": "k"}})).is_err());
        assert!(out("l", json!({"kind": "carrier_pigeon"})).is_err());
        let two = TakSettings {
            outputs: vec![
                serde_json::from_value(
                    json!({"id": "a", "delivery": {"kind": "listen", "bind": "0.0.0.0:1"}}),
                )
                .unwrap(),
                serde_json::from_value(json!({"id": "a", "delivery": {"kind": "multicast"}}))
                    .unwrap(),
            ],
        };
        assert!(two.check().unwrap_err().contains("two"));
        let mut s = TakOutput {
            stale_secs: 5.0,
            ..two.outputs[1].clone()
        };
        assert!(s.check().is_err());
        s.stale_secs = 30.0;
        s.check().unwrap();
    }
}
