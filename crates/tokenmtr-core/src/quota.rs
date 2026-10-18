//! Provider quota windows as observed by polling.

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::ids::IdError;

/// Identifies a quota window kind, e.g. `five_hour` or `seven_day`. Never blank.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(String);

impl WindowId {
    /// The rolling five-hour window.
    pub const FIVE_HOUR: &'static str = "five_hour";
    /// The rolling seven-day window.
    pub const SEVEN_DAY: &'static str = "seven_day";

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

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
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
