//! The replicated decision log: what an entry is, which track-management
//! commands replicate, and how an entry that arrives late is judged against
//! the ones already applied.
//!
//! An entry carries the command as the deciding node ran it, in canonical
//! form: tracks by UID, and any UID the command minted (a new group's)
//! included, so every node that applies it gets the same result.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use ot_core::{SiteCode, Uid};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::Hlc;

/// A decision's id on every node: the deciding node's site code and its
/// sequence there, e.g. `AAA:12`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlobalId {
    pub site: SiteCode,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a decision id (<site>:<seq>): {0:?}")]
pub struct IdError(String);

impl fmt::Display for GlobalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.site, self.seq)
    }
}

impl FromStr for GlobalId {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || IdError(s.to_owned());
        let (site, seq) = s.split_once(':').ok_or_else(bad)?;
        Ok(Self {
            site: site.parse().map_err(|_| bad())?,
            seq: seq.parse().ok().filter(|n| *n > 0).ok_or_else(bad)?,
        })
    }
}

impl Serialize for GlobalId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for GlobalId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// One decision in the replicated log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: GlobalId,
    pub hlc: Hlc,
    /// Who decided, as the deciding node knows them.
    pub actor: String,
    /// Their role there.
    pub role: String,
    /// The command, canonical: `{"op": ..., ...}`.
    pub command: Value,
}

impl Entry {
    pub fn op(&self) -> &str {
        self.command["op"].as_str().unwrap_or_default()
    }
}

/// Track-management commands that hold on every node. The rest stay on the
/// node that ran them: a split names one node's own source track (the other
/// nodes see the result in that node's reports), and a purge is one node's
/// housekeeping.
const REPLICATED: &[&str] = &[
    "pair",
    "unpair",
    "merge",
    "do_not_pair",
    "delete",
    "group_create",
    "group_update",
    "group_members",
    "group_dissolve",
    "delete_history_point",
    "undo",
    // The swarm profile: the output schema and correlation settings.
    "profile_schema",
    "profile_correlation",
];

pub fn replicated(op: &str) -> bool {
    REPLICATED.contains(&op)
}

fn uids(v: &Value) -> Vec<Uid> {
    let one = |v: &Value| {
        v.as_str().and_then(|s| {
            s.trim_start_matches(ot_core::uid::DOC_ID_PREFIX)
                .parse()
                .ok()
        })
    };
    match v {
        Value::Array(a) => a.iter().filter_map(one).collect(),
        other => one(other).into_iter().collect(),
    }
}

/// Every track a command names.
pub fn targets(command: &Value) -> BTreeSet<Uid> {
    [
        "tracks", "a", "b", "from", "into", "track", "group", "members", "add", "remove",
    ]
    .iter()
    .flat_map(|k| uids(&command[*k]))
    .collect()
}

/// The pairs of tracks a command says are, or are not, one object. Two such
/// commands on the same pair conflict.
fn pairs(command: &Value) -> BTreeSet<(Uid, Uid)> {
    let ordered = |a: Uid, b: Uid| if a < b { (a, b) } else { (b, a) };
    let two = |x: &str, y: &str| {
        uids(&command[x])
            .into_iter()
            .zip(uids(&command[y]))
            .map(|(a, b)| ordered(a, b))
            .collect()
    };
    match command["op"].as_str().unwrap_or_default() {
        "pair" => {
            let t = uids(&command["tracks"]);
            let mut out = BTreeSet::new();
            for (i, a) in t.iter().enumerate() {
                for b in &t[i + 1..] {
                    out.insert(ordered(*a, *b));
                }
            }
            out
        }
        "unpair" | "do_not_pair" => two("a", "b"),
        "merge" => two("from", "into"),
        _ => BTreeSet::new(),
    }
}

/// What to do with an entry, given the entries this node already applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Apply,
    /// A later decision on the same tracks was applied first; this one is
    /// kept in the log but changes nothing.
    Superseded(GlobalId),
}

