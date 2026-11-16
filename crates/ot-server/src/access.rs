//! Transport requirements for a source access controller. This is a plan,
//! not permission to connect: the deployment must approve and reconcile it.

use std::net::{IpAddr, SocketAddr};

use axum::{Json, extract::State};
use ot_source::transport::{MulticastInterface, TransportConfig};
use serde::Serialize;
use serde_json::{Value, json};
use url::Url;

use crate::control::{ApiError, AppState};

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Requirement {
    HostEgress {
        host: String,
        port: u16,
        protocol: &'static str,
    },
    TcpEgress {
        host: String,
        port: u16,
    },
    TcpIngress {
        bind: SocketAddr,
        mode: &'static str,
        tls_at_source: bool,
    },
    UdpIngress {
        bind: SocketAddr,
        multicast_group: Option<IpAddr>,
        multicast_interface: Option<MulticastInterface>,
    },
    FileShare {
        path: String,
    },
}

fn endpoint(url: &str, schemes: &[(&str, u16, &'static str)]) -> Result<Requirement, String> {
    let parsed = Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    let (default_port, protocol) = schemes
        .iter()
        .find(|(scheme, _, _)| *scheme == parsed.scheme())
        .map(|(_, port, protocol)| (*port, *protocol))
        .ok_or_else(|| format!("unsupported URL scheme: {}", parsed.scheme()))?;
    let host = parsed
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| "URL needs a host".to_owned())?;
    let port = parsed.port().unwrap_or(default_port);
    if let Ok(ip) = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
    {
        return Ok(Requirement::TcpEgress {
            host: ip.to_string(),
            port,
        });
    }
    Ok(Requirement::HostEgress {
        host: host.to_owned(),
        port,
        protocol,
    })
}

pub(crate) fn access_requirement(transport: &TransportConfig) -> Result<Requirement, String> {
    use TransportConfig as T;
    match transport {
        T::HttpPoll { url, .. } => endpoint(url, &[("http", 80, "HTTP"), ("https", 443, "TLS")]),
        T::Websocket { url, .. } => endpoint(url, &[("ws", 80, "HTTP"), ("wss", 443, "TLS")]),
        T::GrpcClient { url, .. } => endpoint(url, &[("http", 80, "HTTP"), ("https", 443, "TLS")]),
        T::Mqtt { url, .. } => {
            let parsed = Url::parse(url).map_err(|e| format!("invalid MQTT URL: {e}"))?;
            let host = parsed
                .host_str()
                .ok_or_else(|| "MQTT URL needs a host".to_owned())?;
            match parsed.scheme() {
                "mqtts" => {
                    let port = parsed.port().unwrap_or(8883);
                    if let Ok(ip) = host
                        .trim_start_matches('[')
                        .trim_end_matches(']')
                        .parse::<IpAddr>()
                    {
                        Ok(Requirement::TcpEgress {
                            host: ip.to_string(),
                            port,
                        })
                    } else {
                        Ok(Requirement::HostEgress {
                            host: host.to_owned(),
                            port,
                            protocol: "TLS",
                        })
                    }
                }
                "mqtt" => Ok(Requirement::TcpEgress {
                    host: host.to_owned(),
                    port: parsed.port().unwrap_or(1883),
                }),
                scheme => Err(format!("unsupported MQTT scheme: {scheme}")),
            }
        }
        T::TcpClient {
            host, port, tls, ..
        } if tls.is_some() => {
            if let Ok(ip) = host.parse::<IpAddr>() {
                Ok(Requirement::TcpEgress {
                    host: ip.to_string(),
                    port: *port,
                })
            } else {
                Ok(Requirement::HostEgress {
                    host: host.clone(),
                    port: *port,
                    protocol: "TLS",
                })
            }
        }
        T::TcpClient { host, port, .. } => Ok(Requirement::TcpEgress {
            host: host.clone(),
            port: *port,
        }),
        T::TcpServer { bind, tls, .. } => Ok(Requirement::TcpIngress {
            bind: bind.parse().map_err(|e| format!("invalid TCP bind: {e}"))?,
            mode: "raw_tcp",
            tls_at_source: tls.is_some(),
        }),
        T::GrpcServer { bind, tls, .. } => Ok(Requirement::TcpIngress {
            bind: bind
                .parse()
                .map_err(|e| format!("invalid gRPC bind: {e}"))?,
            mode: "grpc",
            tls_at_source: tls.is_some(),
        }),
        T::Udp {
            bind,
            multicast_group,
            multicast_interface,
            ..
        } => Ok(Requirement::UdpIngress {
            bind: bind.parse().map_err(|e| format!("invalid UDP bind: {e}"))?,
            multicast_group: *multicast_group,
            multicast_interface: multicast_interface.clone(),
        }),
        T::File { path, .. } => Ok(Requirement::FileShare { path: path.clone() }),
    }
}

pub async fn plan(
    State(_s): State<AppState>,
    Json(transport): Json<TransportConfig>,
) -> Result<Json<Value>, ApiError> {
    transport
        .check()
        .map_err(|e| ApiError::unprocessable(e.to_string()))?;
    let requirement = access_requirement(&transport).map_err(ApiError::unprocessable)?;
    Ok(Json(json!({
        "transport": transport.kind(),
        "requirement": requirement,
    })))
}

/// The source ID and revision let a reconciler replace or retire access when
/// a draft changes or is deleted. The response contains no source secrets.
pub async fn plans(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    let rows = s.with_db(|db| db.list_sources()).await?;
    let plans: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            let result = row
                .spec
                .get("transport")
                .cloned()
                .ok_or_else(|| "source has no transport".to_owned())
                .and_then(|value| {
                    serde_json::from_value::<TransportConfig>(value).map_err(|e| e.to_string())
                })
                .and_then(|transport| {
                    transport.check().map_err(|e| e.to_string())?;
                    access_requirement(&transport)
                });
            match result {
                Ok(requirement) => json!({
                    "source": row.id,
                    "revision": row.revision,
                    "enabled": row.enabled,
                    "requirement": requirement,
                }),
                Err(error) => json!({
                    "source": row.id,
                    "revision": row.revision,
                    "enabled": row.enabled,
                    "error": error,
                }),
            }
        })
        .collect();
    Ok(Json(json!({ "plans": plans })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_transport_has_an_access_requirement() {
        let cases = [
            (
                json!({"type":"http_poll","url":"https://api.example/v1"}),
                "host_egress",
                "TLS",
                443,
            ),
            (
                json!({"type":"websocket","url":"wss://stream.example/ws"}),
                "host_egress",
                "TLS",
                443,
            ),
            (
                json!({"type":"http_poll","url":"https://192.0.2.4/v1"}),
                "tcp_egress",
                "",
                443,
            ),
            (
                json!({"type":"grpc_client","url":"http://grpc.example:8080","method":"p.S/M"}),
                "host_egress",
                "HTTP",
                8080,
            ),
            (
                json!({"type":"tcp_client","host":"tcp.example","port":9000}),
                "tcp_egress",
                "",
                9000,
            ),
            (
                json!({"type":"tcp_client","host":"tcp.example","port":9001,"tls":{}}),
                "host_egress",
                "TLS",
                9001,
            ),
            (
                json!({"type":"mqtt","url":"mqtt://broker.example","topics":["tracks/#"]}),
                "tcp_egress",
                "",
                1883,
            ),
            (
                json!({"type":"mqtt","url":"mqtts://broker.example","topics":["tracks/#"]}),
                "host_egress",
                "TLS",
                8883,
            ),
            (
                json!({"type":"tcp_server","bind":"0.0.0.0:7000"}),
                "tcp_ingress",
                "",
                0,
            ),
            (
                json!({"type":"grpc_server","bind":"0.0.0.0:7001"}),
                "tcp_ingress",
                "",
                0,
            ),
            (
                json!({"type":"udp","bind":"0.0.0.0:7002"}),
                "udp_ingress",
                "",
                0,
            ),
            (
                json!({"type":"udp","bind":"0.0.0.0:7003","multicast_group":"239.1.2.3"}),
                "udp_ingress",
                "",
                0,
            ),
            (
                json!({"type":"file","path":"/data/source.json"}),
                "file_share",
                "",
                0,
            ),
        ];
        for (raw, kind, protocol, port) in cases {
            let transport: TransportConfig = serde_json::from_value(raw).unwrap();
            transport.check().unwrap();
            let requirement =
                serde_json::to_value(access_requirement(&transport).unwrap()).unwrap();
            assert_eq!(requirement["kind"], kind);
            if !protocol.is_empty() {
                assert_eq!(requirement["protocol"], protocol);
            }
            if port != 0 {
                assert_eq!(requirement["port"], port);
            }
            if transport.kind() == "tcp_server" || transport.kind() == "grpc_server" {
                assert_eq!(requirement["tls_at_source"], false);
            }
        }
    }
}
