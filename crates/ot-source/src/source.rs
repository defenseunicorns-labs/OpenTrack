//! A source: transport + pipeline, stored as one JSON document.

use serde::{Deserialize, Serialize};

use crate::pipeline::PipelineSpec;
use crate::transport::TransportConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpec {
    /// Stable id: lowercase letters, digits, `-` and `_` (max 64). It names
    /// Redis keys and prefixes source track ids, so it never changes.
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub transport: TransportConfig,
    pub pipeline: PipelineSpec,
    /// Higher wins best-source selection (phase 3).
    #[serde(default = "default_priority")]
    pub priority: i64,
    /// Whether the feed reports tracks (a key per object) or detections
    /// (anonymous plots, associated with system tracks by the engine).
    #[serde(default, skip_serializing_if = "Reports::is_tracks")]
    pub reports: Reports,
    /// Whether this source can make a system track authoritative (published)
    /// on its own. Unset: yes for track feeds, no for detections (a radar or
    /// lidar track stays inside OpenTrack until such a source reports for it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish_alone: Option<bool>,
    /// Reports a new track needs from this source before it is confirmed.
    /// Unset: 1 for track feeds (a feed's track is already a track), the
    /// engine's default (3) for detections, where one plot may be clutter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_after: Option<u64>,
    /// A security label for everything this source reports: its reports and
    /// the tracks they make carry it (OpenStare's `stare-security` shape).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<ot_core::SecurityLabel>,
    /// For a source reporting lines of bearing: how the emitters it hears
    /// may move, which bounds the error of locating them from one moving
    /// sensor. Unset: [`EmitterMotion::default`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emitter_motion: Option<EmitterMotion>,
    /// A listening source must authenticate its senders (mutual TLS, or a
    /// bearer token for gRPC). One that cannot (UDP, multicast) or does not
    /// runs only with this set to `accepted`: the recorded acceptance of
    /// that risk, which the authorising official signs off (ASD V-222533;
    /// see `docs/security/hardening.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unauthenticated: Option<Unauthenticated>,
}

/// The risk acceptance for a listening source that does not authenticate
/// its senders. It has one value so that the JSON says what it means:
/// `"unauthenticated": "accepted"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unauthenticated {
    Accepted,
}

/// How the emitters a bearing source hears may move (see
/// `docs/non-point-contacts.md`, single-sensor location).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmitterMotion {
    /// The hardest an emitter may manoeuvre (turn, weave, speed up) unseen
    /// by a constant-velocity fit, m/s². A ship's turn is about 0.1; a small
    /// boat's weave more; an aircraft's turn several.
    pub manoeuvre_mps2: f64,
    /// The fastest an emitter may move, m/s: how far one the bearings cannot
    /// see moving (it runs along the line of sight) may have gone.
    pub max_speed_mps: f64,
}

impl Default for EmitterMotion {
    /// Surface traffic: a ship's turn, 30 m/s (about 60 knots).
    fn default() -> Self {
        Self {
            manoeuvre_mps2: 0.1,
            max_speed_mps: 30.0,
        }
    }
}

impl EmitterMotion {
    fn check(&self) -> Result<(), String> {
        if !(self.manoeuvre_mps2.is_finite() && (0.0..=100.0).contains(&self.manoeuvre_mps2)) {
            return Err(format!(
                "emitter_motion.manoeuvre_mps2 must be 0-100 m/s², got {}",
                self.manoeuvre_mps2
            ));
        }
        if !(self.max_speed_mps.is_finite() && (0.0..=1500.0).contains(&self.max_speed_mps)) {
            return Err(format!(
                "emitter_motion.max_speed_mps must be 0-1500 m/s, got {}",
                self.max_speed_mps
            ));
        }
        Ok(())
    }
}

impl SourceSpec {
    /// [`Self::publish_alone`] with its default applied.
    pub fn publishes_alone(&self) -> bool {
        self.publish_alone
            .unwrap_or(self.reports == Reports::Tracks)
    }

    /// [`Self::confirm_after`] with its default applied: `None` means the
    /// engine's default.
    pub fn confirms_after(&self) -> Option<u64> {
        self.confirm_after
            .or((self.reports == Reports::Tracks).then_some(1))
            .map(|n| n.max(1))
    }
}

