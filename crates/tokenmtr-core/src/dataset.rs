//! The exportable datasets (D17, Phase 7): one revision counter, one cursor per sink, one export
//! table or file family each.

use std::fmt;

use serde::{Deserialize, Serialize};

/// One exportable dataset. Serializes as its export name (`usage_events`, ...), which config,
/// the store, the control protocol and the export files all share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dataset {
    /// `usage_events`.
    UsageEvents,
    /// `tool_calls`.
    ToolCalls,
    /// `subscriptions`.
    Subscriptions,
    /// `quota_snapshots`.
    QuotaSnapshots,
    /// `fx_rates`.
    FxRates,
}

impl Dataset {
    /// Every dataset, in export order.
    pub const ALL: [Self; 5] = [
        Self::UsageEvents,
        Self::ToolCalls,
        Self::Subscriptions,
        Self::QuotaSnapshots,
        Self::FxRates,
    ];

    /// The export name: the serde form, the key in `dataset_revision` and `sink_cursor`, and the
    /// export table or file prefix.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UsageEvents => "usage_events",
            Self::ToolCalls => "tool_calls",
            Self::Subscriptions => "subscriptions",
            Self::QuotaSnapshots => "quota_snapshots",
            Self::FxRates => "fx_rates",
        }
    }

    /// Parses an export name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.as_str() == name)
    }
}

impl fmt::Display for Dataset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::Dataset;

    #[test]
    fn serde_form_is_the_export_name() -> Result<(), serde_json::Error> {
        for dataset in Dataset::ALL {
            assert_eq!(
                serde_json::to_string(&dataset)?,
                format!("\"{}\"", dataset.as_str())
            );
            assert_eq!(Dataset::from_name(dataset.as_str()), Some(dataset));
        }
        Ok(())
    }
}
