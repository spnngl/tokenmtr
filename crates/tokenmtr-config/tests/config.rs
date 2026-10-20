//! Behavior of `tokenmtr-config`: the documented example, every invalid case, comment-preserving
//! edits and defaults.
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "clippy''s allow-*-in-tests covers #[test] functions, not the shared helpers"
)]

use camino::Utf8Path;
use jiff::SignedDuration;
use jiff::civil::date;
use tokenmtr_config::{
    ConfigDocument, ConfigError, ConfigFormat, FileFormat, GitConfig, GitName, LineCol, LogFormat,
    MAX_MACHINE_NAME_BYTES, MenubarRing, Partition, ProfileLocation, RawProject, SinkTarget,
    TimeZoneSetting, Violation, load, load_str,
};
use tokenmtr_core::{
    AgentId, BillingPeriod, Currency, EurMicros, ModelId, Money, Override, Plan, ProfileId,
    ProjectKey, ProviderId, SubscriptionId, SubscriptionSpec, UsdMicros,
};

const EXAMPLE: &str = include_str!("fixtures/example.toml");
const DOCS: &str = include_str!("../../../docs/config.md");

fn env(key: &str) -> Option<String> {
    (key == "HOME").then(|| "/home/u".to_owned())
}

fn parse(source: &str) -> Result<tokenmtr_config::Config, ConfigError> {
    load_str(source, toml(), &env)
}

fn invalid(source: &str) -> (String, Violation, LineCol) {
    match parse(source).unwrap_err() {
        ConfigError::Invalid {
            path,
            violation,
            at,
        } => (path.to_string(), *violation, at.position.unwrap()),
        other => panic!("expected a rule violation, got {other}"),
    }
}

fn syntax(source: &str) -> (String, LineCol) {
    match parse(source).unwrap_err() {
        ConfigError::Syntax { at, message } => (message, at.position.unwrap()),
        other => panic!("expected a syntax error, got {other}"),
    }
}

fn toml() -> ConfigFormat {
    ConfigFormat::from_path(Utf8Path::new("c.toml")).unwrap()
}

fn at(line: usize, column: usize) -> LineCol {
    LineCol { line, column }
}

// -- the example and the docs ------------------------------------------------------------------

#[test]
fn docs_embed_the_example_verbatim() {
    let block = DOCS
        .split("```toml\n")
        .nth(1)
        .and_then(|rest| rest.split("```\n").next())
        .unwrap();
    assert_eq!(
        block, EXAMPLE,
        "docs/config.md drifted from tests/fixtures/example.toml"
    );
}

