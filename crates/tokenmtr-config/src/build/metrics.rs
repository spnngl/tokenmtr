//! Validation of `[metrics]`: exporters, remote-write outputs, their filters and credentials.

use std::collections::BTreeMap;
use std::net::{AddrParseError, IpAddr, Ipv4Addr, SocketAddr};

use jiff::SignedDuration;
use secrecy::{ExposeSecret, SecretString};
use tokenmtr_core::SinkId;
use url::{Host, Url};

use super::{Checked, absolute_path, duration_or, expand_tilde, invalid_glob, reject};
use crate::env::Env;
use crate::error::{ConfigPath, Violation};
use crate::model::{
    Auth, ExporterConfig, ExternalLabels, GlobList, METRIC_LABELS, MetricsConfig, OutputFilter,
    RedactColumn, RedactMode, RemoteWriteConfig, SecretSource,
};
use crate::raw::{RawAuth, RawAuthKind, RawConfig, RawDuration, RawExporter, RawRemoteWrite};

const MIN_REFRESH_INTERVAL: SignedDuration = SignedDuration::from_secs(5);
const MIN_PUSH_INTERVAL: SignedDuration = SignedDuration::from_secs(10);
const MIN_SESSION_WINDOW: SignedDuration = SignedDuration::from_hours(1);
const DEFAULT_LISTEN: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9464);

/// The key names of one kind of secret.
struct SecretKeys {
    inline: &'static str,
    env: &'static str,
    file: &'static str,
    /// The three keys for the error message.
    options: &'static str,
}

const PASSWORD: SecretKeys = SecretKeys {
    inline: "password",
    env: "password_env",
    file: "password_file",
    options: "`password`, `password_env` or `password_file`",
};
const TOKEN: SecretKeys = SecretKeys {
    inline: "token",
    env: "token_env",
    file: "token_file",
    options: "`token`, `token_env` or `token_file`",
};

pub(super) fn build_metrics(raw: &RawConfig, env: &dyn Env) -> Checked<MetricsConfig> {
    let at = ConfigPath::root().key("metrics");
    let metrics = &raw.metrics;
    let redact_paths = raw.privacy.redact_paths.unwrap_or(false);
    let refresh_interval = at_least(
        metrics.refresh_interval,
        SignedDuration::from_secs(30),
        (MIN_REFRESH_INTERVAL, "5s"),
        &at.key("refresh_interval"),
    )?;
    let session_window = at_least(
        metrics.session_window,
        SignedDuration::from_hours(48),
        (MIN_SESSION_WINDOW, "1h"),
        &at.key("session_window"),
    )?;
    // Sink ids and output ids share one namespace.
    let mut ids: Vec<&SinkId> = raw.sinks.iter().map(|sink| &sink.id).collect();
    let mut exporters: Vec<ExporterConfig> = Vec::with_capacity(metrics.exporters.len());
    for (i, exporter) in metrics.exporters.iter().enumerate() {
        let at = at.key("exporters").index(i);
        claim_id(&mut ids, &exporter.id, &at)?;
        let built = build_exporter(exporter, redact_paths, env, &at)?;
        if let Some(other) = exporters.iter().find(|e| e.listen == built.listen) {
            return Err(reject(
                &at.key("listen"),
                Violation::ListenClash(other.id.clone()),
            ));
        }
        exporters.push(built);
    }
    let mut remote_write = Vec::with_capacity(metrics.remote_write.len());
    for (i, output) in metrics.remote_write.iter().enumerate() {
        let at = at.key("remote_write").index(i);
        claim_id(&mut ids, &output.id, &at)?;
        remote_write.push(build_remote_write(output, redact_paths, env, &at)?);
    }
    Ok(MetricsConfig {
        refresh_interval,
        session_window,
        exporters,
        remote_write,
    })
}

/// `value`, or `default`, when it is at least `min` (the duration and how config spells it).
pub(super) fn at_least(
    value: Option<RawDuration>,
    default: SignedDuration,
    (min, written): (SignedDuration, &'static str),
    at: &ConfigPath,
) -> Checked<SignedDuration> {
    let value = duration_or(value, default);
    if value < min {
        return Err(reject(at, Violation::IntervalTooShort { min: written }));
    }
    Ok(value)
}

fn claim_id<'a>(ids: &mut Vec<&'a SinkId>, id: &'a SinkId, at: &ConfigPath) -> Checked<()> {
    if ids.contains(&id) {
        return Err(reject(
            &at.key("id"),
            Violation::DuplicateOutputId(id.clone()),
        ));
    }
    ids.push(id);
    Ok(())
}