/// Judge an entry that arrives against those already applied. Decisions
/// about whether two tracks are one object (pair, unpair, merge, do not
/// pair) and edits of the same group: the later stamp wins. Everything else
/// applies whatever arrived before it: a delete holds until undone, and an
/// undo always runs.
pub fn judge(entry: &Entry, applied: &[Entry]) -> Verdict {
    let later = applied
        .iter()
        .filter(|e| e.hlc > entry.hlc && e.id != entry.id);
    let mine = pairs(&entry.command);
    if !mine.is_empty() {
        for e in later.clone() {
            if !pairs(&e.command).is_disjoint(&mine) {
                return Verdict::Superseded(e.id);
            }
        }
    }
    if entry.op().starts_with("profile_") {
        for e in later.clone() {
            if e.op() == entry.op() {
                return Verdict::Superseded(e.id);
            }
        }
    }
    if entry.op() == "group_update" {
        let g = &entry.command["group"];
        for e in later {
            if e.op() == "group_update" && &e.command["group"] == g {
                return Verdict::Superseded(e.id);
            }
        }
    }
    Verdict::Apply
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn entry(id: &str, hlc: u64, command: Value) -> Entry {
        Entry {
            id: id.parse().unwrap(),
            hlc: Hlc::new(hlc, 0),
            actor: "tm@example".into(),
            role: "track_manager".into(),
            command,
        }
    }

    #[test]
    fn ids_read_back() {
        let id: GlobalId = "AAA:12".parse().unwrap();
        assert_eq!(id.to_string(), "AAA:12");
        assert_eq!(serde_json::to_value(id).unwrap(), json!("AAA:12"));
        for bad in ["AAA12", "aaa:1", "AAA:0", "AAA:x", ":3"] {
            assert!(bad.parse::<GlobalId>().is_err(), "{bad}");
        }
    }

    #[test]
    fn targets_are_every_track_named() {
        let t = targets(&json!({
            "op": "group_members", "group": "tms-AAA000000009",
            "add": ["tms-AAA000000001", "BBB000000002"], "remove": null,
        }));
        let t: Vec<String> = t.iter().map(|u| u.to_string()).collect();
        assert_eq!(t, ["AAA000000001", "AAA000000009", "BBB000000002"]);
    }

    #[test]
    fn the_later_word_on_a_pair_of_tracks_wins_whatever_order_it_arrives_in() {
        let pair = entry(
            "AAA:1",
            100,
            json!({"op": "pair", "tracks": ["AAA000000001", "BBB000000002"]}),
        );
        let dnp = entry(
            "BBB:1",
            200,
            json!({"op": "do_not_pair", "a": "BBB000000002", "b": "AAA000000001"}),
        );
        // In order: both apply, and the later (do not pair) is last.
        assert_eq!(judge(&pair, &[]), Verdict::Apply);
        assert_eq!(judge(&dnp, std::slice::from_ref(&pair)), Verdict::Apply);
        // The earlier arrives last: it changes nothing.
        assert_eq!(
            judge(&pair, std::slice::from_ref(&dnp)),
            Verdict::Superseded(dnp.id)
        );
    }

    #[test]
    fn unrelated_and_non_conflicting_decisions_apply_in_any_order() {
        let pair = entry(
            "AAA:1",
            100,
            json!({"op": "pair", "tracks": ["AAA000000001", "BBB000000002"]}),
        );
        let group = entry(
            "BBB:1",
            200,
            json!({"op": "group_create", "group": "BBB000000005", "members": ["AAA000000001"]}),
        );
        let delete = entry(
            "BBB:2",
            300,
            json!({"op": "delete", "tracks": ["AAA000000001"]}),
        );
        let undo = entry("BBB:3", 50, json!({"op": "undo", "decision": "AAA:1"}));
        assert_eq!(
            judge(&pair, &[group.clone(), delete.clone()]),
            Verdict::Apply
        );
        assert_eq!(judge(&undo, &[pair.clone(), group, delete]), Verdict::Apply);
    }

    #[test]
    fn the_later_edit_of_a_group_wins() {
        let a = entry(
            "AAA:4",
            100,
            json!({"op": "group_update", "group": "AAA000000009", "spec": {"name": "Alpha"}}),
        );
        let b = entry(
            "BBB:7",
            200,
            json!({"op": "group_update", "group": "AAA000000009", "spec": {"name": "Bravo"}}),
        );
        assert_eq!(
            judge(&a, std::slice::from_ref(&b)),
            Verdict::Superseded(b.id)
        );
        assert_eq!(judge(&b, &[a]), Verdict::Apply);
    }

    #[test]
    fn the_later_profile_wins() {
        let a = entry(
            "AAA:4",
            100,
            json!({"op": "profile_correlation", "settings": {}}),
        );
        let b = entry(
            "BBB:7",
            200,
            json!({"op": "profile_correlation", "settings": {}}),
        );
        let s = entry("BBB:8", 300, json!({"op": "profile_schema", "fields": []}));
        assert_eq!(
            judge(&a, std::slice::from_ref(&b)),
            Verdict::Superseded(b.id)
        );
        assert_eq!(judge(&b, std::slice::from_ref(&s)), Verdict::Apply);
    }

    #[test]
    fn only_track_management_replicates() {
        assert!(replicated("merge") && replicated("undo") && replicated("group_create"));
        assert!(!replicated("split") && !replicated("purge") && !replicated("accept"));
    }
}
