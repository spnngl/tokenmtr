//! Provider quota windows as observed by polling.

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ids::IdError;

/// Identifies a quota window kind, e.g. `five_hour`, `seven_day` or `weekly_scoped:fable`.
/// Never blank.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(String);

impl WindowId {
    /// The rolling five-hour window.
    pub const FIVE_HOUR: &'static str = "five_hour";
    /// The rolling seven-day window.
    pub const SEVEN_DAY: &'static str = "seven_day";
    /// Prefix of a weekly window scoped to one model, followed by the model's slug.
    pub const WEEKLY_SCOPED_PREFIX: &'static str = "weekly_scoped:";

    /// Validates a window id.
    ///
    /// # Errors
    ///
    /// [`IdError::Blank`] for an empty or whitespace-only id.
    pub fn new(value: impl Into<String>) -> Result<Self, IdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(IdError::Blank { kind: "window" });
        }
        Ok(Self(value))
    }

    /// The five-hour window id.
    #[must_use]
    pub fn five_hour() -> Self {
        Self(Self::FIVE_HOUR.to_owned())
    }

    /// The seven-day window id.
    #[must_use]
    pub fn seven_day() -> Self {
        Self(Self::SEVEN_DAY.to_owned())
    }

    /// The monthly extra-usage window id ([`crate::extra_usage::EXTRA_USAGE`]).
    #[must_use]
    pub fn extra_usage() -> Self {
        Self(crate::extra_usage::EXTRA_USAGE.to_owned())
    }

    /// The weekly window scoped to the model the provider names `display_name` (`Fable` gives
    /// `weekly_scoped:fable`). The slug keeps lowercase letters, digits and dots; any other run
    /// of characters becomes one `-`. The model changes over time, so no name is hardcoded.
    ///
    /// # Errors
    ///
    /// [`IdError::Blank`] when the name has no letter, digit or dot.
    pub fn weekly_scoped(display_name: &str) -> Result<Self, IdError> {
        let slug = model_slug(display_name);
        if slug.is_empty() {
            return Err(IdError::Blank { kind: "window" });
        }
        Ok(Self(format!("{}{slug}", Self::WEEKLY_SCOPED_PREFIX)))
    }

    /// The model slug of a [`WindowId::weekly_scoped`] window, `None` for the other windows.
    #[must_use]
    pub fn scoped_model(&self) -> Option<&str> {
        self.0.strip_prefix(Self::WEEKLY_SCOPED_PREFIX)
    }

    /// The human name of the window, the one source of every label (UI, alerts, exports):
    /// `5-hour`, `7-day`, `7-day Sonnet`, `Weekly · Fable`, `Extra usage`; an unknown id reads
    /// with spaces for underscores.
    #[must_use]
    pub fn label(&self) -> String {
        if let Some(model) = self.scoped_model() {
            return format!("Weekly · {}", title_words(model));
        }
        match self.0.as_str() {
            Self::FIVE_HOUR => "5-hour".to_owned(),
            Self::SEVEN_DAY => "7-day".to_owned(),
            crate::extra_usage::EXTRA_USAGE => "Extra usage".to_owned(),
            other => match other.strip_prefix("seven_day_") {
                Some(model) => format!("7-day {}", title_words(model)),
                None => other.replace('_', " "),
            },
        }
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `Claude Opus 4.5` → `claude-opus-4.5`.
fn model_slug(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() || c == '.' {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_owned()
}

/// `claude-opus-4.5` → `Claude Opus 4.5`, `sonnet` → `Sonnet`.
fn title_words(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl std::fmt::Display for WindowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for WindowId {
    type Error = IdError;
    fn try_from(value: String) -> Result<Self, IdError> {
        Self::new(value)
    }
}

impl TryFrom<&str> for WindowId {
    type Error = IdError;
    fn try_from(value: &str) -> Result<Self, IdError> {
        Self::new(value)
    }
}

impl Serialize for WindowId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WindowId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Why a utilization was rejected.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("utilization must be finite and non-negative, got {0}")]
pub struct BadUtilization(pub f64);

/// Percent of a quota window used: finite and non-negative; may exceed 100.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
pub struct Utilization(f64);

impl Utilization {
    /// Validates a percentage.
    ///
    /// # Errors
    ///
    /// [`BadUtilization`] for NaN, infinite or negative values.
    pub fn new(percent: f64) -> Result<Self, BadUtilization> {
        if percent.is_finite() && percent >= 0.0 {
            Ok(Self(percent))
        } else {
            Err(BadUtilization(percent))
        }
    }

    /// The percentage.
    #[must_use]
    pub const fn percent(self) -> f64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for Utilization {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(f64::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// One window's state at one poll.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindowState {
    /// Percent used.
    pub utilization: Utilization,
    /// When the window resets, when the provider says.
    pub resets_at: Option<Timestamp>,
}

/// One window in a [`QuotaSnapshot`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindow {
    /// Which window.
    pub id: WindowId,
    /// Its state.
    #[serde(flatten)]
    pub state: QuotaWindowState,
}

/// All quota windows reported by one poll.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    /// When the poll happened.
    pub fetched_at: Timestamp,
    /// The windows reported.
    pub windows: Vec<QuotaWindow>,
}

impl QuotaSnapshot {
    /// The state of one window, if reported.
    #[must_use]
    pub fn window(&self, id: &WindowId) -> Option<&QuotaWindowState> {
        self.windows.iter().find(|w| &w.id == id).map(|w| &w.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_ids_reject_blank() {
        assert!(WindowId::new("  ").is_err());
        assert_eq!(WindowId::five_hour().as_str(), "five_hour");
    }

    #[test]
    fn weekly_scoped_ids_are_slugs_of_the_model_name() -> Result<(), IdError> {
        let fable = WindowId::weekly_scoped("Fable")?;
        assert_eq!(fable.as_str(), "weekly_scoped:fable");
        assert_eq!(fable.scoped_model(), Some("fable"));
        assert_eq!(fable.label(), "Weekly · Fable");
        let opus = WindowId::weekly_scoped("  Claude Opus 4.5 (beta) ")?;
        assert_eq!(opus.as_str(), "weekly_scoped:claude-opus-4.5-beta");
        assert_eq!(opus.label(), "Weekly · Claude Opus 4.5 Beta");
        assert!(WindowId::weekly_scoped(" -/ ").is_err());
        assert_eq!(WindowId::five_hour().scoped_model(), None);
        Ok(())
    }

    #[test]
    fn labels_name_every_window() -> Result<(), IdError> {
        let label = |id: &str| WindowId::new(id).map(|id| id.label());
        assert_eq!(label("five_hour")?, "5-hour");
        assert_eq!(label("seven_day")?, "7-day");
        assert_eq!(label("seven_day_sonnet")?, "7-day Sonnet");
        assert_eq!(label("seven_day_opus")?, "7-day Opus");
        assert_eq!(label("extra_usage")?, "Extra usage");
        assert_eq!(label("monthly_thing")?, "monthly thing");
        Ok(())
    }

    #[test]
    fn utilization_rejects_garbage() {
        assert!(Utilization::new(f64::NAN).is_err());
        assert!(Utilization::new(-0.1).is_err());
        assert!(Utilization::new(f64::INFINITY).is_err());
        assert!(Utilization::new(120.0).is_ok_and(|u| u > Utilization(100.0)));
    }

    #[test]
    fn snapshot_round_trips_through_json() -> Result<(), Box<dyn std::error::Error>> {
        let snap = QuotaSnapshot {
            fetched_at: "2026-01-01T00:00:00Z".parse()?,
            windows: vec![QuotaWindow {
                id: WindowId::five_hour(),
                state: QuotaWindowState {
                    utilization: Utilization::new(42.5)?,
                    resets_at: None,
                },
            }],
        };
        let json = serde_json::to_string(&snap)?;
        assert_eq!(serde_json::from_str::<QuotaSnapshot>(&json)?, snap);
        assert!(serde_json::from_str::<QuotaSnapshot>(&json.replace("42.5", "-1")).is_err());
        Ok(())
    }
}
