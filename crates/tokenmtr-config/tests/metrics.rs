//! `[metrics]`: defaults and every rule on exporters, remote-write outputs, exclusions and
//! credentials.
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "clippy's allow-*-in-tests covers #[test] functions, not the shared helpers"
)]

use camino::Utf8Path;
use jiff::SignedDuration;
use tokenmtr_config::{
    Auth, Config, ConfigError, ConfigFormat, ExternalLabels, METRIC_LABELS, RedactColumn,
    RedactMode, SecretSource, Violation, load_str,
};

fn env(key: &str) -> Option<String> {
    (key == "HOME").then(|| "/home/u".to_owned())
}

fn parse(source: &str) -> Result<Config, ConfigError> {
    let format = ConfigFormat::from_path(Utf8Path::new("c.toml")).unwrap();
    load_str(source, format, &env)
}

/// The config path and the rule of the first violation.
fn invalid(source: &str) -> (String, Violation) {
    match parse(source).unwrap_err() {
        ConfigError::Invalid {
            path, violation, ..
        } => (path.to_string(), *violation),
        other => panic!("expected a rule violation, got {other}"),
    }
}

fn exporter(extra: &str) -> String {
    format!("[[metrics.exporters]]\nid = \"e\"\n{extra}\n")
}

fn remote_write(extra: &str) -> String {
    format!(
        "[[metrics.remote_write]]\nid = \"w\"\nurl = \"https://r.example/api/prom/push\"\n{extra}\n"
    )
}

#[test]
fn outputs_have_defaults() {
    let config = parse(&format!("{}{}", exporter(""), remote_write(""))).unwrap();
    let metrics = &config.metrics;
    assert_eq!(metrics.exporters[0].listen.to_string(), "127.0.0.1:9464");
    assert!(metrics.exporters[0].auth.is_none());
    assert!(metrics.exporters[0].filter.exclude_paths.is_empty());
    assert_eq!(
        metrics.remote_write[0].interval,
        SignedDuration::from_mins(1)
    );
    assert_eq!(
        metrics.remote_write[0].external_labels,
        ExternalLabels::Machine
    );
    assert_eq!(metrics.remote_write[0].tenant, None);
}

#[test]
fn exclusions_are_globs_where_star_stops_at_a_slash() {
    let config = parse(&exporter(
        "exclude_projects = [\"github.com/acme/*\"]\nexclude_paths = [\"~/work/*\", \"/tmp/**\"]",
    ))
    .unwrap();
    let filter = &config.metrics.exporters[0].filter;
    assert!(filter.exclude_projects.is_match("github.com/acme/app"));
    assert!(!filter.exclude_projects.is_match("github.com/acme/app/sub"));
    assert!(!filter.exclude_projects.is_match("github.com/other/app"));
    assert!(filter.exclude_paths.is_match("/home/u/work/app"));
    assert!(!filter.exclude_paths.is_match("/home/u/work/app/src"));
    assert!(filter.exclude_paths.is_match("/tmp/a/b"));
}

#[test]
fn an_invalid_glob_is_rejected_where_it_is_written() {
    let (path, violation) = invalid(&exporter("exclude_projects = [\"ok\", \"[a\"]"));
    assert_eq!(path, "metrics.exporters[0].exclude_projects[1]");
    assert!(matches!(violation, Violation::InvalidGlob { pattern, .. } if pattern == "[a"));
}

#[test]
fn exclude_paths_cannot_match_hashed_paths() {
    let source = format!(
        "[privacy]\nredact_paths = true\n{}",
        exporter("exclude_paths = [\"/x/*\"]")
    );
    let (path, violation) = invalid(&source);
    assert_eq!(path, "metrics.exporters[0].exclude_paths");
    assert!(matches!(
        violation,
        Violation::ExcludePathsWithRedactedPaths
    ));

    let source = format!(
        "[privacy]\nredact_paths = true\n{}",
        exporter("exclude_projects = [\"*/x\"]")
    );
    assert!(
        parse(&source).is_ok(),
        "project exclusion still works on hashed paths"
    );
}

