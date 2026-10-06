//! Probing a candidate source: connect with its transport for a bounded time
//! or number of frames, decode what arrives, infer the schema and suggest a
//! mapping. Captured frames can be kept for mapping preview.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use ot_source::codec::{Codec, CodecConfig};
use ot_source::frame::Frame;
use ot_source::probe::{infer, suggest, suggest_records_path};
use ot_source::transport::{self, LinkStatus, SharedStatus, TransportConfig};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::api::actor;
use crate::control::{ApiError, AppState};

const MAX_FRAMES: usize = 2000;
const MAX_SECS: f64 = 120.0;
/// Records analysed at most (inference cost is linear in this).
const MAX_RECORDS: usize = 5000;
/// Frames echoed back to the UI, each truncated.
const ECHO_FRAMES: usize = 20;
const ECHO_BYTES: usize = 4096;

#[derive(Deserialize)]
pub struct ProbeRequest {
    pub transport: TransportConfig,
    pub codec: CodecConfig,
    /// Stop after this many frames (default 200, max 2000).
    #[serde(default)]
    pub max_frames: Option<usize>,
    /// Stop after this many seconds (default 30, max 120).
    #[serde(default)]
    pub max_secs: Option<f64>,
    /// Keep the captured frames as this source's samples.
    #[serde(default)]
    pub save_as: Option<String>,
}

/// A `file` transport may only read under the data directory (beside the
/// database): elsewhere it could read any file the server can (ASD
/// V-222466). Other transports pass.
pub(crate) fn file_allowed(
    common: &crate::config::Common,
    t: &TransportConfig,
) -> Result<(), String> {
    let TransportConfig::File { path, .. } = t else {
        return Ok(());
    };
    let data = common
        .sqlite
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let data = std::fs::canonicalize(data).map_err(|e| format!("the data directory: {e}"))?;
    let p = std::path::Path::new(path.trim());
    if p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!("file path {path:?}: `..` is not allowed"));
    }
    let p = if p.is_absolute() {
        p.to_path_buf()
    } else {
        data.join(p)
    };
    // An existing path resolves its links; a missing one is checked as written.
    let p = std::fs::canonicalize(&p).unwrap_or(p);
    if p.starts_with(&data) {
        Ok(())
    } else {
        Err(format!(
            "file path {path:?}: a file source reads only under the data directory ({})",
            data.display()
        ))
    }
}

/// Capture frames from a transport until either limit is reached.
async fn capture(
    t: &TransportConfig,
    proto: Option<transport::ProtoContext>,
    max_frames: usize,
    max_secs: f64,
) -> (Vec<Frame>, Option<String>, LinkStatus) {
    let (tx, mut rx) = mpsc::channel::<Frame>(max_frames.max(1));
    let status = SharedStatus::default();
    let t = t.clone();
    let task_status = status.clone();
    let task =
        tokio::spawn(async move { transport::run(&t, proto.as_ref(), tx, task_status).await });
    let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(max_secs);
    let mut frames = Vec::new();
    let mut ended: Option<String> = None;
    while frames.len() < max_frames {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(f)) => frames.push(f),
            Ok(None) => break,
            Err(_) => break,
        }
    }
    if task.is_finished() {
        if let Ok(Err(e)) = task.await {
            ended = Some(format!("{e:#}"));
        }
    } else {
        task.abort();
    }
    let link = status.lock().map(|s| s.clone()).unwrap_or_default();
    let last_error = ended.or_else(|| link.last_error.clone());
    (frames, last_error, link)
}

/// What to tell the user when a probe captured no frames and no error explains why.
fn no_frames_hint(t: &TransportConfig, link: &LinkStatus, secs: f64) -> Option<String> {
    let stream = matches!(
        t,
        TransportConfig::TcpClient { .. } | TransportConfig::TcpServer { .. }
    );
    if link.bytes_in > 0 {
        return Some(format!(
            "Received {} bytes in {secs:.0} s but no complete frame: check the framing \
             (Cursor-on-Target streams are framed by the end tag </event>).",
            link.bytes_in
        ));
    }
    if !link.connected && link.connects == 0 {
        return Some(format!("Never connected in {secs:.0} s."));
    }
    match t {
        TransportConfig::TcpClient { tls: None, .. } => Some(format!(
            "Connected, but nothing arrived in {secs:.0} s. The feed may be idle; the port may \
             need TLS (a TLS server waits silently for a handshake); or it may only take data in \
             (a TAK Server input port never sends)."
        )),
        _ if stream || link.connected => Some(format!(
            "Connected, but nothing arrived in {secs:.0} s: the feed may be idle, or may only take data in."
        )),
        _ => None,
    }
}

