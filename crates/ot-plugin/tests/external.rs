//! External plugins: the Python SDK's example scorer served over a socket,
//! a plugin that stops answering, and the handshake that authenticates
//! both ends (with an in-process fake plugin).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ot_core::Observation;
use ot_plugin::handshake::{self, Role};
use ot_plugin::{AuthFailed, External, Grants, NoSecret, Source};
use ot_source::plugin::ScoreCandidate;
use serde_json::{Value, json};

const SECRET: &str = "4f1c0e9a7b2d6e3f5a8c1b0d9e7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f";

struct Served(Child);

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// sdk/python/examples/domain_scorer.py, served; None without python3.
fn serve_python_scorer() -> Option<(Served, String)> {
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/python");
    let address = format!("127.0.0.1:{}", free_port());
    let mut child = Command::new("python3")
        .arg(sdk.join("examples/domain_scorer.py"))
        .args(["serve", "--address", &address])
        .env("PYTHONPATH", &sdk)
        .env("OT_PLUGIN_SECRET", SECRET)
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    // It says when it is listening.
    let mut line = String::new();
    BufReader::new(child.stderr.take()?)
        .read_line(&mut line)
        .ok()?;
    if !line.contains("serving") {
        eprintln!("skipped: the Python example did not start: {line}");
        let _ = child.kill();
        return None;
    }
    Some((Served(child), address))
}

fn obs(domain: &str, course: f64) -> Observation {
    serde_json::from_value(json!({
        "schema_version": 2, "source_id": "s", "source_track_key": "k",
        "observed_at": "2026-09-26T12:00:00Z", "received_at": "2026-09-26T12:00:00Z",
        "position": {"latitude": 36.9, "longitude": -76.2},
        "kinematics": {"course_deg": course, "speed_mps": 8.0},
        "classification": {"domain": domain}
    }))
    .unwrap()
}

#[test]
fn a_python_scorer_served_over_a_socket() {
    let Some((_server, address)) = serve_python_scorer() else {
        return;
    };
    // Without the secret it is refused; with it, it works.
    let wrong = SECRET.replace('4', "5");
    let e = err(ot_plugin::load(
        &external(&address, Some(&wrong)),
        &Grants::default(),
    ));
    assert!(e.chain().any(|c| c.is::<AuthFailed>()), "{e:#}");
    let p = ot_plugin::load(&external(&address, Some(SECRET)), &Grants::default()).unwrap();
    assert_eq!(p.manifest().name, "domain-scorer");
    assert!(p.decoder(&json!({})).is_err(), "not a codec");
    let mut s = p.scorer(&json!({"max_course_diff_deg": 45.0})).unwrap();
    let k = json!({"ln_lr": 3.0, "pass": true});
    let candidate = |o: Observation| ScoreCandidate {
        view: serde_json::to_value(o).unwrap(),
        kinematic: k.clone(),
    };
    let scores = s
        .score(
            &obs("surface", 90.0),
            &[
                candidate(obs("air", 90.0)),
                candidate(obs("surface", 200.0)),
                candidate(obs("surface", 100.0)),
            ],
        )
        .unwrap();
    assert_eq!(scores[0].evidence["veto"], "domain");
    assert_eq!(scores[1].evidence["veto"], "course");
    assert!(
        scores[2].pass && (scores[2].ln_lr - 3.0).abs() < 1e-9,
        "{scores:?}"
    );
}

/// An external plugin reached with `secret` (and no data directory).
fn external(address: &str, secret: Option<&str>) -> Source {
    Source::External(External::new(address, secret.map(str::to_owned)))
}