#[test]
fn the_example_parses() {
    let config = parse(EXAMPLE).unwrap();

    assert_eq!(config.display.currency, Currency::Usd);
    assert!(matches!(config.display.timezone, TimeZoneSetting::System));
    assert_eq!(config.quota.interval, SignedDuration::from_mins(5));
    assert_eq!(config.menubar.agents, [AgentId::CLAUDE, AgentId::PI]);
    assert_eq!(
        config.menubar.rings,
        [MenubarRing::Session, MenubarRing::Weekly]
    );
    assert_eq!(config.menubar.hidden_profiles, Vec::<ProfileId>::new());
    assert_eq!(config.menubar.live_window, SignedDuration::from_mins(10));
    assert_eq!(config.alerts.rules.quota_thresholds, [80, 95]);
    assert_eq!(config.alerts.rules.budget_thresholds, [80, 100]);
    assert_eq!(
        config.alerts.daemon_stale_after,
        SignedDuration::from_mins(15)
    );
    assert_eq!(config.advisor.headroom.get(), 80);
    assert_eq!(config.backup.keep, 14);
    assert_eq!(config.backup.interval, SignedDuration::from_hours(24));
    assert_eq!(config.git.default_remote.as_str(), "origin");
    assert_eq!(config.git.default_branch, None);

    let rule = &config.projects[0];
    assert_eq!(rule.pattern(), "/home/u/work/acme-*/**");
    assert!(rule.matches(Utf8Path::new("/home/u/work/acme-api/src/lib")));
    assert!(!rule.matches(Utf8Path::new("/home/u/work/other/src")));
    assert_eq!(rule.client.as_ref().unwrap().as_str(), "Acme Corp");

    let budget = &config.budgets[0].budget;
    assert_eq!(budget.amount(), Money::Eur(EurMicros::new(300_000_000)));

    assert_eq!(config.pricing.overrides.len(), 1);

    let subscription = &config.subscriptions[0];
    assert_eq!(subscription.plan(), Plan::Max20x);
    assert_eq!(subscription.fee(), Money::Usd(UsdMicros::new(200_000_000)));
    assert_eq!(subscription.start(), date(2025, 6, 1));
    assert_eq!(subscription.agents(), [AgentId::CLAUDE, AgentId::PI]);

    let claude = &config.agents[&AgentId::CLAUDE];
    assert!(claude.enabled);
    assert_eq!(claude.profiles.len(), 1);
    assert_eq!(claude.profiles[0].location, ProfileLocation::Default);

    let sinks = &config.sinks;
    assert_eq!(sinks.len(), 3);
    assert!(matches!(
        &sinks[0].target,
        SinkTarget::File(f) if f.format == FileFormat::Csv && f.partition == Partition::Month
    ));
    assert!(matches!(&sinks[1].target, SinkTarget::Sqlite(s) if s.path.is_none()));
    assert!(matches!(
        &sinks[2].target,
        SinkTarget::File(f)
            if f.format == FileFormat::Jsonl
                && f.dir.as_deref() == Some(Utf8Path::new("/home/u/Library/CloudStorage/Dropbox/tokenmtr"))
    ));
    assert_eq!(sinks[2].redact.len(), 2);
    assert_eq!(sinks[0].datasets.len(), 5);

    let metrics = &config.metrics;
    assert_eq!(metrics.refresh_interval, SignedDuration::from_secs(30));
    assert_eq!(metrics.session_window, SignedDuration::from_hours(48));
    assert_eq!(metrics.exporters.len(), 2);
    assert_eq!(metrics.exporters[0].listen.to_string(), "127.0.0.1:9464");
    assert_eq!(metrics.exporters[1].listen.to_string(), "0.0.0.0:9465");
    assert!(
        metrics.exporters[1]
            .filter
            .exclude_paths
            .is_match("/home/u/work/private/a/b")
    );
    assert_eq!(metrics.remote_write.len(), 1);
    assert_eq!(
        metrics.remote_write[0].interval,
        SignedDuration::from_mins(1)
    );
}

#[test]
fn a_pricing_override_may_name_a_provider() {
    let config = parse(
        "[[pricing.overrides]]\nmodel = \"m\"\nprovider = \"bedrock\"\ninput_per_m = 3\noutput_per_m = 15\n",
    )
    .unwrap();
    let expected = Override::new(
        ModelId::try_from("m").unwrap(),
        "3".parse().unwrap(),
        "15".parse().unwrap(),
        None,
        None,
        None,
    )
    .unwrap()
    .for_provider(ProviderId::try_from("bedrock").unwrap());
    assert_eq!(config.pricing.overrides, [expected]);
}

#[test]
fn an_empty_file_is_the_default_config() {
    let config = parse("").unwrap();

    assert_eq!(config.display.currency, Currency::Usd);
    assert!(config.quota.enabled);
    assert_eq!(config.quota.interval, SignedDuration::from_mins(5));
    assert_eq!(config.menubar.agents, [AgentId::CLAUDE, AgentId::PI]);
    assert_eq!(
        config.menubar.rings,
        [MenubarRing::Session, MenubarRing::Weekly]
    );
    assert_eq!(
        config.menubar.shortcut.as_deref(),
        Some("Alt+CommandOrControl+U")
    );
    assert_eq!(config.alerts.rules, tokenmtr_core::AlertConfig::default());
    assert_eq!(config.advisor.headroom.get(), 80);
    assert_eq!(config.backup.keep, 14);
    assert!(config.projects.is_empty() && config.budgets.is_empty());
    assert_eq!(config.git, GitConfig::default());
    assert_eq!(config.git.default_remote, GitName::origin());
    assert!(config.subscriptions.is_empty() && config.sinks.is_empty());
    assert!(config.metrics.exporters.is_empty() && config.metrics.remote_write.is_empty());
    assert_eq!(
        config.metrics.refresh_interval,
        SignedDuration::from_secs(30)
    );
    assert_eq!(
        config.metrics.session_window,
        SignedDuration::from_hours(48)
    );
    assert!(config.pricing.auto_update);
    assert!(!config.privacy.redact_paths);
    assert_eq!(config.logging.format, LogFormat::Text);
    assert!(config.store.path.is_none());
    for agent in [AgentId::CLAUDE, AgentId::PI] {
        let profiles = &config.agents[&agent].profiles;
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, ProfileId::default_profile());
    }
}

