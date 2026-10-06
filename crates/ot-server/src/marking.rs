//! The marking every exported file carries (see docs/guides/admin.md,
//! "Security labels"). Each item's own marking is
//! [`ot_core::SecurityLabel::marking`]; a file's overall marking is:
//!
//! - when no item in it has a label (or it has no labelled items at all,
//!   like the audit record): the classification banner's text as configured
//!   (Settings → Banners), whether or not the banner is shown;
//! - otherwise the marking of the items' labels combined as a fused track's
//!   are (the highest classification, every restriction, only the
//!   releasability they share), with the banner's text counted as the
//!   label of any unlabelled item, so a file is never marked below the
//!   system it came from.

use ot_core::SecurityLabel;
use serde_json::Value;

use crate::control::{ApiError, AppState};

/// What a file's marking is worked out from: the banner's text and the
/// classification order fused labels are chosen by.
#[derive(Debug, Clone)]
pub struct Marker {
    pub banner: String,
    pub order: Vec<String>,
}

impl Marker {
    /// From saved instance settings (`app_settings`) and correlation
    /// settings, as the database holds them.
    pub fn from_saved(app_settings: &Value, correlation: Option<&Value>) -> Self {
        let banner = app_settings
            .get("banner")
            .cloned()
            .and_then(|b| serde_json::from_value::<crate::settings_api::BannerSettings>(b).ok())
            .unwrap_or_default()
            .text;
        let order = correlation
            .cloned()
            .and_then(|c| serde_json::from_value::<crate::correlate::CorrelationSettings>(c).ok())
            .unwrap_or_default()
            .labels
            .classification_order;
        Self {
            banner: banner.trim().to_owned(),
            order,
        }
    }

    pub async fn load(s: &AppState) -> Result<Self, ApiError> {
        let (app, correlation) = s
            .with_db(|db| Ok((db.app_settings()?, db.correlation_settings()?)))
            .await?;
        Ok(Self::from_saved(&app, correlation.as_ref()))
    }

    /// The overall marking of a file holding items with these labels.
    pub fn file<'a>(&self, items: impl IntoIterator<Item = Option<&'a SecurityLabel>>) -> String {
        let banner = SecurityLabel {
            classification: self.banner.clone(),
            restrictions: Vec::new(),
            sharing: None,
        };
        let (mut labels, mut unlabelled) = (Vec::new(), false);
        for l in items {
            match l {
                Some(l) => labels.push(l),
                None => unlabelled = true,
            }
        }
        if labels.is_empty() {
            return self.banner.clone();
        }
        if unlabelled && !self.banner.is_empty() {
            labels.push(&banner);
        }
        SecurityLabel::combine(labels, &self.order)
            .map(|l| l.marking())
            .unwrap_or_else(|| self.banner.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn label(c: &str, sharing: Option<&str>) -> SecurityLabel {
        SecurityLabel {
            classification: c.into(),
            restrictions: Vec::new(),
            sharing: sharing.map(str::to_owned),
        }
    }

    #[test]
    fn a_file_is_marked_by_its_highest_label_else_the_banner() {
        let m = Marker::from_saved(
            &json!({"banner": {"enabled": false, "text": "SECRET//NOFORN"}}),
            None,
        );
        assert_eq!(m.banner, "SECRET//NOFORN");
        // Nothing labelled: the banner as it is written.
        assert_eq!(m.file([]), "SECRET//NOFORN");
        assert_eq!(m.file([None, None]), "SECRET//NOFORN");
        let (u, c) = (
            label("U", Some("USA, GBR")),
            label("CONFIDENTIAL", Some("USA")),
        );
        let m = Marker::from_saved(&json!({"banner": {"text": "UNCLASSIFIED"}}), None);
        assert_eq!(m.file([Some(&u), Some(&c)]), "(C//REL TO USA)");
        // An unlabelled item counts as the banner's classification.
        let s = Marker::from_saved(&json!({"banner": {"text": "SECRET"}}), None);
        assert_eq!(s.file([Some(&u), None]), "(S//REL TO USA, GBR)");
        assert_eq!(s.file([Some(&u)]), "(U//REL TO USA, GBR)");
        // Never saved: the default banner text.
        assert_eq!(
            Marker::from_saved(&json!({}), None).file([]),
            "UNCLASSIFIED"
        );
    }
}
