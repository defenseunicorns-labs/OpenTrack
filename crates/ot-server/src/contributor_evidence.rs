//! Opt-in contributor evidence publication settings and output.

use std::{collections::BTreeMap, path::Path, time::Duration};

use ot_core::evidence::{Lineage, OriginalReport, ReportRef, digest};
use ot_nats::Outgoing;
use ot_source::pipeline::ReportEvidenceSettings;
use serde::{Deserialize, Serialize};

const CONFIG: &str = "OT_CONTRIBUTOR_EVIDENCE_CONFIG";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Settings {
    pub stream: String,
    pub subject_prefix: String,
    pub retention_seconds: u64,
    pub sources: BTreeMap<String, ReportEvidenceSettings>,
}

impl Settings {
    pub fn load() -> anyhow::Result<Option<Self>> {
        std::env::var_os(CONFIG)
            .map(|path| Self::read(Path::new(&path)))
            .transpose()
    }

    fn read(path: &Path) -> anyhow::Result<Self> {
        let settings: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        anyhow::ensure!(
            token(&settings.stream)
                && settings.subject_prefix.split('.').all(token)
                && settings.retention_seconds > 0,
            "invalid contributor evidence stream, subject, or retention"
        );
        Ok(settings)
    }

    pub fn retention(&self) -> Duration {
        Duration::from_secs(self.retention_seconds)
    }

    pub fn check_track_stream(&self, stream: &str, prefix: &str) -> anyhow::Result<()> {
        let overlaps = |a: &str, b: &str| {
            a == b
                || a.strip_prefix(b)
                    .is_some_and(|suffix| suffix.starts_with('.'))
        };
        anyhow::ensure!(
            self.stream != stream
                && !overlaps(prefix, &self.subject_prefix)
                && !overlaps(&self.subject_prefix, prefix),
            "contributor evidence must use a separate stream and disjoint subjects"
        );
        Ok(())
    }

    pub fn outgoing(&self, kind: &str, identity: &str, body: Vec<u8>) -> Outgoing {
        Outgoing {
            subject: format!("{}.{kind}.{identity}", self.subject_prefix),
            msg_id: format!("contributor-evidence:{kind}:{identity}"),
            headers: vec![("Nats-Expected-Stream", self.stream.clone())],
            body: body.into(),
        }
    }
}

fn token(token: &str) -> bool {
    !token.is_empty()
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Publication {
    pub stream: String,
    pub subject_prefix: String,
    pub update_id: String,
    pub track_json: Vec<u8>,
    pub report_refs: std::collections::BTreeSet<ReportRef>,
    pub manifest_json: Vec<u8>,
}

#[derive(Serialize)]
struct Manifest<'a> {
    schema: &'static str,
    publisher_id: &'a str,
    update_id: &'a str,
    track_sha256: String,
    complete: bool,
    reports: Vec<ManifestReport<'a>>,
    incomplete_reasons: std::collections::BTreeSet<String>,
}

#[derive(Serialize)]
struct ManifestReport<'a> {
    reference: &'a ReportRef,
    report_id: Option<String>,
}

impl Publication {
    pub fn new(
        settings: &Settings,
        track_json: Vec<u8>,
        publisher_id: &str,
        update_id: &str,
        mut lineage: Lineage,
        report_records: Vec<OriginalReport>,
    ) -> serde_json::Result<Self> {
        if lineage.reports.is_empty() {
            lineage
                .incomplete
                .insert("input_lineage_unavailable".into());
        }
        if lineage.reports.iter().any(|reference| {
            !report_records
                .iter()
                .any(|report| report.reference == *reference)
        }) {
            lineage
                .incomplete
                .insert("original_report_unavailable".into());
        }
        let reports = lineage
            .reports
            .iter()
            .map(|reference| {
                let available = report_records
                    .iter()
                    .any(|record| record.reference == *reference);
                Ok(ManifestReport {
                    reference,
                    report_id: available.then(|| report_id(reference)).transpose()?,
                })
            })
            .collect::<serde_json::Result<_>>()?;
        let manifest_json = serde_json::to_vec(&Manifest {
            schema: "opentrack.contributor-evidence.v1",
            publisher_id,
            update_id,
            track_sha256: digest(&track_json),
            complete: lineage.incomplete.is_empty(),
            reports,
            incomplete_reasons: lineage.incomplete,
        })?;
        Ok(Self {
            stream: settings.stream.clone(),
            subject_prefix: settings.subject_prefix.clone(),
            update_id: update_id.to_owned(),
            track_json,
            report_refs: report_records
                .into_iter()
                .map(|report| report.reference)
                .collect(),
            manifest_json,
        })
    }
}

pub(crate) fn report_id(reference: &ReportRef) -> serde_json::Result<String> {
    Ok(digest(&serde_json::to_vec(reference)?))
}