#[test]
fn the_log_format_is_text_or_json() {
    let config = parse("[logging]\nformat = \"json\"\n").unwrap();
    assert_eq!(config.logging.format, LogFormat::Json);
    assert!(parse("[logging]\nformat = \"xml\"\n").is_err());
    assert!(parse("[logging]\nlevel = \"info\"\n").is_err());
}

#[test]
fn fixed_rate_keeps_its_decimal_digits() {
    let config = parse("[fx]\nfixed_eur_per_usd = 0.92\n").unwrap();
    assert_eq!(config.fx.fixed_eur_per_usd.unwrap().scaled(), 920_000_000);
}

#[test]
fn load_reads_a_file_and_names_it_in_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = Utf8Path::from_path(dir.path()).unwrap().join("config.toml");

    std::fs::write(&path, "[display]\ncurrency = \"GBP\"\n").unwrap();
    let error = load(&path).unwrap_err();
    assert!(
        error.to_string().starts_with(&format!("{path}:2:12:")),
        "{error}"
    );

    std::fs::write(&path, EXAMPLE).unwrap();
    assert!(load(&path).is_ok());

    let other = path.with_extension("yaml");
    assert!(matches!(
        load(&other),
        Err(ConfigError::UnsupportedFormat { .. })
    ));
}

// -- invalid cases -----------------------------------------------------------------------------

#[test]
fn unknown_key_is_rejected() {
    let (message, position) = syntax("schema_version = 1\n\n[display]\ncurrenci = \"USD\"\n");
    assert!(message.contains("unknown field `currenci`"), "{message}");
    assert_eq!(position, at(4, 1));
}

#[test]
fn bad_duration_is_rejected() {
    let (message, position) = syntax("[quota]\ninterval = \"5x\"\n");
    assert!(message.contains("unknown duration unit"), "{message}");
    assert_eq!(position, at(2, 12));
}

#[test]
fn quota_interval_below_a_minute_is_rejected() {
    let (path, violation, position) = invalid("[quota]\ninterval = \"30s\"\n");
    assert_eq!(path, "quota.interval");
    assert!(matches!(violation, Violation::IntervalTooShort { .. }));
    assert_eq!(position, at(2, 12));
}

#[test]
fn the_machine_name_is_trimmed_and_a_blank_one_is_the_default() {
    let named = parse("[machine]\nname = \"  work-mbp \"\n").unwrap();
    assert_eq!(named.machine.name.as_deref(), Some("work-mbp"));
    let blank = parse("[machine]\nname = \"  \"\n").unwrap();
    assert_eq!(blank.machine.name, None);
}

#[test]
fn a_machine_name_that_is_no_label_value_is_rejected() {
    let (path, violation, position) = invalid("[machine]\nname = \"a\\tb\"\n");
    assert_eq!(path, "machine.name");
    assert!(matches!(violation, Violation::NotPrintable));
    assert_eq!(position, at(2, 8));
    let long = format!("[machine]\nname = \"{}\"\n", "é".repeat(65));
    let (_, violation, _) = invalid(&long);
    assert!(matches!(
        violation,
        Violation::TooLong {
            max_bytes: MAX_MACHINE_NAME_BYTES
        }
    ));
    let longest = format!("[machine]\nname = \"{}\"\n", "é".repeat(64));
    assert!(parse(&longest).is_ok());
}

#[test]
fn duplicate_sink_id_is_rejected() {
    let source =
        "[[sinks]]\nid = \"a\"\nkind = \"sqlite\"\n\n[[sinks]]\nid = \"a\"\nkind = \"csv\"\n";
    let (path, violation, position) = invalid(source);
    assert_eq!(path, "sinks[1].id");
    assert!(matches!(violation, Violation::DuplicateSinkId(id) if id.as_str() == "a"));
    assert_eq!(position, at(6, 6));
}

