//! The audit record: an append-only, hash-chained log of every decision
//! and every sign-in event (migration 0016).
//!
//! Each row stores `hash = SHA-256(prev_hash ‖ canonical row)`, where the
//! canonical row is the JSON array `[seq, at_ms, actor, op, outcome, ip,
//! detail, decision_id]` with `detail` as the exact text stored. Changing,
//! removing or inserting a row breaks the chain from there on, which
//! [`Db::verify_audit`] finds. Rows are never updated or deleted (triggers
//! refuse it): the record is kept forever. A database purged by 0.4.0
//! (alpha)'s retention setting keeps the last purged row's hash as the
//! anchor the chain continues from, and the purge's own row.

use aws_lc_rs::digest;
use rusqlite::types::ToSql;
use rusqlite::{OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::{Value, json};

use crate::sqlite::{Db, Result, now_ms};

/// The hash before the first row.
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// An event to record (a sign-in, a lockout, a settings change...).
#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub actor: String,
    pub op: String,
    pub success: bool,
    pub ip: Option<String>,
    pub detail: Value,
}

impl AuditEvent {
    pub fn new(actor: impl Into<String>, op: impl Into<String>) -> Self {
        Self {
            actor: actor.into(),
            op: op.into(),
            success: true,
            ip: None,
            detail: json!({}),
        }
    }

    pub fn failure(mut self) -> Self {
        self.success = false;
        self
    }

    pub fn ip(mut self, ip: impl Into<String>) -> Self {
        self.ip = Some(ip.into());
        self
    }

    /// Details; object keys add to what is already there.
    pub fn detail(mut self, d: Value) -> Self {
        match (&mut self.detail, d) {
            (Value::Object(have), Value::Object(add)) => have.extend(add),
            (slot, other) => *slot = other,
        }
        self
    }
}

/// A row of the audit record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditRow {
    pub seq: i64,
    pub at_ms: i64,
    pub actor: String,
    pub op: String,
    pub outcome: String,
    pub ip: Option<String>,
    pub detail: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_id: Option<i64>,
    pub hash: String,
}

/// Which rows to read, newest first.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
    /// Exact actor (case-insensitive).
    pub actor: Option<String>,
    /// Any of these operations.
    pub ops: Vec<String>,
    /// `success` or `failure`.
    pub outcome: Option<String>,
    /// Only rows before this sequence number (paging back).
    pub before_seq: Option<i64>,
    pub limit: usize,
}

