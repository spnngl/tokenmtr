//! Network clients of `TokenMtr` (D13): Claude quota (D26), ECB FX rates (D27) and the `LiteLLM`
//! price list (D11). Each client is pure: it fetches and parses, then returns core types.
//! Persistence and scheduling belong to the daemon.
//!
//! The only production hosts are in [`ALLOWED_HOSTS`]. Every client takes its base URL
//! ([`Endpoints`]) so tests can use a mock server. OAuth tokens are
//! [`secrecy::SecretString`] and never reach `Debug`, errors or `tracing` (D55).

pub mod account;
pub mod credentials;
pub mod ecb;
mod error;
mod http;
pub mod litellm;
pub mod quota;

pub use account::{AccountInfo, ProfileParseError, parse_profile_response, plan_from_claude};
pub use credentials::{
    ClaudeCredentials, CredentialLocation, CredentialSource, CredentialsError,
    DEFAULT_KEYCHAIN_SERVICE, keychain_service_for_config_dir,
};
pub use ecb::{EcbFx, EcbParseError, FxError, FxFetch, parse_ecb_xml};
pub use error::{NetworkKind, RemoteError};
pub use http::{ALLOWED_HOSTS, Endpoints, USER_AGENT, Validators, build_client};
pub use litellm::{LitellmRefresh, RefreshOutcome, parse_catalog};
pub use quota::{
    ClaudeQuota, ExtraUsage, ParsedQuota, QuotaOutcome, QuotaParseError, QuotaPoll,
    parse_quota_response,
};
