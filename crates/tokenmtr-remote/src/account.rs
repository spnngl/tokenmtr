//! Claude account and plan detection. Holds non-secret fields only.

use serde::{Deserialize, Serialize};
use tokenmtr_core::Plan;

use crate::error::JsonPosition;

/// What is known about one Claude login. Every field is optional because each source (the
/// credentials file, the profile endpoint) fills only some of them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    /// Account email, from `/api/oauth/profile`.
    pub email: Option<String>,
    /// Organization name, from `/api/oauth/profile`.
    pub organization: Option<String>,
    /// The detected plan; `None` when Claude reports a type or tier this version cannot map.
    pub plan: Option<Plan>,
    /// Claude's raw rate-limit tier, e.g. `default_claude_max_20x`, kept for diagnostics.
    pub rate_limit_tier: Option<String>,
}

impl AccountInfo {
    /// Fills the fields this value lacks from `other`, keeping what it already has.
    #[must_use]
    pub fn or(self, other: Self) -> Self {
        Self {
            email: self.email.or(other.email),
            organization: self.organization.or(other.organization),
            plan: self.plan.or(other.plan),
            rate_limit_tier: self.rate_limit_tier.or(other.rate_limit_tier),
        }
    }
}

/// Maps Claude's `subscriptionType` (credentials) or `organization_type` (profile endpoint) and
/// `rateLimitTier` to a [`Plan`]. Case-insensitive; accepts the `claude_` prefix the profile
/// endpoint uses. `max` needs the tier (`..._max_5x`, `..._max_20x`) to pick a multiplier.
#[must_use]
pub fn plan_from_claude(
    subscription_type: Option<&str>,
    rate_limit_tier: Option<&str>,
) -> Option<Plan> {
    let normalized = subscription_type.map(|s| s.trim().to_ascii_lowercase());
    let kind = normalized
        .as_deref()
        .map(|s| s.strip_prefix("claude_").unwrap_or(s));
    let tier = rate_limit_tier.map(str::to_ascii_lowercase);
    let max_by_tier = || match tier.as_deref() {
        Some(t) if t.contains("max_20x") => Some(Plan::Max20x),
        Some(t) if t.contains("max_5x") => Some(Plan::Max5x),
        _ => None,
    };
    match kind {
        Some("pro") => Some(Plan::Pro),
        Some("team") => Some(Plan::Team),
        Some("enterprise") => Some(Plan::Enterprise),
        Some("max_20x") => Some(Plan::Max20x),
        Some("max_5x") => Some(Plan::Max5x),
        Some("max") | None => max_by_tier(),
        Some(_) => None,
    }
}

/// `/api/oauth/profile` response; only the fields we read.
#[derive(Debug, Deserialize)]
struct ProfileResponse {
    account: Option<ProfileAccount>,
    organization: Option<ProfileOrganization>,
}

#[derive(Debug, Deserialize)]
struct ProfileAccount {
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProfileOrganization {
    name: Option<String>,
    organization_type: Option<String>,
    rate_limit_tier: Option<String>,
}

/// Why a profile body was rejected: category and position only, never the offending value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("profile response is not the expected JSON ({0})")]
pub struct ProfileParseError(JsonPosition);

/// Parses an `/api/oauth/profile` body.
///
/// # Errors
/// [`ProfileParseError`] if the body is not a JSON object of the expected shape.
pub fn parse_profile_response(body: &[u8]) -> Result<AccountInfo, ProfileParseError> {
    let profile: ProfileResponse =
        serde_json::from_slice(body).map_err(|e| ProfileParseError(e.into()))?;
    let (organization, organization_type, rate_limit_tier) = match profile.organization {
        Some(o) => (o.name, o.organization_type, o.rate_limit_tier),
        None => (None, None, None),
    };
    Ok(AccountInfo {
        email: profile.account.and_then(|a| a.email),
        organization,
        plan: plan_from_claude(organization_type.as_deref(), rate_limit_tier.as_deref()),
        rate_limit_tier,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_credentials_subscription_types() {
        let max20 = Some("default_claude_max_20x");
        assert_eq!(plan_from_claude(Some("max"), max20), Some(Plan::Max20x));
        assert_eq!(
            plan_from_claude(Some("Max"), Some("default_claude_max_5x")),
            Some(Plan::Max5x)
        );
        assert_eq!(plan_from_claude(Some("pro"), None), Some(Plan::Pro));
        assert_eq!(plan_from_claude(Some("team"), None), Some(Plan::Team));
        assert_eq!(
            plan_from_claude(Some("enterprise"), None),
            Some(Plan::Enterprise)
        );
    }

    #[test]
    fn maps_profile_organization_types() {
        assert_eq!(plan_from_claude(Some("claude_pro"), None), Some(Plan::Pro));
        assert_eq!(
            plan_from_claude(Some("claude_max"), Some("default_claude_max_5x")),
            Some(Plan::Max5x)
        );
        assert_eq!(
            plan_from_claude(Some("claude_team"), None),
            Some(Plan::Team)
        );
    }

    #[test]
    fn unknown_or_ambiguous_is_none() {
        assert_eq!(plan_from_claude(Some("max"), None), None);
        assert_eq!(
            plan_from_claude(Some("free"), Some("default_claude_max_20x")),
            None
        );
        assert_eq!(plan_from_claude(None, None), None);
    }

    #[test]
    fn tier_alone_identifies_max() {
        assert_eq!(
            plan_from_claude(None, Some("default_claude_max_20x")),
            Some(Plan::Max20x)
        );
    }

    #[test]
    fn or_keeps_existing_fields() {
        let a = AccountInfo {
            plan: Some(Plan::Pro),
            ..AccountInfo::default()
        };
        let b = AccountInfo {
            plan: Some(Plan::Team),
            email: Some("a@b.c".into()),
            ..AccountInfo::default()
        };
        let merged = a.or(b);
        assert_eq!(merged.plan, Some(Plan::Pro));
        assert_eq!(merged.email.as_deref(), Some("a@b.c"));
    }
}