/// What checking the chain found.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditVerify {
    /// Every row's hash holds and each follows the one before.
    pub ok: bool,
    pub rows: u64,
    pub first_seq: Option<i64>,
    pub last_seq: Option<i64>,
    /// The newest row's hash: note it elsewhere to detect a cut-off tail.
    pub head_hash: String,
    /// Where the chain starts after a retention purge (0.4.0 alpha).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
    /// The first problems found (at most 20), by sequence number.
    pub problems: Vec<Problem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Anchor {
    pub seq: i64,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Problem {
    pub seq: i64,
    pub problem: String,
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// A row's hash from the one before it and its content.
#[allow(clippy::too_many_arguments)]
pub fn row_hash(
    prev_hash: &str,
    seq: i64,
    at_ms: i64,
    actor: &str,
    op: &str,
    outcome: &str,
    ip: Option<&str>,
    detail: &str,
    decision_id: Option<i64>,
) -> String {
    let canonical = json!([seq, at_ms, actor, op, outcome, ip, detail, decision_id]).to_string();
    let mut ctx = digest::Context::new(&digest::SHA256);
    ctx.update(prev_hash.as_bytes());
    ctx.update(canonical.as_bytes());
    hex(ctx.finish().as_ref())
}

/// The chain's head: the newest row, else the anchor, else the start.
fn head(tx: &rusqlite::Connection) -> Result<(i64, String)> {
    if let Some(h) = tx
        .query_row(
            "SELECT seq, hash FROM audit ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
    {
        return Ok(h);
    }
    Ok(anchor(tx)?
        .map(|a| (a.seq, a.hash))
        .unwrap_or((0, GENESIS.to_owned())))
}

fn anchor(c: &rusqlite::Connection) -> Result<Option<Anchor>> {
    Ok(
        c.query_row("SELECT seq, hash FROM audit_anchor WHERE id = 1", [], |r| {
            Ok(Anchor {
                seq: r.get(0)?,
                hash: r.get(1)?,
            })
        })
        .optional()?,
    )
}

/// Append a row in `tx`; returns its sequence number.
#[allow(clippy::too_many_arguments)]
pub(crate) fn append(
    tx: &Transaction<'_>,
    at_ms: i64,
    actor: &str,
    op: &str,
    success: bool,
    ip: Option<&str>,
    detail: &str,
    decision_id: Option<i64>,
) -> Result<i64> {
    let (last, prev) = head(tx)?;
    let seq = last + 1;
    let outcome = if success { "success" } else { "failure" };
    let hash = row_hash(
        &prev,
        seq,
        at_ms,
        actor,
        op,
        outcome,
        ip,
        detail,
        decision_id,
    );
    tx.execute(
        "INSERT INTO audit (seq, at_ms, actor, op, outcome, ip, detail, decision_id, prev_hash, hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            seq,
            at_ms,
            actor,
            op,
            outcome,
            ip,
            detail,
            decision_id,
            prev,
            hash
        ],
    )?;
    PENDING.with(|p| {
        p.borrow_mut().push(Logged {
            seq,
            actor: actor.to_owned(),
            op: op.to_owned(),
            outcome,
            ip: ip.map(str::to_owned),
            detail: log_detail(detail, decision_id),
            decision_id,
            hash,
        })
    });
    Ok(seq)
}

/// An audit row to write to the server log once its transaction commits.
struct Logged {
    seq: i64,
    actor: String,
    op: String,
    outcome: &'static str,
    ip: Option<String>,
    detail: String,
    decision_id: Option<i64>,
    hash: String,
}

thread_local! {
    /// The client address of the request this thread works for, stamped on
    /// the audit copy of each decision it records (AU-3).
    static CLIENT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with `ip` as the client address of the decisions it records.
pub fn with_client<R>(ip: Option<String>, f: impl FnOnce() -> R) -> R {
    let before = CLIENT.with(|c| c.replace(ip));
    let r = f();
    CLIENT.with(|c| *c.borrow_mut() = before);
    r
}

/// The client address set by [`with_client`] on this thread.
pub(crate) fn client() -> Option<String> {
    CLIENT.with(|c| c.borrow().clone())
}

thread_local! {
    /// Rows appended in the transaction running on this thread.
    static PENDING: std::cell::RefCell<Vec<Logged>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A row's detail for the log. A decision's is without its `before` and
/// `after` (the configuration it changed, which can hold source
/// credentials): its decision id finds them in the audit API.
fn log_detail(detail: &str, decision_id: Option<i64>) -> String {
    if decision_id.is_none() {
        return detail.to_owned();
    }
    match serde_json::from_str::<Value>(detail) {
        Ok(Value::Object(mut o)) => {
            o.remove("before");
            o.remove("after");
            Value::Object(o).to_string()
        }
        _ => String::new(),
    }
}

/// After a transaction: every audit row it appended goes to the server
/// log (target `audit`) when it committed, and is forgotten when not. The
/// log then carries the whole record for a SIEM: on standard output
/// (`OT_LOG_FORMAT=json`) and over OTLP, event name `audit.record`.
pub(crate) fn log_pending(committed: bool) {
    let rows = PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()));
    if !committed {
        return;
    }
    for r in rows {
        // Named, so an OpenTelemetry collector can route audit records
        // (the log record's event name) apart from the other logs.
        tracing::info!(
            name: "audit.record",
            target: "audit",
            seq = r.seq,
            actor = %r.actor,
            op = %r.op,
            outcome = r.outcome,
            ip = r.ip.as_deref(),
            detail = %r.detail,
            decision_id = r.decision_id,
            hash = %r.hash,
            "audit record"
        );
    }
}

/// Append an event in `tx`.
pub fn append_event(tx: &Transaction<'_>, e: &AuditEvent, at_ms: i64) -> Result<i64> {
    append(
        tx,
        at_ms,
        &e.actor,
        &e.op,
        e.success,
        e.ip.as_deref(),
        &e.detail.to_string(),
        None,
    )
}

/// Start the chain on a database that has none, noting the decisions
/// recorded before it (they are not in the chain).
pub(crate) fn start_chain(db: &mut Db) -> Result<()> {
    // Databases are opened often: only take the write lock when it may be
    // needed.
    let started: bool = db.connection().query_row(
        "SELECT EXISTS (SELECT 1 FROM audit) OR EXISTS (SELECT 1 FROM audit_anchor)",
        [],
        |r| r.get(0),
    )?;
    if started {
        return Ok(());
    }
    db.write(|tx| {
        let empty: bool = tx.query_row(
            "SELECT NOT EXISTS (SELECT 1 FROM audit) AND NOT EXISTS (SELECT 1 FROM audit_anchor)",
            [],
            |r| r.get(0),
        )?;
        if empty {
            let before: i64 =
                tx.query_row("SELECT coalesce(max(id), 0) FROM decisions", [], |r| {
                    r.get(0)
                })?;
            let e = AuditEvent::new("system", "audit_chain_start")
                .detail(json!({ "decisions_before_chain": before }));
            append_event(tx, &e, now_ms())?;
        }
        Ok(())
    })
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuditRow> {
    let detail: String = r.get(6)?;
    Ok(AuditRow {
        seq: r.get(0)?,
        at_ms: r.get(1)?,
        actor: r.get(2)?,
        op: r.get(3)?,
        outcome: r.get(4)?,
        ip: r.get(5)?,
        detail: serde_json::from_str(&detail).unwrap_or(Value::String(detail)),
        decision_id: r.get(7)?,
        hash: r.get(8)?,
    })
}

impl Db {
    /// Record an event in its own transaction.
    pub fn audit(&mut self, e: &AuditEvent) -> Result<i64> {
        self.write(|tx| append_event(tx, e, now_ms()))
    }

    /// Rows matching `f`, newest first.
    pub fn audit_rows(&self, f: &AuditFilter) -> Result<Vec<AuditRow>> {
        let mut clauses: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        let mut arg = |clause: &str, v: Box<dyn ToSql>, args: &mut Vec<Box<dyn ToSql>>| {
            args.push(v);
            clauses.push(clause.replace('?', &format!("?{}", args.len())));
        };
        if let Some(v) = f.from_ms {
            arg("at_ms >= ?", Box::new(v), &mut args);
        }
        if let Some(v) = f.to_ms {
            arg("at_ms <= ?", Box::new(v), &mut args);
        }
        if let Some(v) = f.actor.as_ref().filter(|a| !a.trim().is_empty()) {
            arg(
                "actor = ? COLLATE NOCASE",
                Box::new(v.trim().to_owned()),
                &mut args,
            );
        }
        if let Some(v) = f.outcome.as_ref().filter(|a| !a.trim().is_empty()) {
            arg("outcome = ?", Box::new(v.trim().to_owned()), &mut args);
        }
        if let Some(v) = f.before_seq {
            arg("seq < ?", Box::new(v), &mut args);
        }
        let ops: Vec<&String> = f.ops.iter().filter(|o| !o.trim().is_empty()).collect();
        if !ops.is_empty() {
            let mut marks = Vec::new();
            for o in ops {
                args.push(Box::new(o.trim().to_owned()));
                marks.push(format!("?{}", args.len()));
            }
            clauses.push(format!("op IN ({})", marks.join(", ")));
        }
        let wh = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        let sql = format!(
            "SELECT seq, at_ms, actor, op, outcome, ip, detail, decision_id, hash FROM audit
             {wh} ORDER BY seq DESC LIMIT {}",
            f.limit.clamp(1, 100_000)
        );
        let mut st = self.connection().prepare(&sql)?;
        let refs: Vec<&dyn ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = st.query_map(refs.as_slice(), row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Check every row's hash and link, from the start or the anchor.
    pub fn verify_audit(&self) -> Result<AuditVerify> {
        let c = self.connection();
        let anchor = anchor(c)?;
        let (mut expect_seq, mut prev) = anchor
            .as_ref()
            .map(|a| (a.seq + 1, a.hash.clone()))
            .unwrap_or((1, GENESIS.to_owned()));
        let mut out = AuditVerify {
            ok: true,
            rows: 0,
            first_seq: None,
            last_seq: None,
            head_hash: prev.clone(),
            anchor: anchor.clone(),
            problems: Vec::new(),
        };
        let problem = |out: &mut AuditVerify, seq: i64, p: String| {
            out.ok = false;
            if out.problems.len() < 20 {
                out.problems.push(Problem { seq, problem: p });
            }
        };
        let mut last_purge_anchor: Option<String> = None;
        let mut st = c.prepare(
            "SELECT seq, at_ms, actor, op, outcome, ip, detail, decision_id, prev_hash, hash
             FROM audit ORDER BY seq",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let seq: i64 = r.get(0)?;
            let at_ms: i64 = r.get(1)?;
            let actor: String = r.get(2)?;
            let op: String = r.get(3)?;
            let outcome: String = r.get(4)?;
            let ip: Option<String> = r.get(5)?;
            let detail: String = r.get(6)?;
            let decision_id: Option<i64> = r.get(7)?;
            let prev_hash: String = r.get(8)?;
            let hash: String = r.get(9)?;
            out.rows += 1;
            out.first_seq.get_or_insert(seq);
            out.last_seq = Some(seq);
            if seq != expect_seq {
                problem(
                    &mut out,
                    seq,
                    format!("expected row {expect_seq} here: rows are missing"),
                );
            }
            if prev_hash != prev {
                problem(
                    &mut out,
                    seq,
                    "does not follow the row before it (prev_hash differs)".into(),
                );
            }
            let want = row_hash(
                &prev_hash,
                seq,
                at_ms,
                &actor,
                &op,
                &outcome,
                ip.as_deref(),
                &detail,
                decision_id,
            );
            if want != hash {
                problem(
                    &mut out,
                    seq,
                    "its content does not match its hash (changed)".into(),
                );
            }
            if op == "audit_purge" {
                last_purge_anchor = serde_json::from_str::<Value>(&detail)
                    .ok()
                    .and_then(|d| d["anchor_hash"].as_str().map(str::to_owned));
            }
            expect_seq = seq + 1;
            prev = hash;
        }
        out.head_hash = prev;
        // The anchor is also written in the purge row that set it, which the
        // chain protects: an anchor changed afterwards no longer matches.
        if let Some(a) = &anchor
            && last_purge_anchor.as_deref() != Some(a.hash.as_str())
        {
            problem(
                &mut out,
                a.seq,
                "the anchor does not match the last purge's record".into(),
            );
        }
        Ok(out)
    }

    /// The chain's newest sequence number and hash.
    pub fn audit_head(&self) -> Result<(i64, String)> {
        head(self.connection())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Decision;

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    /// What a retention purge in 0.4.0 alpha did (the record is
    /// now kept forever): a database purged then must still verify.
    impl Db {
        /// Delete rows older than `before_ms` (retention), keeping the chain
        /// verifiable: the last deleted row's hash becomes the anchor, and the
        /// purge is recorded as a row of its own. Returns the rows deleted.
        fn purge_audit(&mut self, before_ms: i64, actor: &str) -> Result<usize> {
            self.write(|tx| {
                let last: Option<(i64, String)> = tx
                    .query_row(
                        "SELECT seq, hash FROM audit WHERE at_ms < ?1 ORDER BY seq DESC LIMIT 1",
                        [before_ms],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                let Some((through, hash)) = last else {
                    return Ok(0);
                };
                tx.execute("INSERT INTO audit_purge_gate (id) VALUES (1)", [])?;
                let n = tx.execute("DELETE FROM audit WHERE seq <= ?1", [through])?;
                tx.execute("DELETE FROM audit_purge_gate", [])?;
                tx.execute(
                    "INSERT INTO audit_anchor (id, seq, hash) VALUES (1, ?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET seq = excluded.seq, hash = excluded.hash",
                    params![through, hash],
                )?;
                let e = AuditEvent::new(actor, "audit_purge").detail(json!({
                    "through_seq": through,
                    "rows": n,
                    "before_ms": before_ms,
                    "anchor_hash": hash,
                }));
                append_event(tx, &e, now_ms())?;
                Ok(n)
            })
        }
    }

    #[test]
    fn a_new_database_starts_a_chain_that_verifies() {
        let mut db = db();
        db.audit(&AuditEvent::new("a@x", "login").ip("10.0.0.1"))
            .unwrap();
        db.audit(
            &AuditEvent::new("b@x", "login")
                .failure()
                .detail(json!({"reason": "bad_password"})),
        )
        .unwrap();
        db.record(&Decision::new("a@x", "update_user").reason("why"))
            .unwrap();
        let v = db.verify_audit().unwrap();
        assert!(v.ok, "{v:?}");
        assert_eq!(v.rows, 4, "chain start, two events, one decision");
        let rows = db
            .audit_rows(&AuditFilter {
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows[0].op, "update_user");
        assert!(rows[0].decision_id.is_some());
        assert_eq!(rows[0].detail["reason"], "why");
        assert_eq!(rows[3].op, "audit_chain_start");
        assert_eq!(db.audit_head().unwrap().1, v.head_hash);
    }

    #[test]
    fn filters_select_rows() {
        let mut db = db();
        for (who, op, ok) in [
            ("a@x", "login", true),
            ("b@x", "login", false),
            ("a@x", "logout", true),
        ] {
            let mut e = AuditEvent::new(who, op);
            if !ok {
                e = e.failure();
            }
            db.audit(&e).unwrap();
        }
        let f = |actor: Option<&str>, ops: &[&str], outcome: Option<&str>| {
            db.audit_rows(&AuditFilter {
                actor: actor.map(str::to_owned),
                ops: ops.iter().map(|s| s.to_string()).collect(),
                outcome: outcome.map(str::to_owned),
                limit: 100,
                ..Default::default()
            })
            .unwrap()
            .len()
        };
        assert_eq!(f(Some("A@X"), &[], None), 2);
        assert_eq!(f(None, &["login"], None), 2);
        assert_eq!(f(None, &["login", "logout"], Some("success")), 2);
        assert_eq!(f(None, &[], Some("failure")), 1);
        let now = now_ms();
        let later = db
            .audit_rows(&AuditFilter {
                from_ms: Some(now + 60_000),
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert!(later.is_empty());
    }

    #[test]
    fn rows_cannot_be_changed_or_deleted_through_sql() {
        let mut db = db();
        db.audit(&AuditEvent::new("a@x", "login")).unwrap();
        let c = db.connection();
        assert!(c.execute("UPDATE audit SET actor = 'x'", []).is_err());
        assert!(c.execute("DELETE FROM audit", []).is_err());
    }

    #[test]
    fn tampering_is_found() {
        let mut db = db();
        for i in 0..5 {
            db.audit(&AuditEvent::new("a@x", "login").detail(json!({"n": i})))
                .unwrap();
        }
        assert!(db.verify_audit().unwrap().ok);
        // Someone with the database file drops the triggers and edits a row.
        let c = db.connection();
        c.execute_batch("DROP TRIGGER audit_no_update; DROP TRIGGER audit_no_delete;")
            .unwrap();
        c.execute("UPDATE audit SET actor = 'mallory' WHERE seq = 3", [])
            .unwrap();
        let v = db.verify_audit().unwrap();
        assert!(!v.ok);
        assert_eq!(v.problems[0].seq, 3);
        assert!(v.problems[0].problem.contains("changed"));

        // Recomputing that row's hash breaks the link to the next one.
        let (prev, at, detail): (String, i64, String) = c
            .query_row(
                "SELECT prev_hash, at_ms, detail FROM audit WHERE seq = 3",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        let h = row_hash(
            &prev, 3, at, "mallory", "login", "success", None, &detail, None,
        );
        c.execute("UPDATE audit SET hash = ?1 WHERE seq = 3", [h])
            .unwrap();
        let v = db.verify_audit().unwrap();
        assert!(!v.ok);
        assert_eq!(v.problems[0].seq, 4);

        // A deleted row is a gap.
        let mut db2 = self::db();
        for _ in 0..4 {
            db2.audit(&AuditEvent::new("a@x", "login")).unwrap();
        }
        let c = db2.connection();
        c.execute_batch("DROP TRIGGER audit_no_delete;").unwrap();
        c.execute("DELETE FROM audit WHERE seq = 2", []).unwrap();
        let v = db2.verify_audit().unwrap();
        assert!(!v.ok);
        assert_eq!(v.problems[0].seq, 3);
    }

    #[test]
    fn a_retention_purge_keeps_the_chain_verifiable() {
        let mut db = db();
        for _ in 0..5 {
            db.audit(&AuditEvent::new("a@x", "login")).unwrap();
        }
        let n = db.purge_audit(now_ms() + 1, "system").unwrap();
        assert_eq!(n, 6, "the chain start and five events");
        let v = db.verify_audit().unwrap();
        assert!(v.ok, "{v:?}");
        assert_eq!(v.rows, 1, "the purge's own row");
        assert_eq!(v.anchor.as_ref().unwrap().seq, 6);
        db.audit(&AuditEvent::new("a@x", "logout")).unwrap();
        assert!(db.verify_audit().unwrap().ok);
        assert_eq!(db.purge_audit(0, "system").unwrap(), 0, "nothing older");

        // A changed anchor no longer matches the purge's record.
        db.connection()
            .execute("UPDATE audit_anchor SET hash = ?1", [GENESIS])
            .unwrap();
        assert!(!db.verify_audit().unwrap().ok);
    }

    #[test]
    fn rows_reach_the_log_only_when_committed_and_without_configuration() {
        let d = r#"{"reason":"r","evidence":{"x":1},"before":{"k":"secret"},"after":null}"#;
        let logged: Value = serde_json::from_str(&log_detail(d, Some(3))).unwrap();
        assert_eq!(logged, json!({"reason": "r", "evidence": {"x": 1}}));
        assert_eq!(
            log_detail(r#"{"via":"password"}"#, None),
            r#"{"via":"password"}"#
        );
        // A transaction that fails logs none of the rows it appended.
        let mut db = db();
        let r: Result<()> = db.write(|tx| {
            append_event(tx, &AuditEvent::new("a@x", "login"), now_ms())?;
            Err(crate::StoreError::Conflict("no".into()))
        });
        assert!(r.is_err());
        assert!(PENDING.with(|p| p.borrow().is_empty()));
    }

    #[test]
    fn undo_leaves_the_audit_copy_of_a_decision_alone() {
        let mut db = db();
        let id = db.record(&Decision::new("a@x", "merge")).unwrap();
        let undo = db.record(&Decision::new("a@x", "undo")).unwrap();
        // Undo marks the decision itself in place; the chain is unaffected.
        db.connection()
            .execute(
                "UPDATE decisions SET undone_by = ?2 WHERE id = ?1",
                [id, undo],
            )
            .unwrap();
        assert!(db.verify_audit().unwrap().ok);
    }
}