fn build_exporter(
    raw: &RawExporter,
    redact_paths: bool,
    env: &dyn Env,
    at: &ConfigPath,
) -> Checked<ExporterConfig> {
    let listen = match raw.listen.as_deref() {
        None => DEFAULT_LISTEN,
        Some(text) => text.parse().map_err(|e: AddrParseError| {
            reject(
                &at.key("listen"),
                Violation::InvalidListen {
                    value: text.to_owned(),
                    reason: e.to_string(),
                },
            )
        })?,
    };
    Ok(ExporterConfig {
        id: raw.id.clone(),
        listen,
        auth: build_auth(raw.auth.as_ref(), env, &at.key("auth"))?,
        filter: build_filter(
            [
                raw.exclude_projects.as_deref(),
                raw.exclude_paths.as_deref(),
            ],
            raw.redact.as_ref(),
            redact_paths,
            env,
            at,
        )?,
    })
}

fn build_remote_write(
    raw: &RawRemoteWrite,
    redact_paths: bool,
    env: &dyn Env,
    at: &ConfigPath,
) -> Checked<RemoteWriteConfig> {
    Ok(RemoteWriteConfig {
        id: raw.id.clone(),
        url: build_url(&raw.url, &at.key("url"))?,
        interval: at_least(
            raw.interval,
            SignedDuration::from_mins(1),
            (MIN_PUSH_INTERVAL, "10s"),
            &at.key("interval"),
        )?,
        filter: build_filter(
            [
                raw.exclude_projects.as_deref(),
                raw.exclude_paths.as_deref(),
            ],
            raw.redact.as_ref(),
            redact_paths,
            env,
            at,
        )?,
        tenant: raw
            .tenant
            .as_deref()
            .map(|tenant| printable(tenant, &at.key("tenant")))
            .transpose()?,
        auth: build_auth(raw.auth.as_ref(), env, &at.key("auth"))?,
        external_labels: build_external_labels(
            raw.external_labels.as_ref(),
            &at.key("external_labels"),
        )?,
    })
}

/// `exclusions` is `[exclude_projects, exclude_paths]` as written.
fn build_filter(
    exclusions: [Option<&[String]>; 2],
    redact: Option<&BTreeMap<RedactColumn, RedactMode>>,
    redact_paths: bool,
    env: &dyn Env,
    at: &ConfigPath,
) -> Checked<OutputFilter> {
    let [projects, paths] = exclusions;
    let exclude_projects = globs(projects, env, &at.key("exclude_projects"))?;
    let exclude_paths = globs(paths, env, &at.key("exclude_paths"))?;
    if redact_paths && !exclude_paths.is_empty() {
        return Err(reject(
            &at.key("exclude_paths"),
            Violation::ExcludePathsWithRedactedPaths,
        ));
    }
    let redact = redact.cloned().unwrap_or_default();
    if redact.contains_key(&RedactColumn::Cwd) {
        return Err(reject(
            &at.key("redact").key("cwd"),
            Violation::NotAMetricLabel("cwd"),
        ));
    }
    Ok(OutputFilter {
        exclude_projects,
        exclude_paths,
        redact,
    })
}

pub(super) fn globs(
    patterns: Option<&[String]>,
    env: &dyn Env,
    at: &ConfigPath,
) -> Checked<GlobList> {
    let mut expanded = Vec::new();
    for (i, pattern) in patterns.unwrap_or_default().iter().enumerate() {
        expanded.push(expand_tilde(env, pattern, &at.index(i))?);
    }
    GlobList::new(expanded.clone()).map_err(|e| {
        let i = expanded
            .iter()
            .position(|pattern| Some(pattern.as_str()) == e.glob())
            .unwrap_or(0);
        invalid_glob(&at.index(i), e.glob().unwrap_or_default(), &e)
    })
}

/// An https URL, or http to this machine, without credentials.
fn build_url(text: &str, at: &ConfigPath) -> Checked<Url> {
    let invalid = |reason: &str| {
        reject(
            at,
            Violation::InvalidUrl {
                reason: reason.to_owned(),
            },
        )
    };
    let url = Url::parse(text).map_err(|e| invalid(&e.to_string()))?;
    let Some(host) = url.host() else {
        return Err(invalid("no host"));
    };
    if !url.username().is_empty() || url.password().is_some() {
        return Err(reject(at, Violation::UrlCredentials));
    }
    let loopback = match host {
        Host::Domain(name) => name == "localhost",
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    };
    match url.scheme() {
        "https" => Ok(url),
        "http" if loopback => Ok(url),
        "http" => Err(reject(at, Violation::InsecureUrl)),
        _ => Err(invalid("the scheme must be https or http")),
    }
}

