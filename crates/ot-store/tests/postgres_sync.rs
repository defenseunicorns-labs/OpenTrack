#![cfg(feature = "postgres")]

use ot_core::SiteCode;
use ot_store::sqlite::Db;
use ot_store::sync::SyncStatus;
use ot_sync::{Entry, GlobalId, Hlc};
use serde_json::json;

#[test]
fn postgres_history_schema_registry_and_undo() {
    let Ok(url) = std::env::var("OT_TEST_POSTGRES_URL") else {
        return;
    };
    let mut db = Db::open_postgres(&url).unwrap();
    let site: SiteCode = "TST".parse().unwrap();
    let (uid, _) = db
        .create_system_track(
            site,
            "ais",
            "1",
            ot_store::Decision::new("engine", "create_system_track"),
        )
        .unwrap();
    assert_eq!(db.reports_for(uid).unwrap().len(), 1);
    let retired = db
        .retire_system_track(uid, ot_store::Decision::new("operator", "delete_track"))
        .unwrap();
    let undone = db.undo_decision(retired, "operator", "restore").unwrap();
    assert_eq!(undone.revived, vec![uid]);
    assert_eq!(db.reports_for(uid).unwrap().len(), 1);
    let (nodes, edges) = db
        .purge_track_history(ot_store::Decision::new("operator", "purge_track_history"))
        .unwrap();
    assert!(nodes >= 2 && edges >= 2);
    let field = json!({"key":"operator_note","type":"string","required":false});
    let version = db
        .publish_schema_as(std::slice::from_ref(&field), Some("peer"), "operator")
        .unwrap();
    assert!(version.fields.iter().any(|f| f["key"] == "operator_note"));
    let draft = db
        .put_schema_draft(
            &[json!({"key":"local_field","type":"string","required":false})],
            None,
            "operator",
        )
        .unwrap();
    let imported = db
        .publish_schema_as(
            &[json!({"key":"peer_field","type":"string","required":false})],
            None,
            "operator",
        )
        .unwrap();
    assert_eq!(imported.version, draft.version);
    assert_eq!(
        db.schema_versions().unwrap().last().unwrap().status,
        "draft"
    );
    let first: ot_store::registry::Entity = serde_json::from_value(
        json!({"id":"one","name":"One","identifiers":[{"scheme":"mmsi","value":"123"}]}),
    )
    .unwrap();
    let second: ot_store::registry::Entity = serde_json::from_value(json!({"id":"two","name":"Two","identifiers":[{"scheme":"mmsi","value":"123"},{"scheme":"icao","value":"456"}]})).unwrap();
    let counts = db
        .registry_import(&[first, second], "operator", "test import")
        .unwrap();
    assert_eq!(
        (
            counts.entities_created,
            counts.conflicts,
            counts.identifiers
        ),
        (2, 1, 2)
    );
    assert_eq!(db.entity("two").unwrap().unwrap().identifiers.len(), 1);
}

#[test]
fn postgres_password_and_account_maintenance() {
    let Ok(url) = std::env::var("OT_TEST_POSTGRES_URL") else {
        return;
    };
    let mut db = Db::open_postgres(&url).unwrap();
    let id = "postgres-auth-test";
    db.create_user(&ot_store::NewUser {
        id,
        email: "pg-auth@example.invalid",
        name: "PG Auth",
        role: "admin",
        password_hash: Some("hash-one"),
        origin: "local",
    })
    .unwrap();
    assert_eq!(
        db.password_hash("pg-auth@example.invalid")
            .unwrap()
            .unwrap()
            .1
            .as_deref(),
        Some("hash-one")
    );
    db.rehash_password(id, "hash-one-rehashed").unwrap();
    db.set_password(id, Some("hash-two"), true, 2).unwrap();
    assert_eq!(
        db.recent_password_hashes(id, 2).unwrap(),
        vec!["hash-two", "hash-one"]
    );
    let failure = db.note_login_failure(id, 60_000, 2, 60_000).unwrap();
    assert_eq!(failure.count, 1);
    assert!(!failure.locked_now);
    assert!(
        db.note_login_failure(id, 60_000, 2, 60_000)
            .unwrap()
            .locked_now
    );
    db.note_refused_login(id).unwrap();
    assert!(db.unlock_user(id).unwrap().locked_until_ms.is_none());
    db.note_login(id).unwrap();
    db.set_account_expiry(id, Some(i64::MAX)).unwrap();
    db.revoke_user_tokens(id).unwrap();
    db.set_password(id, None, false, 2).unwrap();
    assert_eq!(
        db.password_hash("pg-auth@example.invalid")
            .unwrap()
            .unwrap()
            .1,
        None
    );
}