#[test]
fn redact_names_labels_and_never_the_cwd() {
    let config = parse(&exporter(
        "redact = { account = \"hash\", session_id = \"drop\", skill = \"hash\" }",
    ))
    .unwrap();
    let redact = &config.metrics.exporters[0].filter.redact;
    assert_eq!(redact[&RedactColumn::Account], RedactMode::Hash);
    assert_eq!(redact[&RedactColumn::SessionId], RedactMode::Drop);
    assert_eq!(redact[&RedactColumn::Skill], RedactMode::Hash);

    let (path, violation) = invalid(&exporter("redact = { cwd = \"hash\" }"));
    assert_eq!(path, "metrics.exporters[0].redact.cwd");
    assert!(matches!(violation, Violation::NotAMetricLabel("cwd")));
}

#[test]
fn listen_must_parse_and_be_unique() {
    let config = parse(&exporter("listen = \"[::1]:9100\"")).unwrap();
    assert_eq!(config.metrics.exporters[0].listen.to_string(), "[::1]:9100");

    let (path, violation) = invalid(&exporter("listen = \"localhost\""));
    assert_eq!(path, "metrics.exporters[0].listen");
    assert!(matches!(violation, Violation::InvalidListen { value, .. } if value == "localhost"));

    let source = format!(
        "{}[[metrics.exporters]]\nid = \"f\"\nlisten = \"127.0.0.1:9464\"\n",
        exporter("")
    );
    let (path, violation) = invalid(&source);
    assert_eq!(path, "metrics.exporters[1].listen");
    assert!(matches!(violation, Violation::ListenClash(id) if id.as_str() == "e"));
}

#[test]
fn urls_are_https_or_this_machine() {
    for url in [
        "https://r.example/push",
        "http://localhost:9009/push",
        "http://127.0.0.1:9009/push",
        "http://[::1]:9009/push",
    ] {
        let source = format!("[[metrics.remote_write]]\nid = \"w\"\nurl = \"{url}\"\n");
        assert!(parse(&source).is_ok(), "{url}");
    }
    let rejected = |url: &str| {
        invalid(&format!(
            "[[metrics.remote_write]]\nid = \"w\"\nurl = \"{url}\"\n"
        ))
    };
    let (path, violation) = rejected("http://r.example/push");
    assert_eq!(path, "metrics.remote_write[0].url");
    assert!(matches!(violation, Violation::InsecureUrl));
    assert!(matches!(
        rejected("https://u:p@r.example/push").1,
        Violation::UrlCredentials
    ));
    assert!(matches!(
        rejected("https://u@r.example/push").1,
        Violation::UrlCredentials
    ));
    assert!(matches!(
        rejected("ftp://r.example/push").1,
        Violation::InvalidUrl { .. }
    ));
    assert!(matches!(
        rejected("r.example/push").1,
        Violation::InvalidUrl { .. }
    ));
}

#[test]
fn a_rejected_url_is_never_echoed() {
    for url in [
        "ftp://u:hunter2@r.example/p",
        "https://u:hunter2@r.example:99999/p",
    ] {
        let (_, violation) = invalid(&format!(
            "[[metrics.remote_write]]\nid = \"w\"\nurl = \"{url}\"\n"
        ));
        assert!(
            !format!("{violation} {violation:?}").contains("hunter2"),
            "{url}"
        );
    }
}