fn build_auth(raw: Option<&RawAuth>, env: &dyn Env, at: &ConfigPath) -> Checked<Option<Auth>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let not_allowed = |kind: &'static str, field: &'static str, present: bool| {
        if present {
            Err(reject(
                &at.key(field),
                Violation::AuthFieldNotAllowed { kind, field },
            ))
        } else {
            Ok(())
        }
    };
    match raw.kind {
        RawAuthKind::Basic => {
            not_allowed("basic", "token", raw.token.is_some())?;
            not_allowed("basic", "token_env", raw.token_env.is_some())?;
            not_allowed("basic", "token_file", raw.token_file.is_some())?;
            let username = raw
                .username
                .as_deref()
                .ok_or_else(|| reject(at, Violation::UsernameRequired))?;
            let username_at = at.key("username");
            if username.contains(':') {
                return Err(reject(&username_at, Violation::UsernameColon));
            }
            Ok(Some(Auth::Basic {
                username: printable(username, &username_at)?,
                password: secret_source(
                    &PASSWORD,
                    (
                        raw.password.as_ref(),
                        raw.password_env.as_deref(),
                        raw.password_file.as_deref(),
                    ),
                    env,
                    at,
                )?,
            }))
        }
        RawAuthKind::Bearer => {
            not_allowed("bearer", "username", raw.username.is_some())?;
            not_allowed("bearer", "password", raw.password.is_some())?;
            not_allowed("bearer", "password_env", raw.password_env.is_some())?;
            not_allowed("bearer", "password_file", raw.password_file.is_some())?;
            Ok(Some(Auth::Bearer {
                token: secret_source(
                    &TOKEN,
                    (
                        raw.token.as_ref(),
                        raw.token_env.as_deref(),
                        raw.token_file.as_deref(),
                    ),
                    env,
                    at,
                )?,
            }))
        }
    }
}

/// The one source among an inline value, an env variable name and a file path. Env and file
/// sources are not read here: an unset variable or a missing file fails the output when it
/// starts (`docs/config.md`).
fn secret_source(
    keys: &SecretKeys,
    (inline, env_name, file): (Option<&SecretString>, Option<&str>, Option<&str>),
    env: &dyn Env,
    at: &ConfigPath,
) -> Checked<SecretSource> {
    match (inline, env_name, file) {
        (Some(value), None, None) => {
            printable(value.expose_secret(), &at.key(keys.inline))?;
            Ok(SecretSource::Inline(value.clone()))
        }
        (None, Some(name), None) => Ok(SecretSource::Env(printable(name, &at.key(keys.env))?)),
        (None, None, Some(path)) => Ok(SecretSource::File(absolute_path(
            env,
            path,
            &at.key(keys.file),
        )?)),
        _ => Err(reject(
            at,
            Violation::SecretSourceCount {
                options: keys.options,
            },
        )),
    }
}

/// `text` when it is not blank and has no control character.
fn printable(text: &str, at: &ConfigPath) -> Checked<String> {
    if text.trim().is_empty() {
        return Err(reject(at, Violation::BlankValue));
    }
    if text.chars().any(char::is_control) {
        return Err(reject(at, Violation::NotPrintable));
    }
    Ok(text.to_owned())
}

fn build_external_labels(
    raw: Option<&BTreeMap<String, String>>,
    at: &ConfigPath,
) -> Checked<ExternalLabels> {
    let Some(raw) = raw else {
        return Ok(ExternalLabels::Machine);
    };
    for (name, value) in raw {
        let at = at.key(name);
        if !is_label_name(name) {
            return Err(reject(&at, Violation::InvalidLabelName(name.clone())));
        }
        if METRIC_LABELS.contains(&name.as_str()) {
            return Err(reject(&at, Violation::ReservedLabel(name.clone())));
        }
        printable(value, &at)?;
    }
    Ok(ExternalLabels::Explicit(raw.clone()))
}

/// A Prometheus label name that is not reserved (`__` prefix).
fn is_label_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with("__")
}