#[test]
fn postgres_probe_frames_are_replaced_and_capped() {
    let Ok(url) = std::env::var("OT_TEST_POSTGRES_URL") else {
        return;
    };
    let mut db = Db::open_postgres(&url).unwrap();
    let frames = vec![
        (b"one".to_vec(), Some(json!({"topic":"ais/1"}))),
        (b"two".to_vec(), None),
    ];
    assert_eq!(db.store_probe_samples("probe-test", &frames, 1).unwrap(), 1);
    let samples = db.probe_samples("probe-test", 10).unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].bytes, b"one");
    assert_eq!(samples[0].meta.as_ref().unwrap()["topic"], "ais/1");
    assert_eq!(db.store_probe_samples("probe-test", &[], 1).unwrap(), 0);
    assert!(db.probe_samples("probe-test", 10).unwrap().is_empty());
}

#[test]
fn postgres_group_and_pair_operations() {
    let Ok(url) = std::env::var("OT_TEST_POSTGRES_URL") else {
        return;
    };
    let mut db = Db::open_postgres(&url).unwrap();
    let site: SiteCode = "GRP".parse().unwrap();
    let (a, _) = db
        .create_system_track(
            site,
            "test",
            "a",
            ot_store::Decision::new("engine", "create_system_track"),
        )
        .unwrap();
    let (b, _) = db
        .create_system_track(
            site,
            "test",
            "b",
            ot_store::Decision::new("engine", "create_system_track"),
        )
        .unwrap();
    let spec: ot_store::groups::GroupSpec =
        serde_json::from_value(json!({"name":"Flight","sidc":"SHSPGG----"})).unwrap();
    let (group, _) = db
        .create_group(
            site,
            &spec,
            &[a],
            ot_store::Decision::new("operator", "create_group"),
        )
        .unwrap();
    assert_eq!(db.groups().unwrap()[0].members, vec![a]);
    db.change_group_members(
        group,
        &[b],
        &[a],
        ot_store::Decision::new("operator", "group_members"),
    )
    .unwrap();
    assert_eq!(db.groups().unwrap()[0].members, vec![b]);
    let renamed: ot_store::groups::GroupSpec =
        serde_json::from_value(json!({"name":"Flight 2","sidc":"SHSPGG----"})).unwrap();
    db.update_group(
        group,
        &renamed,
        ot_store::Decision::new("operator", "update_group"),
    )
    .unwrap();
    assert_eq!(db.groups().unwrap()[0].spec.name, "Flight 2");
    db.pair_system_tracks(&[a, b], ot_store::Decision::new("operator", "pair_tracks"))
        .unwrap();
    db.unpair_system_tracks(a, b, ot_store::Decision::new("operator", "unpair_tracks"))
        .unwrap();
    db.dissolve_group(group, ot_store::Decision::new("operator", "dissolve_group"))
        .unwrap();
    assert!(db.groups().unwrap().is_empty());
}

#[test]
fn postgres_undo_reverses_pair_and_group() {
    let Ok(url) = std::env::var("OT_TEST_POSTGRES_URL") else {
        return;
    };
    let mut db = Db::open_postgres(&url).unwrap();
    let site: SiteCode = "UND".parse().unwrap();
    let (a, _) = db
        .create_system_track(
            site,
            "test",
            "undo-a",
            ot_store::Decision::new("engine", "create_system_track"),
        )
        .unwrap();
    let (b, _) = db
        .create_system_track(
            site,
            "test",
            "undo-b",
            ot_store::Decision::new("engine", "create_system_track"),
        )
        .unwrap();
    let pair = db
        .pair_system_tracks(&[a, b], ot_store::Decision::new("operator", "pair_tracks"))
        .unwrap();
    assert_eq!(db.paired_with(a).unwrap(), vec![b]);
    db.undo_decision(pair, "operator", "reverse pair").unwrap();
    assert!(db.paired_with(a).unwrap().is_empty());
    let spec: ot_store::groups::GroupSpec =
        serde_json::from_value(json!({"name":"Undo Group","sidc":"SHSPGG----"})).unwrap();
    let (_, formed) = db
        .create_group(
            site,
            &spec,
            &[a],
            ot_store::Decision::new("operator", "create_group"),
        )
        .unwrap();
    assert_eq!(db.groups().unwrap().len(), 1);
    db.undo_decision(formed, "operator", "reverse group")
        .unwrap();
    assert!(db.groups().unwrap().is_empty());
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
    let audit = db.verify_audit().unwrap();
    assert!(audit.ok, "audit chain: {:?}", audit.problems);
    assert!(audit.rows > 0);
}