/// How the fake plugin answers the handshake.
#[derive(Clone, Copy, PartialEq)]
enum Hello {
    /// With this secret.
    Secret(&'static str),
    /// Never.
    Silent,
    /// It does not know the method (a plugin from before 0.4.5).
    Unknown,
}

/// What the fake saw.
#[derive(Default)]
struct Seen {
    connections: AtomicUsize,
    /// OpenTrack's proofs that checked out.
    verified: AtomicUsize,
}

/// A tracker plugin on one connection: the handshake as `hello` says, then
/// describe and open; `run` never answers.
fn fake_session(
    stream: impl Read + Write + Send + 'static,
    r: Box<dyn Read + Send>,
    hello: Hello,
    seen: &Seen,
) {
    seen.connections.fetch_add(1, Ordering::SeqCst);
    let mut r = BufReader::new(r);
    let mut w = stream;
    let mut line = String::new();
    let mut challenges: Option<(Vec<u8>, Vec<u8>)> = None;
    let mut authenticated = !matches!(hello, Hello::Secret(_));
    while r.read_line(&mut line).unwrap_or(0) > 0 {
        let req: Value = serde_json::from_str(&line).unwrap();
        line.clear();
        let method = req["method"].as_str().unwrap_or_default();
        let reply = match (method, hello) {
            ("hello", Hello::Silent) => {
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
            ("hello", Hello::Unknown) => json!({"error": "unknown method 'hello'"}),
            ("hello", Hello::Secret(secret)) => {
                let theirs = B64
                    .decode(req["params"]["challenge"].as_str().unwrap())
                    .unwrap();
                let ours = handshake::nonce().to_vec();
                let proof = handshake::proof(secret, Role::Plugin, &theirs, &ours);
                let reply =
                    json!({"result": {"proof": B64.encode(proof), "challenge": B64.encode(&ours)}});
                challenges = Some((theirs, ours));
                reply
            }
            ("verify", Hello::Secret(secret)) => {
                let (c, p) = challenges.take().unwrap();
                let tag = B64
                    .decode(req["params"]["proof"].as_str().unwrap())
                    .unwrap();
                if handshake::verify(secret, Role::OpenTrack, &c, &p, &tag) {
                    seen.verified.fetch_add(1, Ordering::SeqCst);
                    authenticated = true;
                    json!({"result": null})
                } else {
                    json!({"error": "OpenTrack's proof is wrong"})
                }
            }
            _ if !authenticated => json!({"error": "not authenticated"}),
            ("describe", _) => {
                json!({"result": {"name": "fake", "version": "1", "kinds": ["tracker"]}})
            }
            ("open", _) => json!({"result": null}),
            _ => {
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
        };
        let mut reply = reply;
        reply["id"] = req["id"].clone();
        if writeln!(w, "{reply}").is_err() {
            return;
        }
    }
}

/// The fake plugin on TCP.
fn fake_tcp(hello: Hello) -> (String, Arc<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let seen = Arc::new(Seen::default());
    let s = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let s = s.clone();
            std::thread::spawn(move || {
                let r = Box::new(stream.try_clone().unwrap());
                fake_session(stream, r, hello, &s);
            });
        }
    });
    (address, seen)
}

/// The fake plugin on a unix socket at `path`.
fn fake_unix(path: &Path, hello: Hello) -> Arc<Seen> {
    let listener = UnixListener::bind(path).unwrap();
    let seen = Arc::new(Seen::default());
    let s = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let s = s.clone();
            std::thread::spawn(move || {
                let r = Box::new(stream.try_clone().unwrap());
                fake_session(stream, r, hello, &s);
            });
        }
    });
    seen
}

fn short() -> Grants {
    Grants {
        call_timeout_ms: 300,
        ..Grants::default()
    }
}

/// The error of a load that must fail.
fn err<T>(r: anyhow::Result<T>) -> anyhow::Error {
    match r {
        Ok(_) => panic!("loaded, and should not have"),
        Err(e) => e,
    }
}

fn is<E: std::error::Error + 'static>(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<E>())
}

#[test]
fn a_plugin_that_stops_answering_times_out() {
    // Answers the handshake, describe and open, then never again.
    let (address, _) = fake_tcp(Hello::Secret(SECRET));
    let p = ot_plugin::load(&external(&address, Some(SECRET)), &short()).unwrap();
    let mut t = p.tracker(&json!({})).unwrap();
    let started = std::time::Instant::now();
    let e = t.run(chrono::Utc::now(), false).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(2), "{e}");
    // The session is not used again.
    let again = t.run(chrono::Utc::now(), false).unwrap_err();
    assert!(
        again.contains("stopped after an earlier failure"),
        "{again}"
    );
}

#[test]
fn the_right_secret_authenticates_both_ends_on_every_connection() {
    let (address, seen) = fake_tcp(Hello::Secret(SECRET));
    let p = ot_plugin::load(&external(&address, Some(SECRET)), &short()).unwrap();
    assert_eq!(p.manifest().name, "fake");
    let _t = p.tracker(&json!({})).unwrap();
    let _u = p.tracker(&json!({})).unwrap();
    // describe, and each session: every one checked OpenTrack's proof.
    assert_eq!(seen.connections.load(Ordering::SeqCst), 3);
    assert_eq!(seen.verified.load(Ordering::SeqCst), 3);
}