/// Whether a listening transport authenticates its senders; `None` for a
/// transport that connects out (it authenticates the far end, if at all,
/// with its own TLS settings). Mutual TLS counts only when a certificate is
/// required (not `client_cert_optional`); a gRPC bearer token counts too.
fn authenticates_senders(t: &TransportConfig) -> Option<bool> {
    let mutual = |tls: &Option<crate::tls::ServerTls>| {
        tls.as_ref().is_some_and(|t| {
            t.client_ca_file
                .as_deref()
                .is_some_and(|f| !f.trim().is_empty())
                && !t.client_cert_optional
        })
    };
    match t {
        TransportConfig::Udp { .. } => Some(false),
        TransportConfig::TcpServer { tls, .. } => Some(mutual(tls)),
        TransportConfig::GrpcServer { tls, token, .. } => {
            Some(mutual(tls) || token.as_deref().is_some_and(|t| !t.trim().is_empty()))
        }
        TransportConfig::TcpClient { .. }
        | TransportConfig::HttpPoll { .. }
        | TransportConfig::Websocket { .. }
        | TransportConfig::Mqtt { .. }
        | TransportConfig::GrpcClient { .. }
        | TransportConfig::File { .. } => None,
    }
}

fn default_priority() -> i64 {
    100
}

/// What a source's observations are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reports {
    /// Each key is one object over time (AIS, ADS-B, TAK, a radar's tracks).
    #[default]
    Tracks,
    /// Each report is a single detection with no identity of its own.
    Detections,
}

