//! The three deliveries end to end on this host (a local "TAK Server", a
//! UDP receiver, TAK clients of the listener), and the outbox feeder against
//! a real Redis (`OT_TEST_REDIS_URL`; skipped without it).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};

use super::event::CotTrack;
use super::event::tests::{parse, track, uid};
use super::output::{self, Counters, Hub};
use super::settings::{Delivery, TakOutput};

const WAIT: Duration = Duration::from_secs(10);

type Elements = Vec<(String, BTreeMap<String, String>)>;

fn out(id: &str, delivery: Delivery) -> TakOutput {
    TakOutput {
        id: id.into(),
        enabled: true,
        stale_secs: 60.0,
        remarks: true,
        delivery,
    }
}

/// A track reported just now (the output sends nothing past its stale time).
fn cot(n: u64, lat: f64) -> CotTrack {
    let mut c = CotTrack::of(&track(
        n,
        serde_json::json!({
            "name": format!("TRACK {n}"),
            "position": {"latitude": lat, "longitude": -117.0},
            "classification": {"affiliation": "hostile", "domain": "air"}
        }),
    ));
    c.observed_at = chrono::Utc::now();
    c
}

fn hub_with(tracks: &[CotTrack]) -> Arc<Hub> {
    let hub = Arc::new(Hub::default());
    hub.load(tracks.iter().cloned());
    hub
}

fn start(o: TakOutput, hub: &Arc<Hub>) -> (Arc<Counters>, tokio::task::JoinHandle<()>) {
    let counters = Arc::new(Counters::default());
    let task = tokio::spawn(output::run(o, hub.clone(), counters.clone()));
    (counters, task)
}

fn free_port() -> SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

/// Reads whole events off a CoT stream.
struct Events<S> {
    stream: S,
    buf: Vec<u8>,
}

impl<S: AsyncRead + Unpin> Events<S> {
    fn new(stream: S) -> Self {
        Self {
            stream,
            buf: Vec::new(),
        }
    }

    async fn next(&mut self) -> Elements {
        tokio::time::timeout(WAIT, async {
            loop {
                if let Some(end) = self.buf.windows(8).position(|w| w == b"</event>") {
                    let event: Vec<u8> = self.buf.drain(..end + 8).collect();
                    return parse(&event);
                }
                let mut chunk = [0u8; 4096];
                let n = self.stream.read(&mut chunk).await.expect("read");
                assert!(n > 0, "stream ended");
                self.buf.extend_from_slice(&chunk[..n]);
            }
        })
        .await
        .expect("no event in time")
    }

    /// Events until one about `uid` of `kind` (`a-` or `t-x-d-d`) arrives.
    async fn until(&mut self, uid: &str, kind: &str) -> Elements {
        loop {
            let e = self.next().await;
            if e[0].1["uid"] == uid && e[0].1["type"].starts_with(kind) {
                return e;
            }
        }
    }
}

fn link_of(e: &Elements) -> &BTreeMap<String, String> {
    &e.iter().find(|(n, _)| n == "link").expect("a link").1
}

/// Test certificates: a CA, a server certificate for localhost and
/// 127.0.0.1, and a client certificate, all from the CA.
struct Pki {
    dir: tempfile::TempDir,
}

impl Pki {
    fn new() -> Self {
        use rcgen::{
            BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose,
            IsCa, KeyPair, KeyUsagePurpose,
        };
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, pem: String| std::fs::write(dir.path().join(name), pem).unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params
            .distinguished_name
            .push(DnType::CommonName, "OpenTrack TAK test CA");
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let ca = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
        write("ca.pem", ca.pem());
        let leaf = |name: &str, sans: Vec<String>, usage| {
            let mut params = CertificateParams::new(sans).unwrap();
            params.distinguished_name.push(DnType::CommonName, name);
            params.extended_key_usages = vec![usage];
            let key = KeyPair::generate().unwrap();
            let cert = params.signed_by(&key, &ca).unwrap();
            write(&format!("{name}.pem"), cert.pem());
            write(&format!("{name}.key"), key.serialize_pem());
        };
        leaf(
            "server",
            vec!["localhost".into(), "127.0.0.1".into()],
            ExtendedKeyUsagePurpose::ServerAuth,
        );
        leaf("client", vec![], ExtendedKeyUsagePurpose::ClientAuth);
        Self { dir }
    }

