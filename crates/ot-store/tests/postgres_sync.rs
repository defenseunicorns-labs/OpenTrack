#![cfg(feature = "postgres")]

use ot_core::SiteCode;
use ot_store::sqlite::Db;
use ot_store::sync::SyncStatus;
use ot_sync::{Entry, GlobalId, Hlc};
use serde_json::json;

#[test]
fn postgres_password_and_account_maintenance() {
    let Ok(url) = std::env::var("OT_TEST_POSTGRES_URL") else { return; };
    let mut db = Db::open_postgres(&url).unwrap();
    let id = "postgres-auth-test";
    db.create_user(&ot_store::NewUser {
        id, email: "pg-auth@example.invalid", name: "PG Auth", role: "admin",
        password_hash: Some("hash-one"), origin: "local",
    }).unwrap();
    assert_eq!(db.password_hash("pg-auth@example.invalid").unwrap().unwrap().1.as_deref(), Some("hash-one"));
    db.rehash_password(id, "hash-one-rehashed").unwrap();
    db.set_password(id, Some("hash-two"), true, 2).unwrap();
    assert_eq!(db.recent_password_hashes(id, 2).unwrap(), vec!["hash-two", "hash-one"]);
    let failure = db.note_login_failure(id, 60_000, 2, 60_000).unwrap();
    assert_eq!(failure.count, 1);
    assert!(!failure.locked_now);
    assert!(db.note_login_failure(id, 60_000, 2, 60_000).unwrap().locked_now);
    db.note_refused_login(id).unwrap();
    assert!(db.unlock_user(id).unwrap().locked_until_ms.is_none());
    db.note_login(id).unwrap();
    db.set_account_expiry(id, Some(i64::MAX)).unwrap();
    db.revoke_user_tokens(id).unwrap();
    db.set_password(id, None, false, 2).unwrap();
    assert_eq!(db.password_hash("pg-auth@example.invalid").unwrap().unwrap().1, None);
}

#[test]
fn postgres_sync_round_trip() {
    let Ok(url) = std::env::var("OT_TEST_POSTGRES_URL") else {
        eprintln!("set OT_TEST_POSTGRES_URL to run the disposable PostgreSQL integration test");
        return;
    };
    let mut db = Db::open_postgres(&url).unwrap();
    let site: SiteCode = "OTK".parse().unwrap();
    let peer: SiteCode = "ABC".parse().unwrap();
    let now = 1_800_000_000_000u64;
    let own = db
        .sync_append_own(site, "operator", "admin", &json!({"op":"merge"}), now)
        .unwrap();
    assert_eq!(db.sync_next_seq(site).unwrap(), own.id.seq + 1);
    assert_eq!(db.sync_heads().unwrap(), vec![(site, own.id.seq)]);
    assert_eq!(
        db.sync_range(site, own.id.seq, own.id.seq).unwrap(),
        vec![own.clone()]
    );
    assert_eq!(db.sync_since(site, 0, 10).unwrap(), vec![own.clone()]);
    assert_eq!(
        db.sync_entry(own.id).unwrap().unwrap().1,
        SyncStatus::Applied
    );

    let received = Entry {
        id: GlobalId { site: peer, seq: 2 },
        hlc: Hlc::from_i64(own.hlc.as_i64() + 1),
        actor: "peer".into(),
        role: "operator".into(),
        command: json!({"op":"undo","decision":own.id.to_string()}),
    };
    assert!(
        db.sync_record(&received, SyncStatus::Pending, None)
            .unwrap()
    );
    assert!(
        !db.sync_record(&received, SyncStatus::Pending, None)
            .unwrap()
    );
    assert_eq!(db.sync_missing(peer, 3, 10).unwrap(), vec![(1, 1), (3, 3)]);
    assert_eq!(db.sync_pending().unwrap(), vec![received.clone()]);
    db.sync_set_status(received.id, SyncStatus::Applied, None)
        .unwrap();
    assert_eq!(db.sync_undone_by(own.id).unwrap(), Some(received.id));
    assert_eq!(
        db.sync_applied_after(own.hlc).unwrap(),
        vec![received.clone()]
    );
    assert_eq!(db.sync_counts().unwrap().get("applied"), Some(&2));
    assert_eq!(db.sync_expire_pending(i64::MAX).unwrap(), 0);

    let decision = db
        .record(&ot_store::Decision::new("operator", "test_sync_tag"))
        .unwrap();
    assert_eq!(db.max_decision_id().unwrap(), decision);
    assert_eq!(
        db.tag_decisions(decision - 1, "operator", own.id, &["part-a".into()])
            .unwrap(),
        vec![decision]
    );
    assert_eq!(
        db.sync_of_decision(decision).unwrap(),
        Some((own.id, Some("part-a".into())))
    );
    assert_eq!(
        db.decision_of_sync(own.id, Some("part-a")).unwrap(),
        Some(decision)
    );

    let pin = db
        .sync_pin_key(peer, "ed25519:example", "operator")
        .unwrap();
    assert!(pin > decision);
    assert_eq!(db.sync_peer_keys().unwrap()[0].site, peer.as_str());
    assert!(db.sync_unpin_key(peer, "operator").unwrap().is_some());
    assert!(db.sync_peer_keys().unwrap().is_empty());
}