#[test]
fn a_secret_from_the_environment_is_resolved() {
    // Setting a variable needs `unsafe`; PATH is set, and long enough.
    let path = std::env::var("PATH").unwrap_or_default();
    if path.len() < handshake::MIN_SECRET_LEN {
        return;
    }
    let (address, seen) = fake_tcp(Hello::Secret(path.leak()));
    let p = ot_plugin::load(&external(&address, Some("${env:PATH}")), &short()).unwrap();
    assert_eq!(p.manifest().name, "fake");
    assert_eq!(seen.verified.load(Ordering::SeqCst), 1);
}

#[test]
fn a_wrong_secret_is_refused() {
    // The plugin has another secret: its proof does not check.
    let (address, seen) = fake_tcp(Hello::Secret(SECRET));
    let other = "another secret, just as long as the first one is";
    let e = err(ot_plugin::load(&external(&address, Some(other)), &short()));
    assert!(is::<AuthFailed>(&e), "{e:#}");
    assert!(format!("{e:#}").contains("proof is wrong"), "{e:#}");
    // And OpenTrack never proved itself with the wrong one.
    assert_eq!(seen.verified.load(Ordering::SeqCst), 0);
    // A secret too short to use is refused before connecting.
    let e = err(ot_plugin::load(
        &external(&address, Some("short")),
        &short(),
    ));
    assert!(format!("{e:#}").contains("at least 32"), "{e:#}");
}

#[test]
fn a_plugin_that_does_not_answer_the_handshake_is_refused() {
    for hello in [Hello::Silent, Hello::Unknown] {
        let (address, _) = fake_tcp(hello);
        let started = std::time::Instant::now();
        let e = err(ot_plugin::load(&external(&address, Some(SECRET)), &short()));
        assert!(is::<AuthFailed>(&e), "{e:#}");
        assert!(format!("{e:#}").contains("no answer to hello"), "{e:#}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn no_secret_over_tcp_is_refused_without_connecting() {
    let (address, seen) = fake_tcp(Hello::Unknown);
    let e = err(ot_plugin::load(&external(&address, None), &short()));
    assert!(is::<NoSecret>(&e), "{e:#}");
    assert_eq!(seen.connections.load(Ordering::SeqCst), 0);
}

#[test]
fn a_unix_socket_under_the_data_directory_needs_no_secret() {
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir(data.path().join("plugins")).unwrap();
    let path = data.path().join("plugins/fake.sock");
    let seen = fake_unix(&path, Hello::Unknown);
    let source = Source::External(External {
        address: format!("unix:{}", path.display()),
        secret: None,
        data_dir: Some(data.path().to_path_buf()),
    });
    let p = ot_plugin::load(&source, &short()).unwrap();
    assert_eq!(p.manifest().name, "fake");
    assert_eq!(seen.connections.load(Ordering::SeqCst), 1);
}

#[test]
fn a_unix_socket_elsewhere_needs_a_secret() {
    let data = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let path = elsewhere.path().join("fake.sock");
    let seen = fake_unix(&path, Hello::Unknown);
    // A link from the data directory to it does not count either.
    std::os::unix::fs::symlink(&path, data.path().join("link.sock")).unwrap();
    for address in [path.clone(), data.path().join("link.sock")] {
        let source = Source::External(External {
            address: format!("unix:{}", address.display()),
            secret: None,
            data_dir: Some(data.path().to_path_buf()),
        });
        let e = err(ot_plugin::load(&source, &short()));
        assert!(is::<NoSecret>(&e), "{e:#}");
    }
    assert_eq!(seen.connections.load(Ordering::SeqCst), 0);
    // With a secret, a socket anywhere authenticates as TCP does.
    let path = elsewhere.path().join("auth.sock");
    let seen = fake_unix(&path, Hello::Secret(SECRET));
    let source = Source::External(External {
        address: format!("unix:{}", path.display()),
        secret: Some(SECRET.into()),
        data_dir: Some(data.path().to_path_buf()),
    });
    ot_plugin::load(&source, &short()).unwrap();
    assert_eq!(seen.verified.load(Ordering::SeqCst), 1);
}