#[test]
fn sinks_writing_the_same_files_are_rejected() {
    let source =
        "[[sinks]]\nid = \"a\"\nkind = \"sqlite\"\n\n[[sinks]]\nid = \"b\"\nkind = \"sqlite\"\n";
    let (path, violation, position) = invalid(source);
    assert_eq!(path, "sinks[1].kind");
    assert!(matches!(violation, Violation::SinkTargetClash(id) if id.as_str() == "a"));
    assert_eq!(position, at(7, 8));

    let source = "[[sinks]]\nid = \"a\"\nkind = \"csv\"\ndatasets = [\"fx_rates\"]\n\n\
                  [[sinks]]\nid = \"b\"\nkind = \"csv\"\ndatasets = [\"usage_events\"]\n\n\
                  [[sinks]]\nid = \"c\"\nkind = \"jsonl\"\n";
    assert!(
        parse(source).is_ok(),
        "disjoint datasets or formats share a directory"
    );
    let (_, violation, _) = invalid(&format!(
        "{source}\n[[sinks]]\nid = \"d\"\nkind = \"csv\"\n"
    ));
    assert!(matches!(violation, Violation::SinkTargetClash(id) if id.as_str() == "a"));
}

#[test]
fn unknown_sink_kind_is_rejected() {
    let (message, position) = syntax("[[sinks]]\nid = \"w\"\nkind = \"postgres\"\n");
    assert!(message.contains("unknown variant `postgres`"), "{message}");
    assert_eq!(position, at(3, 8));
}

#[test]
fn sink_keys_must_fit_the_kind() {
    let (path, violation, _) =
        invalid("[[sinks]]\nid = \"a\"\nkind = \"sqlite\"\nmode = \"append\"\n");
    assert_eq!(path, "sinks[0].mode");
    assert!(matches!(
        violation,
        Violation::FieldNotAllowed {
            kind: "sqlite",
            field: "mode"
        }
    ));
}

