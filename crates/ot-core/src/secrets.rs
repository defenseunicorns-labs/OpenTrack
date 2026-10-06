//! Hiding a source's secrets from those who may read its specification but
//! not change it (viewers and track managers): passwords, tokens, header
//! and metadata values, credentials in URLs and API keys inside messages a
//! transport sends. `${env:NAME}` references are not secrets and stay.

use serde_json::Value;

/// What a hidden value reads as.
pub const HIDDEN: &str = "••••••";

/// Names whose values are secrets, wherever they appear in a transport.
fn secret_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase().replace(['-', '_'], "");
    [
        "pass",
        "secret",
        "token",
        "auth",
        "apikey",
        "credential",
        "cookie",
        "session",
        "signature",
        "privatekey",
        "bearer",
    ]
    .iter()
    .any(|s| n.contains(s))
}

fn is_env_ref(s: &str) -> bool {
    let s = s.trim();
    s.starts_with("${env:") && s.ends_with('}') && !s[2..].contains('$')
}

/// Hide the secrets in a source specification (a `SourceSpec` as JSON).
pub fn redact_spec(spec: &mut Value) {
    if let Some(t) = spec.get_mut("transport") {
        redact_transport(t);
    }
}

fn redact_transport(v: &mut Value) {
    let Value::Object(map) = v else { return };
    for (k, v) in map.iter_mut() {
        match (k.as_str(), v) {
            // Every value of headers and gRPC metadata: they carry
            // credentials under names of every kind.
            ("headers" | "metadata", Value::Object(m)) => {
                for x in m.values_mut() {
                    hide(x);
                }
            }
            (k, x) if secret_name(k) => {
                hide(x);
            }
            (k, Value::String(s)) if k == "url" || k.ends_with("_url") => *s = redact_url(s),
            // Messages a transport sends (a subscription, a request body):
            // JSON as a value or as text.
            (_, Value::String(s)) => {
                if let Ok(mut j @ (Value::Object(_) | Value::Array(_))) =
                    serde_json::from_str::<Value>(s)
                    && redact_json(&mut j)
                {
                    *s = j.to_string();
                }
            }
            (_, x @ (Value::Object(_) | Value::Array(_))) => {
                redact_json(x);
            }
            _ => {}
        }
    }
}

/// Hide secret-named fields anywhere in a JSON value; whether any were.
fn redact_json(v: &mut Value) -> bool {
    match v {
        Value::Object(m) => {
            let mut any = false;
            for (k, x) in m.iter_mut() {
                if secret_name(k) && !matches!(x, Value::Object(_) | Value::Array(_)) {
                    any |= hide(x);
                } else {
                    any |= redact_json(x);
                }
            }
            any
        }
        // Every element, not only up to the first with a secret.
        Value::Array(a) => {
            let mut any = false;
            for x in a {
                any |= redact_json(x);
            }
            any
        }
        _ => false,
    }
}

/// Replace a secret value (not an env reference, not empty); whether it did.
fn hide(v: &mut Value) -> bool {
    match v {
        Value::String(s) if s.is_empty() || is_env_ref(s) => false,
        Value::Null => false,
        _ => {
            *v = Value::String(HIDDEN.into());
            true
        }
    }
}

/// A URL without its password and with secret-named query values hidden.
pub fn redact_url(url: &str) -> String {
    let Ok(mut u) = url::Url::parse(url) else {
        return url.to_owned();
    };
    if u.password().is_some() {
        let _ = u.set_password(Some(HIDDEN));
    }
    if u.query().is_some() {
        let pairs: Vec<(String, String)> = u
            .query_pairs()
            .map(|(k, v)| {
                let v = if secret_name(&k) && !is_env_ref(&v) {
                    HIDDEN.to_owned()
                } else {
                    v.into_owned()
                };
                (k.into_owned(), v)
            })
            .collect();
        u.query_pairs_mut().clear().extend_pairs(pairs);
    }
    u.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn transport_secrets_are_hidden_and_env_refs_kept() {
        let mut spec = json!({
            "id": "ais",
            "pipeline": { "map": { "auth_note": "not a transport field" } },
            "transport": {
                "kind": "websocket",
                "url": "wss://user:hunter2@stream.example.org/v0?apikey=abc&region=gulf",
                "headers": { "X-Custom": "s3cret", "Accept": "application/json" },
                "subscribe": { "APIKey": "k-123", "BoundingBoxes": [[[1, 2], [3, 4]]] },
                "password": "${env:MQTT_PASSWORD}",
                "token": "raw-token",
                "tls": { "ca_file": "/etc/ca.pem", "key_file": "/etc/key.pem" }
            }
        });
        redact_spec(&mut spec);
        let t = &spec["transport"];
        let url = t["url"].as_str().unwrap();
        assert!(!url.contains("hunter2") && !url.contains("abc"), "{url}");
        assert!(url.contains("region=gulf") && url.contains("user"));
        assert_eq!(t["headers"]["X-Custom"], HIDDEN);
        assert_eq!(t["headers"]["Accept"], HIDDEN);
        assert_eq!(t["subscribe"]["APIKey"], HIDDEN);
        assert_eq!(t["subscribe"]["BoundingBoxes"], json!([[[1, 2], [3, 4]]]));
        assert_eq!(t["password"], "${env:MQTT_PASSWORD}");
        assert_eq!(t["token"], HIDDEN);
        assert_eq!(t["tls"]["key_file"], "/etc/key.pem");
        assert_eq!(
            spec["pipeline"]["map"]["auth_note"],
            "not a transport field"
        );
    }

    #[test]
    fn json_text_messages_are_redacted_too() {
        let mut spec = json!({ "transport": {
            "kind": "tcp_client",
            "send_on_connect": "{\"login\":{\"password\":\"pw\",\"user\":\"u\"}}",
            "body": "plain text stays"
        }});
        redact_spec(&mut spec);
        let s = spec["transport"]["send_on_connect"].as_str().unwrap();
        assert!(!s.contains("\"pw\"") && s.contains("\"u\""), "{s}");
        assert_eq!(spec["transport"]["body"], "plain text stays");
    }
}