    fn path(&self, name: &str) -> String {
        self.dir.path().join(name).display().to_string()
    }

    fn server(&self, mutual: bool) -> ot_source::tls::ServerTls {
        ot_source::tls::ServerTls {
            cert_file: self.path("server.pem"),
            key_file: self.path("server.key"),
            client_ca_file: mutual.then(|| self.path("ca.pem")),
            client_cert_optional: false,
            client_crl_files: Vec::new(),
        }
    }

    fn client(&self, cert: bool) -> ot_source::tls::ClientTls {
        ot_source::tls::ClientTls {
            ca_file: Some(self.path("ca.pem")),
            cert_file: cert.then(|| self.path("client.pem")),
            key_file: cert.then(|| self.path("client.key")),
            ..Default::default()
        }
    }
}

#[tokio::test]
async fn pushes_to_a_tak_server_and_reconnects_with_the_picture() {
    let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = server.local_addr().unwrap().port();
    let hub = hub_with(&[cot(1, 32.0)]);
    let (counters, task) = start(
        out(
            "tak",
            Delivery::TakServer {
                host: "127.0.0.1".into(),
                port,
                tls: None,
            },
        ),
        &hub,
    );

    let (conn, _) = tokio::time::timeout(WAIT, server.accept())
        .await
        .unwrap()
        .unwrap();
    let mut events = Events::new(conn);
    // The picture first.
    let first = events.next().await;
    assert_eq!(first[0].0, "event");
    assert_eq!(
        (
            first[0].1["uid"].as_str(),
            first[0].1["type"].as_str(),
            first[0].1["how"].as_str()
        ),
        ("tms-OTK000000001", "a-h-A", "m-f")
    );
    // Then the stream: an update, a new track, a delete.
    hub.upsert(cot(2, 33.0));
    let e = events.until("tms-OTK000000002", "a-").await;
    let point = &e.iter().find(|(n, _)| n == "point").unwrap().1;
    assert_eq!(point["lat"], "33");
    hub.delete(uid(1));
    let d = events.until("tms-OTK000000001", "t-x-d-d").await;
    assert_eq!(link_of(&d)["uid"], "tms-OTK000000001");
    assert_eq!(link_of(&d)["type"], "a-h-A");
    assert_eq!(d[0].1["stale"], d[0].1["time"]);
    assert_eq!(counters.state().0, "connected");
    assert_eq!(
        counters.clients.load(std::sync::atomic::Ordering::Relaxed),
        1
    );

    // The server goes away and comes back: the picture again, without the
    // deleted track.
    drop(events);
    let (conn, _) = tokio::time::timeout(WAIT, server.accept())
        .await
        .unwrap()
        .unwrap();
    let mut events = Events::new(conn);
    let again = events.next().await;
    assert_eq!(again[0].1["uid"], "tms-OTK000000002");
    assert!(counters.sent.load(std::sync::atomic::Ordering::Relaxed) >= 4);
    task.abort();
}