const SUBSCRIPTION: &str = "\
[[subscriptions]]
id = \"a\"
provider = \"anthropic\"
plan = \"pro\"
agents = [\"claude\"]
period = \"monthly\"
start = \"2025-01-01\"
";

#[test]
fn overlapping_subscriptions_are_rejected() {
    let source = format!(
        "{SUBSCRIPTION}\n[[subscriptions]]\nid = \"b\"\nprovider = \"anthropic\"\nplan = \"max_5x\"\nagents = [\"claude\"]\nperiod = \"monthly\"\nstart = \"2025-03-01\"\n"
    );
    let (path, violation, position) = invalid(&source);
    assert_eq!(path, "subscriptions[1]");
    assert!(matches!(
        violation,
        Violation::OverlappingSubscriptions { first, second }
            if first.as_str() == "a" && second.as_str() == "b"
    ));
    assert_eq!(position.line, 9);
}

#[test]
fn subscriptions_with_disjoint_dates_are_accepted() {
    let source = format!(
        "{SUBSCRIPTION}end = \"2025-03-01\"\n\n[[subscriptions]]\nid = \"b\"\nprovider = \"anthropic\"\nplan = \"max_5x\"\nagents = [\"claude\"]\nperiod = \"monthly\"\nstart = \"2025-03-01\"\n"
    );
    assert_eq!(parse(&source).unwrap().subscriptions.len(), 2);
}

#[test]
fn menubar_rings_parse_and_must_be_unique() {
    let config =
        parse("[menubar]\nrings = [\"per_model\"]\nhidden_profiles = [\"work\"]\n").unwrap();
    assert_eq!(config.menubar.rings, [MenubarRing::PerModel]);
    assert_eq!(config.menubar.hidden_profiles.len(), 1);
    assert_eq!(
        parse("[menubar]\nrings = []\n").unwrap().menubar.rings,
        Vec::<MenubarRing>::new()
    );

    let (path, _, _) = invalid("[menubar]\nrings = [\"weekly\", \"weekly\"]\n");
    assert_eq!(path, "menubar.rings[1]");
    let (path, _, _) = invalid("[menubar]\nhidden_profiles = [\"a\", \"a\"]\n");
    assert_eq!(path, "menubar.hidden_profiles[1]");
}

#[test]
fn menubar_agents_must_be_enabled() {
    let source = "[menubar]\nagents = [\"claude\", \"pi\"]\n\n[agents.pi]\nenabled = false\n";
    let (path, violation, position) = invalid(source);
    assert_eq!(path, "menubar.agents[1]");
    assert!(matches!(violation, Violation::AgentDisabled(agent) if agent == AgentId::PI));
    assert_eq!(position, at(2, 21));
}

#[test]
fn team_plan_needs_an_explicit_fee() {
    let source = SUBSCRIPTION.replace("\"pro\"", "\"team\"");
    let (path, violation, _) = invalid(&source);
    assert_eq!(path, "subscriptions[0]");
    assert!(matches!(
        violation,
        Violation::FeeRequired {
            plan: Plan::Team,
            ..
        }
    ));

    let with_fee = format!("{source}fee = 30.0\n");
    assert_eq!(
        parse(&with_fee).unwrap().subscriptions[0].fee(),
        Money::Usd(UsdMicros::new(30_000_000))
    );
}

#[test]
fn eur_subscription_without_fee_is_rejected() {
    let source = format!("{SUBSCRIPTION}fee_currency = \"EUR\"\n");
    let (_, violation, _) = invalid(&source);
    assert!(matches!(
        violation,
        Violation::FeeRequired {
            currency: Currency::Eur,
            ..
        }
    ));
}

#[test]
fn profiles_of_one_agent_must_not_share_roots() {
    let source = "\
[[agents.claude.profiles]]
id = \"a\"
roots = [\"~/.claude/projects\"]

[[agents.claude.profiles]]
id = \"b\"
roots = [\"~/.claude/projects/work\"]
";
    let (path, violation, position) = invalid(source);
    assert_eq!(path, "agents.claude.profiles[1].roots");
    assert!(
        matches!(violation, Violation::SharedRoot { profile, other, .. }
        if profile.as_str() == "b" && other.as_str() == "a")
    );
    assert_eq!(position.line, 7);
}

#[test]
fn separate_roots_and_config_dirs_are_accepted() {
    let source = "\
[[agents.claude.profiles]]
id = \"a\"
roots = [\"~/.claude/projects\"]

[[agents.claude.profiles]]
id = \"work\"
config_dir = \"~/.claude-work\"
";
    let config = parse(source).unwrap();
    let profiles = &config.agents[&AgentId::CLAUDE].profiles;
    assert_eq!(profiles.len(), 3, "implicit default is added");
    assert_eq!(profiles[0].id, ProfileId::default_profile());
}

#[test]
fn budget_must_name_a_declared_profile() {
    let source = "[[budgets]]\nid = \"m\"\nperiod = \"month\"\namount = 10\nprofiles = [\"default\", \"work\"]\n";
    let (path, violation, position) = invalid(source);
    assert_eq!(path, "budgets[0].profiles[1]");
    assert!(matches!(violation, Violation::UnknownProfile(p) if p.as_str() == "work"));
    assert_eq!(position, at(5, 24));
}

#[test]
fn project_glob_must_parse() {
    let (path, violation, position) =
        invalid("[[projects]]\nmatch = \"~/work/[abc\"\nname = \"x\"\n");
    assert_eq!(path, "projects[0].match");
    assert!(matches!(violation, Violation::InvalidGlob { .. }));
    assert_eq!(position, at(2, 9));
}

#[test]
fn tilde_needs_a_home() {
    let result = load_str("[store]\npath = \"~/usage.db\"\n", toml(), &|_: &str| None);
    let Err(ConfigError::Invalid { violation, .. }) = result else {
        panic!("expected a rule violation");
    };
    assert!(matches!(*violation, Violation::HomeUnset { .. }));
}

#[test]
fn relative_paths_are_rejected() {
    let (_, violation, _) = invalid("[store]\npath = \"usage.db\"\n");
    assert!(matches!(violation, Violation::RelativePath { .. }));
}

#[test]
fn unsupported_schema_version_is_rejected() {
    let (_, violation, position) = invalid("schema_version = 2\n");
    assert!(matches!(
        violation,
        Violation::UnsupportedSchemaVersion { found: 2, .. }
    ));
    assert_eq!(position, at(1, 18));
}

#[test]
fn duplicate_budget_scope_entries_are_rejected() {
    let source = "[[budgets]]\nid = \"m\"\nperiod = \"month\"\namount = 10\nagents = [\"claude\", \"claude\"]\n";
    let (path, violation, _) = invalid(source);
    assert_eq!(path, "budgets[0].agents[1]");
    assert!(matches!(violation, Violation::DuplicateEntry(_)));
}

// -- editing -----------------------------------------------------------------------------------

const COMMENTED: &str = "\
# my config
schema_version = 1

[display]
currency = \"EUR\"  # shown everywhere

[[projects]]  # first match wins
match = \"~/work/acme-*/**\"
name = \"acme\"

# subscriptions go below
[daemon]
watch = true  # keep on
";

fn subscription_spec() -> SubscriptionSpec {
    SubscriptionSpec {
        id: SubscriptionId::try_from("claude-max").unwrap(),
        provider: ProviderId::try_from("anthropic").unwrap(),
        plan: Plan::Max20x,
        agents: vec![AgentId::CLAUDE],
        profiles: None,
        fee: Money::Usd(UsdMicros::new(200_000_000)),
        period: BillingPeriod::Monthly,
        start: date(2025, 6, 1),
        end: None,
    }
}

#[test]
fn adding_entries_keeps_comments_and_existing_text() {
    let mut document = ConfigDocument::new(COMMENTED, toml());

    document
        .add_subscription(&subscription_spec(), &env)
        .unwrap();
    let config = document
        .add_project_rule(
            &RawProject {
                pattern: "~/oss/**".to_owned(),
                name: ProjectKey::try_from("oss").unwrap(),
                client: None,
            },
            &env,
        )
        .unwrap();

    assert_eq!(config.subscriptions.len(), 1);
    assert_eq!(config.projects.len(), 2);
    assert_eq!(config.projects[1].name.as_str(), "oss");
    assert_eq!(
        document.text(),
        "\
# my config
schema_version = 1

[display]
currency = \"EUR\"  # shown everywhere

[[projects]]  # first match wins
match = \"~/work/acme-*/**\"
name = \"acme\"

[[projects]]
match = \"~/oss/**\"
name = \"oss\"

# subscriptions go below
[daemon]
watch = true  # keep on

[[subscriptions]]
id = \"claude-max\"
provider = \"anthropic\"
plan = \"max_20x\"
agents = [\"claude\"]
fee = 200.0
fee_currency = \"USD\"
period = \"monthly\"
start = \"2025-06-01\"
"
    );
}

#[test]
fn setting_menubar_rings_keeps_comments() {
    let mut document = ConfigDocument::new(EXAMPLE, toml());
    let work = ProfileId::try_from("work").unwrap();
    let config = document
        .set_menubar_rings(&[MenubarRing::PerModel], std::slice::from_ref(&work), &env)
        .unwrap();

    assert_eq!(config.menubar.rings, [MenubarRing::PerModel]);
    assert_eq!(config.menubar.hidden_profiles, [work]);
    assert!(document.text().contains(
        "rings = [\"per_model\"]  # quota rings: session | weekly | per_model; [] = plain icon\n"
    ));
    assert!(
        document
            .text()
            .contains("hidden_profiles = [\"work\"]           # profiles")
    );

    let mut bare = ConfigDocument::new("schema_version = 1\n", toml());
    bare.set_menubar_rings(&[], &[], &env).unwrap();
    assert_eq!(bare.text(), "schema_version = 1\n\n[menubar]\nrings = []\n");

    let error = bare
        .set_menubar_rings(&[MenubarRing::Weekly, MenubarRing::Weekly], &[], &env)
        .unwrap_err();
    assert!(matches!(error, ConfigError::Invalid { .. }), "{error}");
}

#[test]
fn git_names_must_be_non_blank_words() {
    let config =
        parse("[git]\ndefault_remote = \"upstream\"\ndefault_branch = \"main\"\n").unwrap();
    assert_eq!(config.git.default_remote.as_str(), "upstream");
    assert_eq!(
        config.git.default_branch.map(String::from).as_deref(),
        Some("main")
    );
    for bad in ["", "my remote"] {
        let source = format!("[git]\ndefault_remote = \"{bad}\"\n");
        assert!(parse(&source).is_err(), "{bad:?} accepted");
    }
}

#[test]
fn the_most_specific_account_wins() {
    let config = parse(
        "account = \"me\"\n\
         [agents.pi]\naccount = \"pi-team\"\n\
         [[agents.pi.profiles]]\nid = \"default\"\n\
         [[agents.pi.profiles]]\nid = \"work\"\nroots = [\"/w\"]\naccount = \"work@acme\"\n",
    )
    .unwrap();
    let account = |agent, profile: &str| {
        config
            .accounts
            .get(agent, &ProfileId::try_from(profile).unwrap())
            .map(|a| a.as_str().to_owned())
    };

    assert_eq!(account(AgentId::PI, "work").as_deref(), Some("work@acme"));
    assert_eq!(account(AgentId::PI, "default").as_deref(), Some("pi-team"));
    assert_eq!(account(AgentId::PI, "gone").as_deref(), Some("pi-team"));
    assert_eq!(account(AgentId::CLAUDE, "default").as_deref(), Some("me"));
    assert!(parse("").unwrap().accounts.is_empty());
}

#[test]
fn a_blank_account_is_rejected() {
    for source in [
        "account = \" \"\n",
        "[agents.claude]\naccount = \"\"\n",
        "[[agents.pi.profiles]]\nid = \"default\"\naccount = \"\"\n",
    ] {
        assert!(parse(source).is_err(), "{source:?} accepted");
    }
}

#[test]
fn setting_git_defaults_keeps_comments() {
    let mut document = ConfigDocument::new(EXAMPLE, toml());
    let upstream = GitName::try_from("upstream".to_owned()).unwrap();
    let main = GitName::try_from("main".to_owned()).unwrap();
    let config = document
        .set_git_defaults(&upstream, Some(&main), &env)
        .unwrap();
    assert_eq!(config.git.default_remote, upstream);
    assert_eq!(config.git.default_branch.as_ref(), Some(&main));
    assert!(document.text().contains(
        "default_remote = \"upstream\"      # its URL keys projects; else origin, else the first remote\n"
    ));
    assert!(document.text().contains("\ndefault_branch = \"main\"\n"));

    let config = document.set_git_defaults(&upstream, None, &env).unwrap();
    assert_eq!(config.git.default_branch, None);
    assert!(!document.text().contains("\ndefault_branch = "));
    assert!(document.text().contains("# default_branch = \"main\""));

    let mut bare = ConfigDocument::new("schema_version = 1\n", toml());
    bare.set_git_defaults(&GitName::origin(), None, &env)
        .unwrap();
    assert_eq!(
        bare.text(),
        "schema_version = 1\n\n[git]\ndefault_remote = \"origin\"\n"
    );
}

#[test]
fn a_rejected_edit_leaves_the_document_unchanged() {
    let mut document = ConfigDocument::new(COMMENTED, toml());
    document
        .add_subscription(&subscription_spec(), &env)
        .unwrap();
    let before = document.text().to_owned();

    let error = document
        .add_subscription(&subscription_spec(), &env)
        .unwrap_err();

    assert!(matches!(error, ConfigError::Invalid { .. }), "{error}");
    assert_eq!(document.text(), before);
}

#[test]
fn an_edited_example_still_parses() {
    let mut document = ConfigDocument::new(EXAMPLE, toml());
    let mut spec = subscription_spec();
    spec.id = SubscriptionId::try_from("extra").unwrap();
    spec.provider = ProviderId::try_from("openai").unwrap();
    document.add_subscription(&spec, &env).unwrap();

    assert!(document.text().starts_with(EXAMPLE.lines().next().unwrap()));
    assert!(document.text().contains("# end = \"2026-06-01\""));
    assert_eq!(document.config(&env).unwrap().subscriptions.len(), 2);
}