#[test]
fn ids_are_shared_with_sinks_and_across_outputs() {
    let sink = "[[sinks]]\nid = \"e\"\nkind = \"sqlite\"\n\n";
    let (path, violation) = invalid(&format!("{sink}{}", exporter("")));
    assert_eq!(path, "metrics.exporters[0].id");
    assert!(matches!(violation, Violation::DuplicateOutputId(id) if id.as_str() == "e"));

    let source = format!(
        "{}[[metrics.remote_write]]\nid = \"e\"\nurl = \"https://r.example/p\"\n",
        exporter("")
    );
    let (path, _) = invalid(&source);
    assert_eq!(path, "metrics.remote_write[0].id");

    let source = format!(
        "{}[[metrics.exporters]]\nid = \"e\"\nlisten = \"127.0.0.1:1\"\n",
        exporter("")
    );
    let (path, _) = invalid(&source);
    assert_eq!(path, "metrics.exporters[1].id");
}

#[test]
fn a_secret_has_exactly_one_source() {
    let bearer = |auth: &str| exporter(&format!("auth = {{ kind = \"bearer\", {auth} }}"));
    let config = parse(&bearer("token_env = \"SCRAPE_TOKEN\"")).unwrap();
    assert_eq!(
        config.metrics.exporters[0].auth,
        Some(Auth::Bearer {
            token: SecretSource::Env("SCRAPE_TOKEN".to_owned())
        })
    );
    let config = parse(&bearer("token_file = \"~/.config/t.secret\"")).unwrap();
    assert!(matches!(
        &config.metrics.exporters[0].auth,
        Some(Auth::Bearer { token: SecretSource::File(path) })
            if path == "/home/u/.config/t.secret"
    ));
    let config = parse(&bearer("token = \"glc_x\"")).unwrap();
    assert!(matches!(
        &config.metrics.exporters[0].auth,
        Some(Auth::Bearer {
            token: SecretSource::Inline(_)
        })
    ));

    for none_or_many in [
        "",
        "token = \"a\", token_env = \"B\"",
        "token_env = \"B\", token_file = \"/c\"",
    ] {
        let (path, violation) = invalid(&bearer(none_or_many));
        assert_eq!(path, "metrics.exporters[0].auth", "{none_or_many}");
        assert!(matches!(violation, Violation::SecretSourceCount { .. }));
    }
}

#[test]
fn env_and_file_secrets_are_not_read_by_the_config() {
    let source = remote_write(
        "auth = { kind = \"basic\", username = \"u\", password_env = \"TOKENMTR_UNSET_ANYWHERE\" }",
    );
    assert!(parse(&source).is_ok());
    let source = remote_write(
        "auth = { kind = \"basic\", username = \"u\", password_file = \"/no/such/file\" }",
    );
    assert!(parse(&source).is_ok());
}

#[test]
fn auth_keys_must_fit_the_kind() {
    let (path, violation) = invalid(&remote_write(
        "auth = { kind = \"basic\", username = \"u\", password = \"p\", token = \"t\" }",
    ));
    assert_eq!(path, "metrics.remote_write[0].auth.token");
    assert!(matches!(
        violation,
        Violation::AuthFieldNotAllowed {
            kind: "basic",
            field: "token"
        }
    ));
    let (path, violation) = invalid(&exporter(
        "auth = { kind = \"bearer\", token = \"t\", username = \"u\" }",
    ));
    assert_eq!(path, "metrics.exporters[0].auth.username");
    assert!(matches!(
        violation,
        Violation::AuthFieldNotAllowed {
            kind: "bearer",
            field: "username"
        }
    ));
    let (_, violation) = invalid(&remote_write(
        "auth = { kind = \"basic\", password = \"p\" }",
    ));
    assert!(matches!(violation, Violation::UsernameRequired));
    let (path, violation) = invalid(&remote_write(
        "auth = { kind = \"basic\", username = \"a:b\", password = \"p\" }",
    ));
    assert_eq!(path, "metrics.remote_write[0].auth.username");
    assert!(matches!(violation, Violation::UsernameColon));
}

