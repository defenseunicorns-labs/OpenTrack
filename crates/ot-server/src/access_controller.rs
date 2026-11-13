//! Reconcile explicitly approved source destinations into the existing UDS Package.
//! The Kubernetes identity can only read and patch that one Package.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, bail};
use ot_source::transport::{MulticastInterface, TransportConfig};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::{Instant, MissedTickBehavior};

use crate::{
    access::{Requirement, access_requirement},
    config::Common,
};

const OWNED_PREFIX: &str = "OpenTrack source access (managed): ";
const TOKEN_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/token";
const CA_PATH: &str = "/var/run/secrets/kubernetes.io/serviceaccount/ca.crt";

fn normalized_target(host: &str) -> String {
    let unbracketed = host.trim_start_matches('[').trim_end_matches(']');
    unbracketed.parse::<IpAddr>().map_or_else(
        |_| host.trim_end_matches('.').to_ascii_lowercase(),
        |ip| ip.to_string(),
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Approvals {
    #[serde(default)]
    auto: bool,
    #[serde(default)]
    hosts: Vec<HostApproval>,
    #[serde(default)]
    tcp: Vec<TcpApproval>,
    #[serde(default)]
    udp_ports: Vec<u16>,
    #[serde(default)]
    tcp_ingress_ports: Vec<u16>,
    #[serde(default)]
    multicast: Vec<MulticastApproval>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostApproval {
    host: String,
    port: u16,
    protocol: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TcpApproval {
    host: String,
    port: u16,
    cidr: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MulticastApproval {
    group: IpAddr,
    port: u16,
    #[serde(default)]
    interface: Option<String>,
}

#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Serialize, Deserialize)]
struct MulticastRule {
    group: IpAddr,
    port: u16,
    interface: Option<String>,
}

#[derive(Default)]
struct Desired {
    hosts: BTreeSet<HostRule>,
    tcp: BTreeSet<CidrRule>,
    tcp_hosts: BTreeSet<(String, u16)>,
    udp: BTreeSet<u16>,
    tcp_ingress: BTreeSet<u16>,
    multicast: BTreeSet<MulticastRule>,
}

type DnsCache = BTreeMap<(String, u16), (BTreeSet<IpAddr>, Instant)>;

#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq)]
struct CidrRule {
    cidr: String,
    port: u16,
}

impl CidrRule {
    fn new(cidr: &str, port: u16) -> anyhow::Result<Self> {
        let (addr, prefix) = cidr
            .split_once('/')
            .context("TCP egress approval needs an IP CIDR")?;
        let addr: std::net::IpAddr = addr.parse().context("invalid TCP egress IP")?;
        let prefix: u8 = prefix.parse().context("invalid TCP egress prefix")?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        if prefix == 0 || prefix > max || port == 0 {
            bail!("invalid TCP egress CIDR or port");
        }
        Ok(Self {
            cidr: format!("{addr}/{prefix}"),
            port,
        })
    }

    fn package_rule(&self) -> Value {
        json!({
            "direction": "Egress",
            "selector": {"app.kubernetes.io/name": "opentrack", "app.kubernetes.io/component": "server"},
            "remoteCidr": self.cidr,
            "remoteProtocol": "TCP",
            "port": self.port,
            "description": format!("{OWNED_PREFIX}{}:{} TCP", self.cidr, self.port),
        })
    }

    fn contains_ip(&self, ip: IpAddr) -> bool {
        let Some((addr, prefix)) = self.cidr.split_once('/') else {
            return false;
        };
        let Ok(base) = addr.parse::<IpAddr>() else {
            return false;
        };
        let Ok(bits) = prefix.parse::<u32>() else {
            return false;
        };
        match (base, ip) {
            (IpAddr::V4(a), IpAddr::V4(b)) => {
                let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
                (u32::from(a) & mask) == (u32::from(b) & mask)
            }
            (IpAddr::V6(a), IpAddr::V6(b)) => {
                let mask = u128::MAX.checked_shl(128 - bits).unwrap_or(0);
                (u128::from(a) & mask) == (u128::from(b) & mask)
            }
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Ord, PartialOrd, Eq, PartialEq)]
struct HostRule {
    host: String,
    port: u16,
    protocol: String,
}

impl HostRule {
    fn new(host: &str, port: u16, protocol: &str) -> anyhow::Result<Self> {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty()
            || host.len() > 253
            || host.split('.').any(|part| {
                part.is_empty()
                    || part.len() > 63
                    || part.starts_with('-')
                    || part.ends_with('-')
                    || !part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
            })
        {
            bail!("invalid source destination hostname: {host}");
        }
        if port == 0 || !matches!(protocol, "TLS" | "HTTP") {
            bail!("invalid source destination port or protocol");
        }
        Ok(Self {
            host,
            port,
            protocol: protocol.to_owned(),
        })
    }

    fn package_rule(&self, source_service_account: &str) -> Value {
        json!({
            "direction": "Egress",
            "selector": {"app.kubernetes.io/name": "opentrack"},
            "serviceAccount": source_service_account,
            "remoteHost": self.host,
            "remoteProtocol": self.protocol,
            "port": self.port,
            "description": format!("{OWNED_PREFIX}{}:{} {}", self.host, self.port, self.protocol),
        })
    }
}

fn approved_rules(
    rows: Vec<ot_store::SourceRow>,
    approvals: &Approvals,
    available_udp: &BTreeSet<u16>,
    available_tcp: &BTreeSet<u16>,
    relay_enabled: bool,
) -> anyhow::Result<Desired> {
    let approved: BTreeSet<_> = approvals
        .hosts
        .iter()
        .map(|h| HostRule::new(&h.host, h.port, &h.protocol))
        .collect::<anyhow::Result<_>>()?;
    let tcp: Vec<_> = approvals
        .tcp
        .iter()
        .map(|approval| {
            let host = normalized_target(&approval.host);
            if host.parse::<IpAddr>().is_err() {
                HostRule::new(&host, approval.port, "TLS")?;
            }
            let rule = CidrRule::new(&approval.cidr, approval.port)?;
            if let Ok(ip) = host.parse::<IpAddr>()
                && !rule.contains_ip(ip)
            {
                bail!("TCP egress CIDR does not contain its approved IP host");
            }
            Ok((host, approval.port, rule))
        })
        .collect::<anyhow::Result<_>>()?;
    let mut desired = Desired::default();
    let approved_udp: BTreeSet<_> = approvals.udp_ports.iter().copied().collect();
    if approved_udp.contains(&0) || !approved_udp.is_subset(available_udp) {
        bail!("UDP approvals include a port that was not preprovisioned");
    }
    let approved_tcp: BTreeSet<_> = approvals.tcp_ingress_ports.iter().copied().collect();
    if approved_tcp.contains(&0) || !approved_tcp.is_subset(available_tcp) {
        bail!("TCP ingress approvals include a port that was not preprovisioned");
    }
    let approved_multicast: BTreeSet<_> = approvals
        .multicast
        .iter()
        .map(|entry| {
            if !entry.group.is_multicast()
                || entry.port == 0
                || !available_udp.contains(&entry.port)
            {
                bail!("multicast approval needs a multicast group and preprovisioned UDP port");
            }
            if let Some(interface) = &entry.interface {
                let parsed: MulticastInterface =
                    interface.parse().map_err(|e: String| anyhow::anyhow!(e))?;
                ot_source::transport::check_multicast(entry.group, Some(&parsed))
                    .map_err(anyhow::Error::msg)?;
            }
            Ok(MulticastRule {
                group: entry.group,
                port: entry.port,
                interface: entry.interface.clone(),
            })
        })
        .collect::<anyhow::Result<_>>()?;
    let ports: BTreeSet<_> = approved_multicast.iter().map(|rule| rule.port).collect();
    if ports.len() != approved_multicast.len() {
        bail!("each multicast group needs its own UDP port");
    }
    // Drafts need the network path for probe before they can be enabled.
    for row in rows {
        let Some(raw) = row.spec.get("transport") else {
            continue;
        };
        let Ok(transport) = serde_json::from_value::<TransportConfig>(raw.clone()) else {
            continue;
        };
        if transport.check().is_err() {
            continue;
        }
        match access_requirement(&transport) {
            Ok(Requirement::HostEgress {
                host,
                port,
                protocol,
            }) => {
                if let Ok(rule) = HostRule::new(&host, port, protocol)
                    && approved.contains(&rule)
                {
                    desired.hosts.insert(rule);
                }
            }
            Ok(Requirement::TcpEgress { host, port }) => {
                let host = normalized_target(&host);
                for (_, _, rule) in tcp.iter().filter(|(h, p, _)| *h == host && *p == port) {
                    desired.tcp.insert(rule.clone());
                    desired.tcp_hosts.insert((host.clone(), port));
                }
            }
            Ok(Requirement::UdpIngress {
                bind,
                multicast_group: None,
                ..
            }) if bind.ip().is_unspecified() && approved_udp.contains(&bind.port()) => {
                desired.udp.insert(bind.port());
            }
            Ok(Requirement::UdpIngress {
                bind,
                multicast_group: Some(group),
                multicast_interface,
            }) if relay_enabled && bind.ip().is_unspecified() => {
                let rule = MulticastRule {
                    group,
                    port: bind.port(),
                    interface: multicast_interface.map(|i| i.to_string()),
                };
                if approved_multicast.contains(&rule) {
                    desired.multicast.insert(rule);
                }
            }
            Ok(Requirement::TcpIngress { bind, .. })
                if bind.ip().is_unspecified() && approved_tcp.contains(&bind.port()) =>
            {
                desired.tcp_ingress.insert(bind.port());
            }
            _ => {}
        }
    }
    Ok(desired)
}

async fn add_auto_rules(
    rows: &[ot_store::SourceRow],
    desired: &mut Desired,
    available_udp: &BTreeSet<u16>,
    available_tcp: &BTreeSet<u16>,
    relay_enabled: bool,
    dns_cache: &mut DnsCache,
) {
    for row in rows {
        let Some(raw) = row.spec.get("transport") else {
            continue;
        };
        let Ok(transport) = serde_json::from_value::<TransportConfig>(raw.clone()) else {
            continue;
        };
        if transport.check().is_err() {
            continue;
        }
        match access_requirement(&transport) {
            Ok(Requirement::HostEgress {
                host,
                port,
                protocol,
            }) => {
                if let Ok(rule) = HostRule::new(&host, port, protocol) {
                    desired.hosts.insert(rule);
                }
            }
            Ok(Requirement::TcpEgress { host, port }) => {
                let host = normalized_target(&host);
                let looked_up = tokio::time::timeout(
                    Duration::from_secs(5),
                    tokio::net::lookup_host((host.as_str(), port)),
                )
                .await;
                let resolved = match looked_up {
                    Ok(Ok(addrs)) => addrs.map(|addr| addr.ip()).take(8).collect::<BTreeSet<_>>(),
                    Err(error) => {
                        tracing::warn!(source = %row.id, %host, error = %error, "TCP destination lookup timed out");
                        BTreeSet::new()
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(source = %row.id, %host, error = %error, "TCP destination lookup failed");
                        BTreeSet::new()
                    }
                };
                let addrs = if resolved.is_empty() {
                    dns_cache
                        .get(&(host.clone(), port))
                        .filter(|(_, checked_at)| checked_at.elapsed() < Duration::from_secs(600))
                        .map(|(addrs, _)| addrs.clone())
                        .unwrap_or_default()
                } else {
                    dns_cache.insert((host.clone(), port), (resolved.clone(), Instant::now()));
                    resolved
                };
                for ip in addrs.into_iter().take(8) {
                    let cidr = format!("{ip}/{}", if ip.is_ipv4() { 32 } else { 128 });
                    if let Ok(rule) = CidrRule::new(&cidr, port) {
                        desired.tcp.insert(rule);
                        desired.tcp_hosts.insert((host.clone(), port));
                    }
                }
            }
            Ok(Requirement::TcpIngress { bind, .. })
                if bind.ip().is_unspecified() && available_tcp.contains(&bind.port()) =>
            {
                desired.tcp_ingress.insert(bind.port());
            }
            Ok(Requirement::UdpIngress {
                bind,
                multicast_group: None,
                ..
            }) if bind.ip().is_unspecified() && available_udp.contains(&bind.port()) => {
                desired.udp.insert(bind.port());
            }
            Ok(Requirement::UdpIngress {
                bind,
                multicast_group: Some(group),
                multicast_interface,
            }) if relay_enabled
                && bind.ip().is_unspecified()
                && available_udp.contains(&bind.port()) =>
            {
                if desired
                    .multicast
                    .iter()
                    .any(|rule| rule.port == bind.port() && rule.group != group)
                {
                    tracing::warn!(source = %row.id, port = bind.port(), "multicast port already assigned to another group");
                } else {
                    desired.multicast.insert(MulticastRule {
                        group,
                        port: bind.port(),
                        interface: multicast_interface.map(|i| i.to_string()),
                    });
                }
            }
            _ => {}
        }
    }
}

fn merged_expose(package: &Value, desired: &Desired) -> anyhow::Result<Value> {
    let current = package
        .pointer("/spec/network/expose")
        .and_then(Value::as_array)
        .context("UDS Package is missing spec.network.expose")?;
    let mut result: Vec<Value> = current
        .iter()
        .filter(|entry| {
            !entry
                .get("description")
                .and_then(Value::as_str)
                .is_some_and(|description| description.starts_with(OWNED_PREFIX))
        })
        .cloned()
        .collect();
    result.extend(desired.udp.iter().map(|port| json!({
        "service": format!("opentrack-source-udp-{port}"),
        "selector": {"app.kubernetes.io/name": "opentrack", "app.kubernetes.io/component": "server"},
        "protocol": "UDP",
        "port": port,
        "description": format!("{OWNED_PREFIX}UDP ingress {port}"),
    })));
    Ok(Value::Array(result))
}

fn source_decision(row: &ot_store::SourceRow, desired: &Desired, reconciled: bool) -> Value {
    let requirement = row
        .spec
        .get("transport")
        .cloned()
        .ok_or_else(|| "source has no transport".to_owned())
        .and_then(|raw| serde_json::from_value::<TransportConfig>(raw).map_err(|e| e.to_string()))
        .and_then(|transport| access_requirement(&transport));
    let (state, reason) = match requirement {
        Ok(Requirement::HostEgress {
            host,
            port,
            protocol,
        }) => {
            let approved =
                HostRule::new(&host, port, protocol).is_ok_and(|r| desired.hosts.contains(&r));
            (
                if !approved {
                    "pending_approval"
                } else if reconciled {
                    "policy_applied"
                } else {
                    "reconcile_failed"
                },
                None,
            )
        }
        Ok(Requirement::TcpEgress { host, port }) => {
            let approved = desired
                .tcp_hosts
                .contains(&(normalized_target(&host), port));
            (
                if !approved {
                    "pending_approval"
                } else if reconciled {
                    "policy_applied"
                } else {
                    "reconcile_failed"
                },
                None,
            )
        }
        Ok(Requirement::UdpIngress {
            bind,
            multicast_group: None,
            ..
        }) => {
            let approved = bind.ip().is_unspecified() && desired.udp.contains(&bind.port());
            (
                if !approved {
                    "pending_approval"
                } else if reconciled {
                    "policy_applied"
                } else {
                    "reconcile_failed"
                },
                None,
            )
        }
        Ok(Requirement::UdpIngress {
            bind,
            multicast_group: Some(group),
            multicast_interface,
        }) => {
            let rule = MulticastRule {
                group,
                port: bind.port(),
                interface: multicast_interface.map(|i| i.to_string()),
            };
            let approved = bind.ip().is_unspecified() && desired.multicast.contains(&rule);
            (
                if !approved {
                    "pending_approval"
                } else if reconciled {
                    "policy_applied"
                } else {
                    "reconcile_failed"
                },
                None,
            )
        }
        Ok(Requirement::FileShare { .. }) => ("file_share_or_data_path", None),
        Ok(Requirement::TcpIngress { bind, .. }) => {
            let approved = bind.ip().is_unspecified() && desired.tcp_ingress.contains(&bind.port());
            (
                if !approved {
                    "pending_approval"
                } else if reconciled {
                    "policy_applied"
                } else {
                    "reconcile_failed"
                },
                None,
            )
        }
        Err(error) => ("invalid", Some(error)),
    };
    json!({"state":state, "revision":row.revision, "enabled":row.enabled, "reason":reason})
}

fn merged_allow(
    package: &Value,
    desired: &Desired,
    source_service_account: &str,
    relay_node_cidr: Option<&str>,
) -> anyhow::Result<Value> {
    let current = package
        .pointer("/spec/network/allow")
        .and_then(Value::as_array)
        .context("UDS Package is missing spec.network.allow")?;
    let mut result: Vec<Value> = current
        .iter()
        .filter(|rule| {
            !rule
                .get("description")
                .and_then(Value::as_str)
                .is_some_and(|description| description.starts_with(OWNED_PREFIX))
        })
        .cloned()
        .collect();
    for rule in &desired.hosts {
        let candidate = rule.package_rule(source_service_account);
        let already_declared = result.iter().any(|existing| {
            existing.get("direction") == candidate.get("direction")
                && existing.get("selector") == candidate.get("selector")
                && existing.get("serviceAccount") == candidate.get("serviceAccount")
                && existing.get("remoteHost") == candidate.get("remoteHost")
                && existing.get("remoteProtocol") == candidate.get("remoteProtocol")
                && existing.get("port") == candidate.get("port")
        });
        if !already_declared {
            result.push(candidate);
        }
    }
    result.extend(
        desired
            .tcp
            .iter()
            .map(CidrRule::package_rule),
    );
    result.extend(desired.tcp_ingress.iter().map(|port| json!({
        "direction": "Ingress",
        "selector": {"app.kubernetes.io/name": "opentrack", "app.kubernetes.io/component": "server"},
        "remoteNamespace": "istio-tenant-gateway",
        "remoteSelector": {"app": "tenant-ingressgateway"},
        "remoteProtocol": "TCP",
        "port": port,
        "description": format!("{OWNED_PREFIX}{port} TCP ingress"),
    })));
    let relay_node_cidr = if desired.multicast.is_empty() {
        None
    } else {
        let cidr = relay_node_cidr.context("multicast relay node CIDR is required")?;
        Some(CidrRule::new(cidr, 1)?.cidr)
    };
    for port in desired
        .multicast
        .iter()
        .map(|rule| rule.port)
        .collect::<BTreeSet<_>>()
    {
        result.push(json!({
            "direction": "Ingress",
            "selector": {"app.kubernetes.io/name": "opentrack", "app.kubernetes.io/component": "server"},
            "remoteCidr": relay_node_cidr.as_deref(),
            "remoteProtocol": "UDP",
            "port": port,
            "description": format!("{OWNED_PREFIX}{port} multicast relay"),
        }));
    }
    Ok(Value::Array(result))
}

async fn reconcile(
    client: &reqwest::Client,
    api: &str,
    token: &str,
    package_name: &str,
    source_service_account: &str,
    desired: &Desired,
    relay_node_cidr: Option<&str>,
) -> anyhow::Result<()> {
    let url = format!(
        "{api}/apis/uds.dev/v1alpha1/namespaces/{}/packages/{package_name}",
        std::env::var("POD_NAMESPACE").context("POD_NAMESPACE is required")?
    );
    for _ in 0..3 {
        let response = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?;
        let package: Value = response.json().await?;
        let current = package
            .pointer("/spec/network/allow")
            .context("Package has no allow list")?;
        let next = merged_allow(&package, desired, source_service_account, relay_node_cidr)?;
        let current_expose = package
            .pointer("/spec/network/expose")
            .context("Package has no expose list")?;
        let next_expose = merged_expose(&package, desired)?;
        if *current == next && *current_expose == next_expose {
            return Ok(());
        }
        let revision = package
            .pointer("/metadata/resourceVersion")
            .and_then(Value::as_str)
            .context("Package has no resourceVersion")?;
        let patch = json!([
            {"op":"test","path":"/metadata/resourceVersion","value":revision},
            {"op":"replace","path":"/spec/network/allow","value":next},
            {"op":"replace","path":"/spec/network/expose","value":next_expose}
        ]);
        let response = client
            .patch(&url)
            .bearer_auth(token)
            .header("Content-Type", "application/json-patch+json")
            .json(&patch)
            .send()
            .await?;
        if response.status().is_success() {
            return Ok(());
        }
        if response.status().as_u16() != 409 && response.status().as_u16() != 422 {
            bail!("UDS Package patch failed: {}", response.status());
        }
    }
    bail!("UDS Package changed during three reconciliation attempts")
}

async fn reconcile_multicast_plan(
    client: &reqwest::Client,
    api: &str,
    token: &str,
    desired: &BTreeSet<MulticastRule>,
) -> anyhow::Result<()> {
    let namespace = std::env::var("POD_NAMESPACE").context("POD_NAMESPACE is required")?;
    let url = format!("{api}/api/v1/namespaces/{namespace}/configmaps/opentrack-multicast-plan");
    let next = serde_json::to_string(desired)?;
    for _ in 0..3 {
        let config: Value = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if config.pointer("/data/plan.json").and_then(Value::as_str) == Some(next.as_str()) {
            return Ok(());
        }
        let revision = config
            .pointer("/metadata/resourceVersion")
            .and_then(Value::as_str)
            .context("multicast plan has no resourceVersion")?;
        let patch = json!([
            {"op":"test","path":"/metadata/resourceVersion","value":revision},
            {"op":"replace","path":"/data/plan.json","value":next}
        ]);
        let response = client
            .patch(&url)
            .bearer_auth(token)
            .header("Content-Type", "application/json-patch+json")
            .json(&patch)
            .send()
            .await?;
        if response.status().is_success() {
            return Ok(());
        }
        if response.status().as_u16() != 409 && response.status().as_u16() != 422 {
            bail!("multicast plan patch failed: {}", response.status());
        }
    }
    bail!("multicast plan changed during three reconciliation attempts")
}

pub async fn run(
    common: Common,
    shutdown: impl std::future::Future<Output = ()>,
) -> anyhow::Result<()> {
    let approval_path: PathBuf = std::env::var_os("OT_SOURCE_ACCESS_APPROVALS_FILE")
        .context("OT_SOURCE_ACCESS_APPROVALS_FILE is required")?
        .into();
    let package_name =
        std::env::var("OT_SOURCE_ACCESS_PACKAGE").unwrap_or_else(|_| "opentrack".to_owned());
    let source_service_account =
        std::env::var("OT_SOURCE_SERVICE_ACCOUNT").unwrap_or_else(|_| "default".to_owned());
    let available_udp: BTreeSet<u16> = serde_json::from_str::<Vec<u16>>(
        &std::env::var("OT_SOURCE_UDP_PORTS").unwrap_or_else(|_| "[]".to_owned()),
    )?
    .into_iter()
    .collect();
    let available_tcp: BTreeSet<u16> = serde_json::from_str::<Vec<u16>>(
        &std::env::var("OT_SOURCE_TCP_PORTS").unwrap_or_else(|_| "[]".to_owned()),
    )?
    .into_iter()
    .collect();
    let relay_enabled = std::env::var("OT_SOURCE_MULTICAST_RELAY_ENABLED").as_deref() == Ok("true");
    let relay_node_cidr = std::env::var("OT_SOURCE_MULTICAST_NODE_CIDR").ok();
    if relay_enabled {
        CidrRule::new(
            relay_node_cidr
                .as_deref()
                .context("OT_SOURCE_MULTICAST_NODE_CIDR is required for the relay")?,
            1,
        )?;
    }
    let host =
        std::env::var("KUBERNETES_SERVICE_HOST").context("Kubernetes API host is required")?;
    let port = std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".to_owned());
    let api = format!("https://{host}:{port}");
    let ca = reqwest::Certificate::from_pem(&std::fs::read(CA_PATH)?)?;
    let client = reqwest::Client::builder()
        .add_root_certificate(ca)
        .timeout(Duration::from_secs(15))
        .build()?;
    let redis = common.open_redis().await?;
    let mut ticker = tokio::time::interval(Duration::from_secs(15));
    let mut dns_cache = DnsCache::new();
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => return Ok(()),
            _ = ticker.tick() => {
                let attempt = async {
                    tracing::debug!("reconciliation cycle started");
                    // An unreadable or malformed approval file is an error, never an empty policy.
                    let approvals: Approvals = serde_json::from_slice(&tokio::fs::read(&approval_path).await?)?;
                    let rows = common.open_new_db()?.list_sources()?;
                    tracing::debug!(sources = rows.len(), "source configurations loaded");
                    let mut desired = approved_rules(rows.clone(), &approvals, &available_udp, &available_tcp, relay_enabled)?;
                    if approvals.auto {
                        add_auto_rules(&rows, &mut desired, &available_udp, &available_tcp, relay_enabled, &mut dns_cache).await;
                    }
                    tracing::debug!(hosts = desired.hosts.len(), tcp = desired.tcp.len(), "network requirements computed");
                    let token = tokio::fs::read_to_string(TOKEN_PATH).await?;
                    let reconcile_result = async {
                        tracing::debug!("reconciling UDS Package");
                        reconcile(&client, &api, token.trim(), &package_name, &source_service_account, &desired, relay_node_cidr.as_deref()).await?;
                        tracing::debug!("UDS Package reconciled");
                        if relay_enabled {
                            reconcile_multicast_plan(&client, &api, token.trim(), &desired.multicast).await?;
                        }
                        anyhow::Ok(())
                    }.await;
                    for row in &rows {
                        tracing::debug!(source = %row.id, "publishing source access decision");
                        let state = source_decision(row, &desired, reconcile_result.is_ok());
                        redis.put_source_access(&row.id, &state.to_string(), Duration::from_secs(45)).await?;
                    }
                    reconcile_result?;
                    anyhow::Ok(desired.hosts.len() + desired.tcp.len() + desired.udp.len() + desired.tcp_ingress.len() + desired.multicast.len())
                }.await;
                match attempt {
                    Ok(count) => tracing::debug!(count, "source access reconciled"),
                    Err(error) => tracing::warn!(error = %format!("{error:#}"), "source access reconciliation failed"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, transport: Value) -> ot_store::SourceRow {
        ot_store::SourceRow {
            id: id.into(),
            name: id.into(),
            transport: "test".into(),
            codec: "json".into(),
            enabled: false,
            priority: 0,
            revision: 1,
            spec: json!({"transport":transport}),
            raw_subject: None,
            created_at_ms: 0,
            updated_at_ms: 0,
            problems: json!([]),
            draft: true,
        }
    }

    #[test]
    fn merge_preserves_unowned_rules_and_retires_owned_rules() {
        let package = json!({"spec":{"network":{"allow":[
            {"direction":"Egress","description":"existing rule"},
            {"direction":"Egress","description":"OpenTrack source access (managed): old:443 TLS"}
        ]}}});
        let desired = Desired {
            hosts: BTreeSet::from([HostRule::new("STREAM.EXAMPLE", 443, "TLS").unwrap()]),
            ..Default::default()
        };
        let merged = merged_allow(&package, &desired, "default", None).unwrap();
        assert_eq!(merged.as_array().unwrap().len(), 2);
        assert_eq!(merged[0]["description"], "existing rule");
        assert_eq!(merged[1]["remoteHost"], "stream.example");
    }

    #[test]
    fn approval_destinations_are_exact_and_validated() {
        assert_eq!(
            HostRule::new("STREAM.EXAMPLE.", 443, "TLS").unwrap(),
            HostRule::new("stream.example", 443, "TLS").unwrap()
        );
        assert!(HostRule::new("*.example", 443, "TLS").is_err());
        assert!(HostRule::new("stream.example", 0, "TLS").is_err());
        assert!(HostRule::new("stream.example", 443, "UDP").is_err());
        assert!(CidrRule::new("0.0.0.0/0", 9000).is_err());
        assert!(CidrRule::new("192.0.2.0/24", 9000).is_ok());
        let cidr = CidrRule::new("192.0.2.0/24", 9000).unwrap();
        assert!(cidr.contains_ip("192.0.2.4".parse().unwrap()));
        assert!(!cidr.contains_ip("192.0.3.4".parse().unwrap()));
    }

    #[test]
    fn udp_exposure_preserves_the_ui_and_retires_old_ports() {
        let package = json!({"spec":{"network":{"expose":[
            {"service":"opentrack","description":"OpenTrack UI and API"},
            {"service":"old","description":"OpenTrack source access (managed): UDP ingress 7000"}
        ]}}});
        let desired = Desired {
            udp: BTreeSet::from([7001]),
            ..Default::default()
        };
        let merged = merged_expose(&package, &desired).unwrap();
        assert_eq!(merged.as_array().unwrap().len(), 2);
        assert_eq!(merged[0]["service"], "opentrack");
        assert_eq!(merged[1]["service"], "opentrack-source-udp-7001");
    }

    #[test]
    fn tcp_ingress_only_opens_a_reserved_port_from_the_tenant_gateway() {
        let package = json!({"spec":{"network":{"allow":[
            {"direction":"Ingress","remoteGenerated":"IntraNamespace"}
        ]}}});
        let desired = Desired {
            tcp_ingress: BTreeSet::from([7000]),
            ..Default::default()
        };
        let merged = merged_allow(&package, &desired, "default", None).unwrap();
        assert_eq!(merged.as_array().unwrap().len(), 2);
        assert_eq!(merged[0]["remoteGenerated"], "IntraNamespace");
        assert_eq!(merged[1]["remoteNamespace"], "istio-tenant-gateway");
        assert_eq!(merged[1]["remoteSelector"]["app"], "tenant-ingressgateway");
        assert_eq!(merged[1]["port"], 7000);
    }

    #[test]
    fn multicast_relay_requires_the_exact_group_and_reserved_port() {
        let row = ot_store::SourceRow {
            id: "multicast".into(),
            name: "Multicast".into(),
            transport: "udp".into(),
            codec: "json".into(),
            enabled: false,
            priority: 0,
            revision: 1,
            spec: json!({"transport":{"type":"udp","bind":"0.0.0.0:7002","multicast_group":"239.2.3.4"}}),
            raw_subject: None,
            created_at_ms: 0,
            updated_at_ms: 0,
            problems: json!([]),
            draft: true,
        };
        let approvals: Approvals = serde_json::from_value(json!({
            "multicast":[{"group":"239.2.3.4","port":7002}]
        }))
        .unwrap();
        let available = BTreeSet::from([7002]);
        let desired = approved_rules(
            vec![row.clone()],
            &approvals,
            &available,
            &BTreeSet::new(),
            true,
        )
        .unwrap();
        assert_eq!(desired.multicast.len(), 1);
        let absent =
            approved_rules(vec![row], &approvals, &available, &BTreeSet::new(), false).unwrap();
        assert!(absent.multicast.is_empty());
    }

    #[test]
    fn multicast_ingress_uses_an_exact_node_cidr() {
        let package = json!({"spec":{"network":{"allow":[]}}});
        let desired = Desired {
            multicast: BTreeSet::from([MulticastRule {
                group: "239.2.3.4".parse().unwrap(),
                port: 7101,
                interface: Some("192.0.2.10".into()),
            }]),
            ..Default::default()
        };
        assert!(merged_allow(&package, &desired, "default", None).is_err());
        let merged = merged_allow(&package, &desired, "default", Some("192.0.2.10/32"))
            .unwrap();
        assert_eq!(merged[0]["remoteCidr"], "192.0.2.10/32");
        assert_eq!(merged[0]["remoteProtocol"], "UDP");
        assert!(merged[0].get("remoteGenerated").is_none());
    }

    #[test]
    fn tcp_cidr_egress_uses_server_selector_without_service_account() {
        let rule = CidrRule::new("192.0.2.10/32", 1883).unwrap().package_rule();
        assert_eq!(rule["remoteCidr"], "192.0.2.10/32");
        assert_eq!(rule["selector"]["app.kubernetes.io/component"], "server");
        assert!(rule.get("serviceAccount").is_none());
    }

    #[test]
    fn multiple_tcp_listeners_have_distinct_policy_names() {
        let package = json!({"spec":{"network":{"allow":[]}}});
        let desired = Desired {
            tcp_ingress: BTreeSet::from([7000, 7001]),
            ..Default::default()
        };
        let rules = merged_allow(&package, &desired, "default", None).unwrap();
        assert_ne!(rules[0]["description"], rules[1]["description"]);
        assert!(rules[0]["description"].as_str().unwrap().contains("7000"));
        assert!(rules[1]["description"].as_str().unwrap().contains("7001"));
    }

    #[tokio::test]
    async fn multicast_relay_forwards_a_datagram() {
        use socket2::{Domain, Protocol, Socket, Type};

        let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let rule = MulticastRule {
            group: "239.255.42.42".parse().unwrap(),
            port,
            interface: Some("127.0.0.1".into()),
        };
        let destination = receiver.local_addr().unwrap().to_string();
        let task = tokio::spawn(async move { relay_multicast_to(&rule, &destination).await });
        let sender = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).unwrap();
        sender
            .bind(&"127.0.0.1:0".parse::<SocketAddr>().unwrap().into())
            .unwrap();
        sender
            .set_multicast_if_v4(&"127.0.0.1".parse().unwrap())
            .unwrap();
        sender.set_multicast_loop_v4(true).unwrap();
        let mut buf = [0u8; 32];
        let received = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                sender
                    .send_to(
                        b"track",
                        &format!("239.255.42.42:{port}")
                            .parse::<SocketAddr>()
                            .unwrap()
                            .into(),
                    )
                    .unwrap();
                if let Ok(Ok((n, _))) =
                    tokio::time::timeout(Duration::from_millis(100), receiver.recv_from(&mut buf))
                        .await
                {
                    break &buf[..n] == b"track";
                }
            }
        })
        .await
        .unwrap();
        task.abort();
        assert!(received);
    }

    #[test]
    fn multicast_plan_rejects_unapproved_ports_and_nonmulticast_groups() {
        let available = BTreeSet::from([7002]);
        let rule = |group: &str, port| MulticastRule {
            group: group.parse().unwrap(),
            port,
            interface: None,
        };
        assert!(validate_multicast_plan(vec![rule("239.2.3.4", 7002)], &available).is_ok());
        assert!(validate_multicast_plan(vec![rule("239.2.3.4", 7003)], &available).is_err());
        assert!(validate_multicast_plan(vec![rule("192.0.2.4", 7002)], &available).is_err());
        assert!(
            validate_multicast_plan(
                vec![rule("239.2.3.4", 7002), rule("239.2.3.5", 7002)],
                &available
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn auto_mode_plans_each_network_transport_with_exact_rules() {
        let rows = vec![
            row(
                "http",
                json!({"type":"http_poll","url":"https://api.example"}),
            ),
            row(
                "mqtt",
                json!({"type":"mqtt","url":"mqtt://192.0.2.10","topics":["tracks"]}),
            ),
            row("tcp", json!({"type":"tcp_server","bind":"0.0.0.0:7000"})),
            row("udp", json!({"type":"udp","bind":"0.0.0.0:7100"})),
            row(
                "multicast",
                json!({"type":"udp","bind":"0.0.0.0:7101","multicast_group":"239.2.3.4"}),
            ),
        ];
        let mut desired = Desired::default();
        add_auto_rules(
            &rows,
            &mut desired,
            &BTreeSet::from([7100, 7101]),
            &BTreeSet::from([7000]),
            true,
            &mut DnsCache::new(),
        )
        .await;
        assert!(
            desired
                .hosts
                .contains(&HostRule::new("api.example", 443, "TLS").unwrap())
        );
        assert!(
            desired
                .tcp
                .contains(&CidrRule::new("192.0.2.10/32", 1883).unwrap())
        );
        assert!(desired.tcp_ingress.contains(&7000));
        assert!(desired.udp.contains(&7100));
        assert_eq!(desired.multicast.len(), 1);
    }
}

/// The optional host-network bridge for multicast feeds. Only exact group,
/// interface and port tuples approved by the operator are joined.
pub async fn run_multicast_relay(
    _common: Common,
    shutdown: impl std::future::Future<Output = ()>,
) -> anyhow::Result<()> {
    let plan_path: PathBuf = std::env::var_os("OT_SOURCE_MULTICAST_PLAN_FILE")
        .context("OT_SOURCE_MULTICAST_PLAN_FILE is required")?
        .into();
    let available_udp: BTreeSet<u16> = serde_json::from_str::<Vec<u16>>(
        &std::env::var("OT_SOURCE_UDP_PORTS").unwrap_or_else(|_| "[]".to_owned()),
    )?
    .into_iter()
    .collect();
    let namespace = std::env::var("POD_NAMESPACE").context("POD_NAMESPACE is required")?;
    let mut tasks: BTreeMap<MulticastRule, tokio::task::JoinHandle<()>> = BTreeMap::new();
    let mut ticker = tokio::time::interval(Duration::from_secs(15));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                for (_, task) in tasks { task.abort(); }
                return Ok(());
            }
            _ = ticker.tick() => {
                let desired = (|| -> anyhow::Result<BTreeSet<MulticastRule>> {
                    let rules: Vec<MulticastRule> = serde_json::from_slice(&std::fs::read(&plan_path)?)?;
                    validate_multicast_plan(rules, &available_udp)
                })();
                let desired = match desired {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::warn!(error = %error, "multicast relay plan failed; keeping current groups");
                        continue;
                    }
                };
                tasks.retain(|rule, task| {
                    if desired.contains(rule) && !task.is_finished() { true } else {
                        task.abort();
                        false
                    }
                });
                for rule in desired {
                    if tasks.contains_key(&rule) { continue; }
                    let r = rule.clone();
                    let ns = namespace.clone();
                    let task = tokio::spawn(async move {
                        if let Err(error) = relay_multicast(&r, &ns).await {
                            tracing::warn!(group = %r.group, port = r.port, error = %error, "multicast relay stopped");
                        }
                    });
                    tasks.insert(rule, task);
                }
            }
        }
    }
}

fn validate_multicast_plan(
    rules: Vec<MulticastRule>,
    available_udp: &BTreeSet<u16>,
) -> anyhow::Result<BTreeSet<MulticastRule>> {
    for rule in &rules {
        if !rule.group.is_multicast() || !available_udp.contains(&rule.port) {
            bail!("multicast plan contains a group or port outside the allowed pool");
        }
        if let Some(interface) = &rule.interface {
            let parsed: MulticastInterface = interface.parse().map_err(anyhow::Error::msg)?;
            ot_source::transport::check_multicast(rule.group, Some(&parsed))
                .map_err(anyhow::Error::msg)?;
        }
    }
    let desired: BTreeSet<_> = rules.into_iter().collect();
    if desired
        .iter()
        .map(|rule| rule.port)
        .collect::<BTreeSet<_>>()
        .len()
        != desired.len()
    {
        bail!("multicast plan assigns more than one group to a port");
    }
    Ok(desired)
}

async fn relay_multicast(rule: &MulticastRule, namespace: &str) -> anyhow::Result<()> {
    let destination_name = format!(
        "opentrack-source-udp-{}.{}.svc.cluster.local:{}",
        rule.port, namespace, rule.port
    );
    relay_multicast_to(rule, &destination_name).await
}

async fn relay_multicast_to(rule: &MulticastRule, destination_name: &str) -> anyhow::Result<()> {
    let bind: SocketAddr = if rule.group.is_ipv4() {
        format!("0.0.0.0:{}", rule.port)
    } else {
        format!("[::]:{}", rule.port)
    }
    .parse()?;
    let interface: Option<MulticastInterface> = rule
        .interface
        .as_deref()
        .map(str::parse)
        .transpose()
        .map_err(anyhow::Error::msg)?;
    let incoming = ot_source::transport::udp_socket(bind, Some(rule.group), interface.as_ref())?;
    let outgoing = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    let mut destination = tokio::net::lookup_host(&destination_name)
        .await?
        .find(|addr| addr.is_ipv4())
        .context("UDP source Service has no IPv4 address")?;
    let mut refreshed_at = tokio::time::Instant::now();
    let mut buf = vec![0u8; 65_536];
    tracing::info!(group = %rule.group, port = rule.port, interface = ?rule.interface, "multicast relay joined group");
    loop {
        let (n, _) = incoming.recv_from(&mut buf).await?;
        if refreshed_at.elapsed() >= Duration::from_secs(60) {
            if let Ok(Some(addr)) = tokio::net::lookup_host(&destination_name)
                .await
                .map(|mut addrs| addrs.find(|addr| addr.is_ipv4()))
            {
                destination = addr;
            }
            refreshed_at = tokio::time::Instant::now();
        }
        outgoing.send_to(&buf[..n], destination).await?;
    }
}