#[tokio::test]
async fn pushes_to_a_tak_server_over_mutual_tls() {
    let pki = Pki::new();
    let acceptor = pki.server(true).acceptor().unwrap();
    let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = server.local_addr().unwrap().port();
    let hub = hub_with(&[cot(1, 32.0)]);
    let (_, task) = start(
        out(
            "tak-tls",
            Delivery::TakServer {
                host: "localhost".into(),
                port,
                tls: Some(pki.client(true)),
            },
        ),
        &hub,
    );
    let (tcp, _) = tokio::time::timeout(WAIT, server.accept())
        .await
        .unwrap()
        .unwrap();
    let tls = acceptor
        .accept(tcp)
        .await
        .expect("TLS with a client certificate");
    let cn = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|c| c.first())
        .and_then(|c| ot_source::tls::common_name(c));
    assert_eq!(cn.as_deref(), Some("client"));
    let mut events = Events::new(tls);
    assert_eq!(events.next().await[0].1["uid"], "tms-OTK000000001");
    hub.upsert(cot(3, 34.0));
    events.until("tms-OTK000000003", "a-").await;
    task.abort();

    // Without the client certificate the server refuses it, and the output
    // counts the failure and keeps trying.
    let hub = hub_with(&[cot(1, 32.0)]);
    let (counters, task) = start(
        out(
            "tak-tls",
            Delivery::TakServer {
                host: "localhost".into(),
                port,
                tls: Some(pki.client(false)),
            },
        ),
        &hub,
    );
    let (tcp, _) = tokio::time::timeout(WAIT, server.accept())
        .await
        .unwrap()
        .unwrap();
    assert!(acceptor.accept(tcp).await.is_err());
    tokio::time::timeout(WAIT, async {
        while counters.errors.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the refusal is counted");
    task.abort();
}

/// Receive datagrams on `socket` until one about `uid` of `kind`.
async fn datagram(socket: &tokio::net::UdpSocket, uid: &str, kind: &str) -> Elements {
    tokio::time::timeout(WAIT, async {
        let mut buf = vec![0u8; 65536];
        loop {
            let n = socket.recv(&mut buf).await.unwrap();
            // One whole event per datagram.
            let e = parse(&buf[..n]);
            assert_eq!(e[0].0, "event");
            if e[0].1["uid"] == uid && e[0].1["type"].starts_with(kind) {
                return e;
            }
        }
    })
    .await
    .expect("no datagram in time")
}

async fn udp_round_trip(group: &str, receiver: tokio::net::UdpSocket) {
    let port = receiver.local_addr().unwrap().port();
    let hub = hub_with(&[cot(1, 32.0)]);
    let (_, task) = start(
        out(
            "sa",
            Delivery::Multicast {
                group: group.into(),
                port,
                ttl: 1,
                interface: None,
            },
        ),
        &hub,
    );
    datagram(&receiver, "tms-OTK000000001", "a-").await;
    hub.upsert(cot(2, 33.0));
    datagram(&receiver, "tms-OTK000000002", "a-").await;
    hub.delete(uid(2));
    let d = datagram(&receiver, "tms-OTK000000002", "t-x-d-d").await;
    assert_eq!(link_of(&d)["uid"], "tms-OTK000000002");
    task.abort();
}

#[tokio::test]
async fn sends_udp_datagrams_to_a_unicast_address() {
    let receiver = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    udp_round_trip("127.0.0.1", receiver).await;
}

#[tokio::test]
async fn sends_multicast_sa() {
    use std::net::Ipv4Addr;
    let group = Ipv4Addr::new(239, 2, 3, 99);
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    if let Err(e) = socket.join_multicast_v4(&group, &Ipv4Addr::UNSPECIFIED) {
        eprintln!("skipped: no multicast here ({e})");
        return;
    }
    socket.set_nonblocking(true).unwrap();
    let receiver = tokio::net::UdpSocket::from_std(socket).unwrap();
    // A host with no multicast route cannot send to the group at all.
    let probe = output::udp_socket(
        SocketAddr::from((group, receiver.local_addr().unwrap().port())),
        1,
        None,
    )
    .unwrap();
    if let Err(e) = probe
        .send_to(b"probe", (group, receiver.local_addr().unwrap().port()))
        .await
    {
        eprintln!("skipped: no multicast route here ({e})");
        return;
    }
    let mut buf = [0u8; 16];
    if tokio::time::timeout(Duration::from_secs(1), receiver.recv(&mut buf))
        .await
        .is_err()
    {
        eprintln!("skipped: multicast is not looped back here");
        return;
    }
    udp_round_trip(&group.to_string(), receiver).await;
}

#[tokio::test]
async fn sends_multicast_sa_over_ipv6() {
    // Link-local scope, joined and sent on the system's default interface.
    let group: std::net::Ipv6Addr = "ff02::6969:99".parse().unwrap();
    let Ok(socket) = std::net::UdpSocket::bind("[::]:0") else {
        eprintln!("skipped: no IPv6 here");
        return;
    };
    if let Err(e) = socket.join_multicast_v6(&group, 0) {
        eprintln!("skipped: no IPv6 multicast here ({e})");
        return;
    }
    socket.set_nonblocking(true).unwrap();
    let receiver = tokio::net::UdpSocket::from_std(socket).unwrap();
    let port = receiver.local_addr().unwrap().port();
    let dest = SocketAddr::from((group, port));
    let probe = output::udp_socket(dest, 1, None).unwrap();
    if let Err(e) = probe.send_to(b"probe", dest).await {
        eprintln!("skipped: no IPv6 multicast route here ({e})");
        return;
    }
    let mut buf = [0u8; 16];
    if tokio::time::timeout(Duration::from_secs(1), receiver.recv(&mut buf))
        .await
        .is_err()
    {
        eprintln!("skipped: IPv6 multicast is not looped back here");
        return;
    }
    udp_round_trip(&group.to_string(), receiver).await;
}

#[tokio::test]
async fn a_track_whose_reports_stopped_is_not_in_the_picture() {
    // Track 1 reported 10 minutes ago (the output's stale time is 60 s); track 2 just now.
    let mut old = cot(1, 32.0);
    old.observed_at = chrono::Utc::now() - chrono::Duration::minutes(10);
    let addr = free_port();
    let hub = hub_with(&[old.clone(), cot(2, 33.0)]);
    let (_counters, task) = start(
        out(
            "eud",
            Delivery::Listen {
                bind: addr.to_string(),
                tls: None,
            },
        ),
        &hub,
    );
    let stream = tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(c) = tokio::net::TcpStream::connect(addr).await {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let mut client = Events::new(stream);
    let first = client.next().await;
    assert_eq!(
        first[0].1["uid"], "tms-OTK000000002",
        "only the track still reporting"
    );
    // Timed at its report, not at the send.
    let time: chrono::DateTime<chrono::Utc> = first[0].1["time"].parse().unwrap();
    assert!(chrono::Utc::now() - time < chrono::Duration::seconds(5));
    // An update of the silent track changes nothing either (its report is still 10 minutes old)…
    hub.upsert(old);
    // …while a new report brings it back.
    hub.upsert(cot(1, 32.5));
    let back = client.until("tms-OTK000000001", "a-").await;
    assert_eq!(
        back.iter().find(|(n, _)| n == "point").unwrap().1["lat"],
        "32.5"
    );
    task.abort();
}

#[tokio::test]
async fn serves_tak_clients_the_picture_then_the_stream() {
    let addr = free_port();
    let hub = hub_with(&[cot(1, 32.0)]);
    let (counters, task) = start(
        out(
            "eud",
            Delivery::Listen {
                bind: addr.to_string(),
                tls: None,
            },
        ),
        &hub,
    );
    let connect = || async {
        tokio::time::timeout(WAIT, async {
            loop {
                if let Ok(c) = tokio::net::TcpStream::connect(addr).await {
                    return c;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap()
    };
    let mut a = Events::new(connect().await);
    assert_eq!(a.next().await[0].1["uid"], "tms-OTK000000001");
    hub.upsert(cot(2, 33.0));
    a.until("tms-OTK000000002", "a-").await;

    // A second client gets the whole picture on connect.
    let mut b = Events::new(connect().await);
    let mut seen = vec![
        b.next().await[0].1["uid"].clone(),
        b.next().await[0].1["uid"].clone(),
    ];
    seen.sort();
    assert_eq!(seen, ["tms-OTK000000001", "tms-OTK000000002"]);

    // Both hear the delete.
    hub.delete(uid(1));
    for c in [&mut a, &mut b] {
        let d = c.until("tms-OTK000000001", "t-x-d-d").await;
        assert_eq!(link_of(&d)["uid"], "tms-OTK000000001");
    }
    assert_eq!(
        counters.clients.load(std::sync::atomic::Ordering::Relaxed),
        2
    );
    assert_eq!(counters.state().0, "listening");

    // One leaving does not disturb the other.
    drop(a);
    hub.upsert(cot(3, 34.0));
    b.until("tms-OTK000000003", "a-").await;
    tokio::time::timeout(WAIT, async {
        while counters.clients.load(std::sync::atomic::Ordering::Relaxed) != 1 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the client that left is no longer counted");
    task.abort();
}

#[tokio::test]
async fn serves_tak_clients_over_tls_with_client_certificates() {
    let pki = Pki::new();
    let addr = free_port();
    let hub = hub_with(&[cot(1, 32.0)]);
    let (counters, task) = start(
        out(
            "eud-tls",
            Delivery::Listen {
                bind: addr.to_string(),
                tls: Some(pki.server(true)),
            },
        ),
        &hub,
    );
    let dial = |cert: bool| {
        let config = Arc::new(pki.client(cert).client_config().unwrap());
        async move {
            let tcp = tokio::time::timeout(WAIT, async {
                loop {
                    if let Ok(c) = tokio::net::TcpStream::connect(addr).await {
                        return c;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
            let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
            tokio_rustls::TlsConnector::from(config)
                .connect(name, tcp)
                .await
        }
    };
    let mut c = Events::new(dial(true).await.expect("a client with a certificate"));
    assert_eq!(c.next().await[0].1["uid"], "tms-OTK000000001");
    hub.upsert(cot(2, 33.0));
    c.until("tms-OTK000000002", "a-").await;

    // Without a certificate: refused (under TLS 1.3 the refusal can arrive
    // after the client's side of the handshake), nothing sent.
    let errors = counters.errors.load(std::sync::atomic::Ordering::Relaxed);
    if let Ok(s) = dial(false).await {
        let mut s = s;
        let mut buf = [0u8; 64];
        let r = tokio::time::timeout(WAIT, s.read(&mut buf)).await.unwrap();
        assert!(matches!(r, Ok(0) | Err(_)), "{r:?}");
    }
    tokio::time::timeout(WAIT, async {
        while counters.errors.load(std::sync::atomic::Ordering::Relaxed) == errors {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the refusal is counted");
    task.abort();
}

#[tokio::test]
async fn outputs_follow_the_settings() {
    let hub = hub_with(&[]);
    let mut outputs = super::Outputs::default();
    let udp = |id: &str, port: u16| {
        out(
            id,
            Delivery::Multicast {
                group: "127.0.0.1".into(),
                port,
                ttl: 1,
                interface: None,
            },
        )
    };
    let mut s = super::TakSettings {
        outputs: vec![udp("a", 9), udp("b", 9)],
    };
    outputs.apply(&s, &hub);
    let a = outputs.counters("a").unwrap();
    assert!(outputs.counters("b").is_some());
    // Unchanged: left running. Changed: restarted. Off or gone: stopped.
    s.outputs[1].stale_secs = 90.0;
    outputs.apply(&s, &hub);
    assert!(Arc::ptr_eq(&a, &outputs.counters("a").unwrap()));
    let b = outputs.counters("b").unwrap();
    s.outputs[1].enabled = false;
    outputs.apply(&s, &hub);
    assert!(outputs.counters("b").is_none());
    s.outputs.remove(0);
    outputs.apply(&s, &hub);
    assert!(outputs.counters("a").is_none());
    drop(b);
    // An output the checks refuse is not started.
    s.outputs = vec![udp("bad id", 9)];
    outputs.apply(&s, &hub);
    assert!(outputs.counters("bad id").is_none());
    let (counts, gauges) = outputs.metrics(&hub);
    assert!(counts.is_empty());
    assert!(gauges.contains(&("clients".to_string(), 0)));
    outputs.stop_all();
}

/// The outbox to the hub, against a real Redis, in a namespace of its own.
mod redis_e2e {
    use super::*;
    use ot_store::RedisStore;

    async fn store() -> Option<RedisStore> {
        let url = std::env::var("OT_TEST_REDIS_URL").ok()?;
        let ns = format!(
            "ot-test-cot-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_micros()
        );
        Some(
            RedisStore::connect(&url, ot_store::Keys::new(ns))
                .await
                .unwrap(),
        )
    }

    #[tokio::test]
    async fn feeds_published_tracks_and_deletes_from_the_outbox() {
        let Some(redis) = store().await else {
            eprintln!("skipped: OT_TEST_REDIS_URL not set");
            return;
        };
        // Before the role starts: one published track, one held back.
        let live = track(1, serde_json::json!({}));
        redis.put_system_track(&live, false).await.unwrap();
        let mut held = track(2, serde_json::json!({}));
        held.published = Some(false);
        redis.put_system_track_quietly(&held).await.unwrap();

        let mut f =
            super::super::Feeder::start(redis.clone(), "c1".into(), Duration::from_secs(60))
                .await
                .unwrap();
        assert_eq!(f.hub.len(), 1, "the published picture is loaded");
        assert!(f.hub.get(uid(1)).is_some());
        let mut rx = f.hub.subscribe();
        // The writer's group is untouched by the cot group.
        f.pump(true).await.unwrap();

        // A new track goes out at once; its next update waits for the
        // interval, unless urgent.
        let mut t3 = track(3, serde_json::json!({"name": "THREE"}));
        redis.put_system_track(&t3, false).await.unwrap();
        f.pump(false).await.unwrap();
        assert!(matches!(rx.try_recv(), Ok(output::Change::Upsert(t)) if t.callsign == "THREE"));
        t3.view.name = Some("THREE B".into());
        redis.put_system_track(&t3, false).await.unwrap();
        f.pump(false).await.unwrap();
        assert!(rx.try_recv().is_err(), "coalesced behind the interval");
        t3.view.name = Some("THREE C".into());
        redis.put_system_track(&t3, true).await.unwrap();
        f.pump(false).await.unwrap();
        assert!(matches!(rx.try_recv(), Ok(output::Change::Upsert(t)) if t.callsign == "THREE C"));

        // Withdrawn (filtered out): deleted downstream, as on NATS.
        let mut w = f.hub.get(uid(1)).map(|_| live.clone()).unwrap();
        w.published = Some(false);
        redis.withdraw_system_track(&w, "filtered").await.unwrap();
        f.pump(false).await.unwrap();
        match rx.try_recv() {
            Ok(output::Change::Delete { uid: u, last_type }) => {
                assert_eq!((u, last_type.as_deref()), (uid(1), Some("a-u-G")))
            }
            other => panic!("{other:?}"),
        }

        // Retired: deleted; a delete for a track never sent is not sent.
        redis.retire_system_track(uid(3), "dropped").await.unwrap();
        redis.retire_system_track(uid(2), "dropped").await.unwrap();
        f.pump(false).await.unwrap();
        assert!(matches!(rx.try_recv(), Ok(output::Change::Delete { uid: u, .. }) if u == uid(3)));
        assert!(rx.try_recv().is_err());
        assert_eq!(f.hub.len(), 0);
        assert_eq!((f.upserts, f.deletes), (2, 2));

        // Everything read is acknowledged.
        assert!(
            redis
                .read_outbox(
                    super::super::GROUP,
                    "c1",
                    100,
                    Duration::from_millis(1),
                    true
                )
                .await
                .unwrap()
                .is_empty()
        );
        let backlog = redis.outbox_backlog(super::super::GROUP).await.unwrap();
        assert_eq!((backlog.pending, backlog.lag), (0, 0));
        redis.purge_namespace().await.unwrap();
    }
}