#[test]
fn secret_values_are_checked_and_never_printed() {
    let (path, violation) = invalid(&exporter("auth = { kind = \"bearer\", token = \" \" }"));
    assert_eq!(path, "metrics.exporters[0].auth.token");
    assert!(matches!(violation, Violation::BlankValue));
    let (_, violation) = invalid(&exporter("auth = { kind = \"bearer\", token = \"a\\nb\" }"));
    assert!(matches!(violation, Violation::NotPrintable));
    let (_, violation) = invalid(&exporter(
        "auth = { kind = \"bearer\", token_file = \"relative.secret\" }",
    ));
    assert!(matches!(violation, Violation::RelativePath { .. }));

    let config = parse(&exporter(
        "auth = { kind = \"bearer\", token = \"hunter2-secret\" }",
    ))
    .unwrap();
    assert!(!format!("{config:?}").contains("hunter2-secret"));
}

#[test]
fn external_labels_are_valid_and_not_metric_labels() {
    let config = parse(&remote_write(
        "external_labels = { site = \"home\", _x1 = \"y\" }",
    ))
    .unwrap();
    let ExternalLabels::Explicit(labels) = &config.metrics.remote_write[0].external_labels else {
        panic!("expected explicit labels");
    };
    assert_eq!(labels["site"], "home");

    for bad in ["1a", "a-b", "__name__", "__x"] {
        let (path, violation) = invalid(&remote_write(&format!(
            "external_labels = {{ \"{bad}\" = \"v\" }}"
        )));
        assert_eq!(
            path,
            format!("metrics.remote_write[0].external_labels.{bad}")
        );
        assert!(
            matches!(violation, Violation::InvalidLabelName(name) if name == bad),
            "{bad}"
        );
    }
    for reserved in METRIC_LABELS {
        let (_, violation) = invalid(&remote_write(&format!(
            "external_labels = {{ {reserved} = \"v\" }}"
        )));
        assert!(
            matches!(violation, Violation::ReservedLabel(_)),
            "{reserved}"
        );
    }
    let (_, violation) = invalid(&remote_write("external_labels = { site = \"\" }"));
    assert!(matches!(violation, Violation::BlankValue));
}

#[test]
fn tenant_is_header_safe() {
    let config = parse(&remote_write("tenant = \"team-a\"")).unwrap();
    assert_eq!(
        config.metrics.remote_write[0].tenant.as_deref(),
        Some("team-a")
    );
    let (path, violation) = invalid(&remote_write("tenant = \" \""));
    assert_eq!(path, "metrics.remote_write[0].tenant");
    assert!(matches!(violation, Violation::BlankValue));
}

#[test]
fn intervals_have_minimums() {
    let config = parse("[metrics]\nrefresh_interval = \"5s\"\nsession_window = \"1h\"\n").unwrap();
    assert_eq!(
        config.metrics.refresh_interval,
        SignedDuration::from_secs(5)
    );
    assert_eq!(config.metrics.session_window, SignedDuration::from_hours(1));
    assert!(parse(&remote_write("interval = \"10s\"")).is_ok());

    for (source, key, min) in [
        (
            "[metrics]\nrefresh_interval = \"4s\"\n".to_owned(),
            "metrics.refresh_interval",
            "5s",
        ),
        (
            "[metrics]\nsession_window = \"59m\"\n".to_owned(),
            "metrics.session_window",
            "1h",
        ),
        (
            remote_write("interval = \"9s\""),
            "metrics.remote_write[0].interval",
            "10s",
        ),
    ] {
        let (path, violation) = invalid(&source);
        assert_eq!(path, key);
        assert!(
            matches!(violation, Violation::IntervalTooShort { min: m } if m == min),
            "{key}"
        );
    }
}

#[test]
fn unknown_metrics_keys_are_rejected() {
    for source in [
        "[metrics]\nenabled = true\n".to_owned(),
        exporter("enabled = true"),
        remote_write("batch = 1"),
        exporter("auth = { kind = \"bearer\", token = \"t\", scheme = \"x\" }"),
    ] {
        assert!(
            matches!(parse(&source), Err(ConfigError::Syntax { .. })),
            "{source}"
        );
    }
}
