//! Source-independent references to reports behind a particular observation.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// A record within the input frame accepted by a configured source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReportRef {
    pub source_id: String,
    pub frame_sha256: String,
    pub capture_profile_sha256: String,
    pub record_sha256: Option<String>,
    pub record_index: usize,
}

/// Retained original report. This lives on the restricted evidence path.
#[derive(Clone, Serialize, Deserialize)]
pub struct OriginalReport {
    pub reference: ReportRef,
    pub format: String,
    pub native_report_id: Option<serde_json::Value>,
    /// Original record bytes, encoded as base64. Absence records a codec
    /// that could not isolate the original bytes for this record.
    pub original_base64: Option<String>,
    /// Markings as found at the configured path in the original decoded record.
    pub marking: OriginalMarking,
}

impl std::fmt::Debug for OriginalReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OriginalReport")
            .field("reference", &self.reference)
            .finish_non_exhaustive()
    }
}

/// A missing marking differs from a source with no configured marking extraction.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum OriginalMarking {
    Present(serde_json::Value),
    Absent,
    Unavailable,
}

/// Report references that have survived derivation. Incomplete input cannot
/// be made complete by merging it with known input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lineage {
    pub reports: BTreeSet<ReportRef>,
    pub incomplete: BTreeSet<String>,
}

impl Lineage {
    pub fn combine<'a>(inputs: impl IntoIterator<Item = Option<&'a Self>>) -> Self {
        let mut lineage = Self::default();
        for input in inputs {
            if let Some(input) = input {
                lineage.reports.extend(input.reports.iter().cloned());
                lineage.incomplete.extend(input.incomplete.iter().cloned());
                if input.reports.is_empty() && input.incomplete.is_empty() {
                    lineage
                        .incomplete
                        .insert("input_lineage_unavailable".into());
                }
            } else {
                lineage
                    .incomplete
                    .insert("input_lineage_unavailable".into());
            }
        }
        lineage
    }
}