impl Reports {
    fn is_tracks(&self) -> bool {
        *self == Reports::Tracks
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SourceError {
    #[error("source id must be 1-64 characters of a-z, 0-9, '-' or '_', got {0:?}")]
    BadId(String),
    #[error("source name must not be empty")]
    NoName,
    #[error("{0}")]
    Transport(String),
    #[error("{0}")]
    Unauthenticated(String),
    #[error(transparent)]
    Mapping(#[from] crate::mapping::MappingError),
    #[error("framing: {0}")]
    Framing(#[from] crate::frame::FrameError),
    #[error("{0}")]
    Tracker(String),
    #[error("mapping targets schema version {wanted}, but it is {found}")]
    SchemaVersion { wanted: u32, found: String },
    #[error("rule {rule:?} maps ext.{key}, which schema version {version} does not define")]
    UndeclaredExtension {
        rule: String,
        key: String,
        version: u32,
    },
}

impl SourceSpec {
    /// Why this source may not run as configured: a listener that does not
    /// authenticate its senders without the recorded risk acceptance, or
    /// the acceptance on a source that does not need it (so the flag, and
    /// the warning it carries, only ever marks a real exception).
    pub fn authentication_problem(&self) -> Option<String> {
        let accepted = self.unauthenticated == Some(Unauthenticated::Accepted);
        match (authenticates_senders(&self.transport), accepted) {
            (Some(false), false) => Some(format!(
                "a {} source listens without authenticating its senders: {}, or accept \
                 the risk with \"unauthenticated\": \"accepted\"",
                self.transport.kind(),
                match &self.transport {
                    TransportConfig::Udp { .. } => "UDP cannot carry TLS",
                    TransportConfig::GrpcServer { .. } => {
                        "set tls with a client_ca_file (mutual TLS) or a bearer token"
                    }
                    _ => "set tls with a client_ca_file (mutual TLS)",
                }
            )),
            (None, true) => Some(format!(
                "\"unauthenticated\" is for listening sources; a {} source does not listen",
                self.transport.kind()
            )),
            (Some(true), true) => {
                Some("this source authenticates its senders; clear \"unauthenticated\"".to_owned())
            }
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), SourceError> {
        if let Some(label) = &self.security {
            label.validate().map_err(SourceError::Tracker)?;
        }
        if let Some(m) = &self.emitter_motion {
            m.check().map_err(SourceError::Tracker)?;
        }
        let id_ok = !self.id.is_empty()
            && self.id.len() <= 64
            && self
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if !id_ok {
            return Err(SourceError::BadId(self.id.clone()));
        }
        if self.name.trim().is_empty() {
            return Err(SourceError::NoName);
        }
        self.pipeline.mapping.validate()?;
        if let Some(t) = &self.pipeline.tracker {
            t.validate().map_err(SourceError::Tracker)?;
        }
        if let crate::codec::CodecConfig::Plugin { .. }
        | crate::codec::CodecConfig::Protobuf { .. } = &self.pipeline.codec
        {
            crate::codec::Codec::new(self.pipeline.codec.clone())
                .map_err(|e| SourceError::Tracker(e.to_string()))?;
        }
        self.transport.check().map_err(SourceError::Transport)?;
        // gRPC carries protobuf: the method must fit the codec's message.
        if let TransportConfig::GrpcClient { .. } | TransportConfig::GrpcServer { .. } =
            &self.transport
        {
            let proto = crate::transport::ProtoContext::of(&self.pipeline.codec)
                .map_err(SourceError::Transport)?
                .ok_or_else(|| {
                    SourceError::Transport("a gRPC source decodes with the protobuf codec".into())
                })?;
            match &self.transport {
                TransportConfig::GrpcClient {
                    method, request, ..
                } => {
                    proto
                        .client_request(method, request)
                        .map_err(SourceError::Transport)?;
                }
                TransportConfig::GrpcServer { methods, .. } => {
                    proto
                        .server_methods(methods)
                        .map_err(SourceError::Transport)?;
                }
                _ => {}
            }
        }
        if let TransportConfig::TcpClient { framing, .. }
        | TransportConfig::TcpServer { framing, .. } = &self.transport
        {
            crate::frame::Framer::new(framing.clone())?;
        }
        // Last, so that a broken setting is reported before its security.
        if let Some(e) = self.authentication_problem() {
            return Err(SourceError::Unauthenticated(e));
        }
        Ok(())
    }

    /// Validate against the extension schema version the mapping targets
    /// (`None` if that version is not published).
    pub fn validate_against(
        &self,
        schema: Option<&crate::schema::ExtensionSchema>,
    ) -> Result<(), SourceError> {
        self.validate()?;
        let wanted = self.pipeline.mapping.schema_version;
        let Some(schema) = schema.filter(|s| s.version == wanted) else {
            return Err(SourceError::SchemaVersion {
                wanted,
                found: "not published".into(),
            });
        };
        for (key, rules) in crate::schema::mapped_ext_keys(&self.pipeline.mapping) {
            if schema.field(&key).is_none() {
                return Err(SourceError::UndeclaredExtension {
                    rule: rules[0].clone(),
                    key,
                    version: wanted,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emitter_motion_is_checked() {
        assert!(EmitterMotion::default().check().is_ok());
        let fast = EmitterMotion {
            manoeuvre_mps2: 5.0,
            max_speed_mps: 300.0,
        };
        assert!(fast.check().is_ok(), "an aircraft");
        for bad in [
            EmitterMotion {
                manoeuvre_mps2: -0.1,
                max_speed_mps: 30.0,
            },
            EmitterMotion {
                manoeuvre_mps2: f64::NAN,
                max_speed_mps: 30.0,
            },
            EmitterMotion {
                manoeuvre_mps2: 0.1,
                max_speed_mps: 5000.0,
            },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
        let m: EmitterMotion =
            serde_json::from_str(r#"{"manoeuvre_mps2": 0.25, "max_speed_mps": 25}"#).unwrap();
        assert_eq!(m.max_speed_mps, 25.0);
        assert!(serde_json::from_str::<EmitterMotion>(r#"{"manoeuvre_mps2": 0.25}"#).is_err());
    }
    use serde_json::json;

    fn spec(id: &str) -> serde_json::Value {
        json!({
            "id": id, "name": "Test",
            "transport": { "type": "udp", "bind": "0.0.0.0:6969" }, "unauthenticated": "accepted",
            "pipeline": { "codec": { "type": "cot_xml" }, "mapping": { "rules": [
                { "name": "event", "key": "event.@uid", "fields": {
                    "position.latitude": "event.point.@lat", "position.longitude": "event.point.@lon" } } ] } }
        })
    }

    #[test]
    fn detection_feeds_do_not_publish_alone_by_default() {
        let mut v = spec("radar");
        let tracks: SourceSpec = serde_json::from_value(v.clone()).unwrap();
        assert!(tracks.publishes_alone());
        v["reports"] = json!("detections");
        let plots: SourceSpec = serde_json::from_value(v.clone()).unwrap();
        assert!(!plots.publishes_alone());
        v["publish_alone"] = json!(true);
        let chosen: SourceSpec = serde_json::from_value(v).unwrap();
        assert!(chosen.publishes_alone());
    }

    #[test]
    fn ids_are_restricted() {
        let ok: SourceSpec = serde_json::from_value(spec("cot-udp_1")).unwrap();
        ok.validate().unwrap();
        assert_eq!(ok.priority, 100);
        for bad in ["", "CoT", "a b", "x:y"] {
            let s: SourceSpec = serde_json::from_value(spec(bad)).unwrap();
            assert!(matches!(s.validate(), Err(SourceError::BadId(_))), "{bad}");
        }
    }

    #[test]
    fn listeners_authenticate_senders_or_carry_the_acceptance() {
        let with = |transport: serde_json::Value, flag: bool| {
            let mut v = spec("feed");
            v["transport"] = transport;
            if !flag {
                v.as_object_mut().unwrap().remove("unauthenticated");
            }
            check(v)
        };
        let udp = json!({"type": "udp", "bind": "0.0.0.0:6969"});
        let e = with(udp.clone(), false).unwrap_err();
        assert!(
            e.contains("UDP cannot carry TLS") && e.contains("accepted"),
            "{e}"
        );
        with(udp, true).unwrap();
        // TLS alone lets anyone send; so does an optional client certificate.
        let tls = json!({"cert_file": "s.pem", "key_file": "s.key"});
        let tcp = |tls: serde_json::Value| json!({"type": "tcp_server", "bind": "0.0.0.0:8087", "tls": tls});
        assert!(with(json!({"type": "tcp_server", "bind": "0.0.0.0:8087"}), false).is_err());
        assert!(with(tcp(tls.clone()), false).is_err());
        let mut optional = tls.clone();
        optional["client_ca_file"] = json!("ca.pem");
        optional["client_cert_optional"] = json!(true);
        assert!(with(tcp(optional), false).is_err());
        let mut mutual = tls.clone();
        mutual["client_ca_file"] = json!("ca.pem");
        with(tcp(mutual.clone()), false).unwrap();
        // The acceptance only marks a real exception.
        let e = with(tcp(mutual), true).unwrap_err();
        assert!(e.contains("clear"), "{e}");
        let e = with(
            json!({"type": "tcp_client", "host": "feed", "port": 8087}),
            true,
        )
        .unwrap_err();
        assert!(e.contains("does not listen"), "{e}");
        with(
            json!({"type": "tcp_client", "host": "feed", "port": 8087}),
            false,
        )
        .unwrap();
        // The flag round-trips as written.
        let s: SourceSpec = serde_json::from_value(spec("feed")).unwrap();
        assert_eq!(s.unauthenticated, Some(Unauthenticated::Accepted));
        assert_eq!(
            serde_json::to_value(&s).unwrap()["unauthenticated"],
            "accepted"
        );
        let mut v = spec("feed");
        v["unauthenticated"] = json!(true);
        assert!(
            serde_json::from_value::<SourceSpec>(v).is_err(),
            "only \"accepted\""
        );
    }

    #[test]
    fn a_grpc_server_authenticates_with_a_token_or_mutual_tls() {
        let batch = "acme.tracks.v1.TrackBatch";
        let e = check(grpc_spec(
            json!({"type": "grpc_server", "bind": "0.0.0.0:50051"}),
            batch,
        ))
        .unwrap_err();
        assert!(e.contains("bearer token"), "{e}");
        let mut v = grpc_spec(
            json!({"type": "grpc_server", "bind": "0.0.0.0:50051"}),
            batch,
        );
        v["unauthenticated"] = json!("accepted");
        check(v).unwrap();
        check(grpc_spec(
            json!({"type": "grpc_server", "bind": "0.0.0.0:50051",
                   "tls": {"cert_file": "s.pem", "key_file": "s.key", "client_ca_file": "ca.pem"}}),
            batch,
        ))
        .unwrap();
    }

    /// A gRPC source over the test producer's schema (see `proto::tests`).
    fn grpc_spec(transport: serde_json::Value, message: &str) -> serde_json::Value {
        json!({
            "id": "acme", "name": "ACME feed", "transport": transport,
            "pipeline": {
                "codec": { "type": "protobuf", "files": crate::proto::tests::files(),
                           "message": message, "records": "tracks" },
                "mapping": { "rules": [ { "name": "track", "key": "track_id", "fields": {
                    "position.latitude": "position.lat", "position.longitude": "position.lon" } } ] }
            }
        })
    }

    fn check(v: serde_json::Value) -> Result<(), String> {
        let s: SourceSpec = serde_json::from_value(v).map_err(|e| e.to_string())?;
        s.validate().map_err(|e| e.to_string())
    }

    #[test]
    fn a_grpc_source_must_fit_its_protobuf_schema() {
        let client = |method: &str, request: serde_json::Value| {
            json!({"type": "grpc_client", "url": "https://feed.acme.example:443", "method": method, "request": request,
                   "metadata": {"authorization": "Bearer ${env:ACME_TOKEN}"}})
        };
        let batch = "acme.tracks.v1.TrackBatch";
        check(grpc_spec(
            client("acme.tracks.v1.TrackFeed/Stream", json!({"area": "solent"})),
            batch,
        ))
        .unwrap();
        check(grpc_spec(
            json!({"type": "grpc_server", "bind": "0.0.0.0:50051", "token": "${env:ACME_TOKEN}"}),
            batch,
        ))
        .unwrap();
        let e = check(grpc_spec(
            client("acme.tracks.v1.TrackFeed/Nope", json!(null)),
            batch,
        ))
        .unwrap_err();
        assert!(e.contains("no method"), "{e}");
        let e = check(grpc_spec(
            client("acme.tracks.v1.TrackFeed/Stream", json!(null)),
            "acme.tracks.v1.Track",
        ))
        .unwrap_err();
        assert!(e.contains("answers acme.tracks.v1.TrackBatch"), "{e}");
        let e = check(grpc_spec(
            client("acme.tracks.v1.TrackFeed/Stream", json!({"area": 7})),
            batch,
        ))
        .unwrap_err();
        assert!(e.contains("request"), "{e}");
        let e = check(grpc_spec(
            json!({"type": "grpc_server", "bind": "0.0.0.0:50051", "methods": ["acme.tracks.v1.TrackFeed/Stream"]}),
            batch,
        ))
        .unwrap_err();
        assert!(e.contains("takes acme.tracks.v1.Subscribe"), "{e}");
        let e = check(grpc_spec(json!({"type": "grpc_client", "url": "feed:443", "method": "acme.tracks.v1.TrackFeed/Stream"}), batch))
            .unwrap_err();
        assert!(e.contains("http://"), "{e}");
        let e = check(grpc_spec(
            json!({"type": "grpc_client", "url": "http://f:1", "method": "acme.tracks.v1.TrackFeed/Stream",
                   "metadata": {"grpc-timeout": "1S"}}),
            batch,
        ))
        .unwrap_err();
        assert!(e.contains("metadata"), "{e}");
        // gRPC needs the protobuf codec.
        let mut v = grpc_spec(
            json!({"type": "grpc_server", "bind": "0.0.0.0:50051"}),
            batch,
        );
        v["pipeline"]["codec"] = json!({"type": "json"});
        assert!(check(v).unwrap_err().contains("protobuf codec"));
        // A broken .proto is caught when the source is saved.
        let mut v = grpc_spec(
            json!({"type": "grpc_server", "bind": "0.0.0.0:50051"}),
            batch,
        );
        v["pipeline"]["codec"]["files"]["tracks.proto"] = json!("syntax = \"proto3\"; message {");
        assert!(check(v).unwrap_err().contains("tracks.proto:1"));
    }

    #[tokio::test]
    async fn producers_push_through_a_grpc_server_source_to_records() {
        let v = grpc_spec(
            json!({"type": "grpc_server", "bind": "127.0.0.1:0", "token": "abc"}),
            "acme.tracks.v1.TrackBatch",
        );
        let spec: SourceSpec = serde_json::from_value(v).unwrap();
        spec.validate().unwrap();
        // Bind a known free port for the test.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut transport = spec.transport.clone();
        if let TransportConfig::GrpcServer { bind, .. } = &mut transport {
            *bind = format!("127.0.0.1:{port}");
        }
        let proto = crate::transport::ProtoContext::of(&spec.pipeline.codec).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            let _ = crate::transport::run(&transport, proto.as_ref(), tx, Default::default()).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // A producer pushes one batch of two tracks.
        let set = crate::proto::ProtoSet::compile(&crate::proto::tests::files()).unwrap();
        let batch = set.message("acme.tracks.v1.TrackBatch").unwrap();
        let push = crate::grpc::Subscription {
            url: format!("http://127.0.0.1:{port}"),
            path: "/acme.tracks.v1.TrackFeed/Push".into(),
            request: set
                .encode(
                    &batch,
                    &json!({"tracks": [
                    {"track_id": "V1", "position": {"lat": 50.8, "lon": -1.1}},
                    {"track_id": "V2", "position": {"lat": 50.7, "lon": -1.3}}]}),
                )
                .unwrap(),
            metadata: [("authorization".to_owned(), "Bearer abc".to_owned())].into(),
            tls: None,
            max_message: 1 << 16,
            keepalive: None,
            connect_timeout: std::time::Duration::from_secs(5),
        };
        let ended = crate::grpc::subscribe(&push, || {}, |_ack| async { Ok(()) })
            .await
            .unwrap();
        assert_eq!(ended.messages, 1, "the Ack");
        let frame = rx.recv().await.unwrap();
        let mut codec = crate::codec::Codec::new(spec.pipeline.codec.clone()).unwrap();
        let records = codec.decode(&frame).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1]["track_id"], "V2");
        assert_eq!(records[1]["position"]["lon"], -1.3);
        assert_eq!(
            records[0]["_frame"]["method"],
            "/acme.tracks.v1.TrackFeed/Push"
        );
    }
}