/// Where a transport reads from, for the audit record (credentials in
/// URLs hidden).
fn target(t: &TransportConfig) -> Value {
    let v = serde_json::to_value(t).unwrap_or_default();
    let pick = [
        "path", "url", "address", "host", "bind", "group", "endpoint",
    ]
    .iter()
    .find_map(|k| v.get(*k).and_then(Value::as_str).map(str::to_owned));
    pick.map_or(Value::Null, |p| json!(ot_source::secrets::redact_url(&p)))
}

/// Record a probe in the audit record: who, which transport and where,
/// how it went (ASD V-222466).
async fn audit_probe(s: &AppState, actor: &str, what: &Value, ok: bool, error: Option<&str>) {
    let mut detail = what.clone();
    if let Some(e) = error {
        detail["error"] = json!(e);
    }
    let mut e = ot_store::audit::AuditEvent::new(actor, "probe_source").detail(detail);
    if !ok {
        e = e.failure();
    }
    if let Err(err) = s.with_db(move |db| db.audit(&e)).await {
        tracing::warn!(error = %err.message, "probe not audited");
    }
}

pub async fn probe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<ProbeRequest>,
) -> Result<Json<Value>, ApiError> {
    let max_frames = req.max_frames.unwrap_or(200).clamp(1, MAX_FRAMES);
    let max_secs = req.max_secs.unwrap_or(30.0).clamp(1.0, MAX_SECS);
    if let Some(id) = &req.save_as
        && (id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'))
    {
        return Err(ApiError::unprocessable("save_as must be a valid source id"));
    }
    let admin = actor(&headers);
    let what = json!({ "transport": req.transport.kind(), "target": target(&req.transport) });
    if let Err(e) = file_allowed(&s.common, &req.transport) {
        audit_probe(&s, &admin, &what, false, Some(&e)).await;
        return Err(ApiError::forbidden(e));
    }
    let started = std::time::Instant::now();
    // A gRPC probe needs the protobuf codec's schema.
    let proto = match transport::ProtoContext::of(&req.codec) {
        Ok(p) => p,
        Err(e) => return Err(ApiError::unprocessable(format!("protobuf: {e}"))),
    };
    let (frames, link_error, link) = capture(&req.transport, proto, max_frames, max_secs).await;
    let elapsed = started.elapsed().as_secs_f64();
    let hint = if frames.is_empty() && link_error.is_none() {
        no_frames_hint(&req.transport, &link, elapsed)
    } else {
        None
    };
    audit_probe(
        &s,
        &admin,
        &json!({ "transport": what["transport"], "target": what["target"], "frames": frames.len() }),
        link_error.is_none() || !frames.is_empty(),
        link_error.as_deref(),
    )
    .await;

    // For JSON without a record path, see whether frames wrap an array.
    let mut codec_cfg = req.codec.clone();
    let mut suggested_records = None;
    if let CodecConfig::Json {
        records: None,
        context,
    } = &req.codec
    {
        let parsed: Vec<Value> = frames
            .iter()
            .take(50)
            .filter_map(|f| serde_json::from_slice(&f.bytes).ok())
            .collect();
        if let Some(p) = suggest_records_path(&parsed) {
            codec_cfg = CodecConfig::Json {
                records: Some(p.parse().map_err(|e| ApiError::internal(format!("{e}")))?),
                context: context.clone(),
            };
            suggested_records = Some(p);
        }
    }
    let mut codec =
        Codec::new(codec_cfg.clone()).map_err(|e| ApiError::unprocessable(e.to_string()))?;
    let mut records = Vec::new();
    let mut decode_errors = 0;
    let mut last_decode_error = None;
    for f in &frames {
        match codec.decode(f) {
            Ok(r) => records.extend(r),
            Err(e) => {
                decode_errors += 1;
                last_decode_error = Some(e.to_string());
            }
        }
        if records.len() >= MAX_RECORDS {
            records.truncate(MAX_RECORDS);
            break;
        }
    }
    let fields = infer(&records);
    let suggestion = suggest(&fields);

    let mut saved = None;
    if let Some(id) = req.save_as.clone() {
        let samples: Vec<(Vec<u8>, Option<Value>)> = frames
            .iter()
            .map(|f| {
                let meta = (!f.meta.is_empty()).then(|| Value::Object(f.meta.clone()));
                (f.bytes.to_vec(), meta)
            })
            .collect();
        let n = s
            .with_db(move |db| {
                db.store_probe_samples(&id, &samples, ot_store::probe::DEFAULT_SAMPLE_CAP)
            })
            .await?;
        saved = Some(n);
    }

    let echo: Vec<Value> = frames
        .iter()
        .take(ECHO_FRAMES)
        .map(|f| {
            let text = String::from_utf8_lossy(&f.bytes);
            let truncated = text.len() > ECHO_BYTES;
            let mut end = ECHO_BYTES.min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            json!({ "received_at": f.received_at, "origin": f.origin, "bytes": f.bytes.len(),
                    "meta": (!f.meta.is_empty()).then_some(&f.meta),
                    "text": &text[..end], "truncated": truncated })
        })
        .collect();

    Ok(Json(json!({
        "frames": frames.len(),
        "seconds": (elapsed * 10.0).round() / 10.0,
        "link_error": link_error,
        "bytes_received": link.bytes_in,
        "hint": hint,
        "decode_errors": decode_errors,
        "last_decode_error": last_decode_error,
        "records": records.len(),
        "codec": codec_cfg,
        "suggested_records_path": suggested_records,
        "fields": fields,
        "suggestion": suggestion,
        "samples_saved": saved,
        "sample_frames": echo,
    })))
}

#[derive(Deserialize)]
pub struct SamplesQuery {
    limit: Option<usize>,
}

/// A source's stored sample frames, as text.
pub async fn samples(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<SamplesQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = q
        .limit
        .unwrap_or(50)
        .min(ot_store::probe::DEFAULT_SAMPLE_CAP);
    let rows = s.with_db(move |db| db.probe_samples(&id, limit)).await?;
    let frames: Vec<Value> = rows
        .into_iter()
        .map(|s| {
            json!({ "captured_at_ms": s.captured_at_ms, "meta": s.meta,
                    "text": String::from_utf8_lossy(&s.bytes) })
        })
        .collect();
    Ok(Json(json!({ "samples": frames })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tcp(tls: bool) -> TransportConfig {
        serde_json::from_value(json!({"type": "tcp_client", "host": "h", "port": 1,
            "tls": if tls { json!({}) } else { Value::Null }}))
        .unwrap()
    }

    #[test]
    fn a_silent_probe_says_why_it_may_be() {
        let connected = LinkStatus {
            connected: true,
            connects: 1,
            ..Default::default()
        };
        let plain = no_frames_hint(&tcp(false), &connected, 10.0).unwrap();
        assert!(
            plain.contains("need TLS") && plain.contains("TAK Server input"),
            "{plain}"
        );
        let tls = no_frames_hint(&tcp(true), &connected, 10.0).unwrap();
        assert!(
            tls.starts_with("Connected, but nothing arrived") && !tls.contains("TLS"),
            "{tls}"
        );
        let unframed = LinkStatus {
            bytes_in: 512,
            ..connected.clone()
        };
        let hint = no_frames_hint(&tcp(false), &unframed, 10.0).unwrap();
        assert!(
            hint.contains("512 bytes") && hint.contains("</event>"),
            "{hint}"
        );
        let never = no_frames_hint(&tcp(false), &LinkStatus::default(), 10.0).unwrap();
        assert_eq!(never, "Never connected in 10 s.");
    }

    #[tokio::test]
    async fn capture_stops_at_the_frame_limit() {
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let t = TransportConfig::Udp {
            bind: addr.to_string(),
            multicast_group: None,
            multicast_interface: None,
        };
        let sender = tokio::spawn(async move {
            let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            for i in 0..200 {
                let _ = sock
                    .send_to(format!("{{\"id\":{i}}}").as_bytes(), addr)
                    .await;
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        let (frames, err, _) = capture(&t, None, 5, 10.0).await;
        sender.abort();
        assert_eq!(frames.len(), 5);
        assert!(err.is_none());
    }

    #[tokio::test]
    async fn capture_reports_connection_failures() {
        let t = TransportConfig::TcpClient {
            host: "127.0.0.1".into(),
            port: 9,
            framing: ot_source::frame::Framing::Lines { max_len: 1024 },
            send_on_connect: None,
            tls: None,
        };
        let (frames, err, _) = capture(&t, None, 5, 2.0).await;
        assert!(frames.is_empty());
        assert!(err.unwrap().contains("connecting"));
    }

    #[test]
    fn a_file_source_reads_only_under_the_data_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("replays")).unwrap();
        std::fs::write(dir.path().join("replays/a.jsonl"), "{}").unwrap();
        let common = crate::config_backup::tests::common(dir.path());
        let file = |path: &str| TransportConfig::File {
            path: path.into(),
            extension: None,
            framing: ot_source::frame::Framing::Message,
            frames_per_second: None,
            repeat: false,
        };
        let inside = dir.path().join("replays/a.jsonl");
        assert!(file_allowed(&common, &file(inside.to_str().unwrap())).is_ok());
        assert!(
            file_allowed(&common, &file("replays")).is_ok(),
            "relative to the data directory"
        );
        assert!(file_allowed(&common, &file("/etc/passwd")).is_err());
        assert!(file_allowed(&common, &file("replays/../../x")).is_err());
        // A link out of the data directory is followed, and refused.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", dir.path().join("etc")).unwrap();
            assert!(file_allowed(&common, &file("etc/passwd")).is_err());
        }
    }
}
