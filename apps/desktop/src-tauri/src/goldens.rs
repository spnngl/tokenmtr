//! Golden command responses shared with the UI.
//!
//! Every `apps/desktop/src/shared/schemas/__fixtures__/<name>.json` is the pretty serde JSON of a
//! value built here from the real response types, and `golden.test.ts` parses each through the
//! UI's Zod schemas. A field renamed or retyped on either side fails one of the two tests.
//! `just goldens` (`UPDATE_GOLDENS=1`) rewrites the files; otherwise the test fails when stale.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use jiff::Timestamp;
use jiff::civil::Date;
use serde::Serialize;
use serde_json::Value;
use tokenmtr_config::MenubarRing;
use tokenmtr_core::{
    Advice, AgentId, AlertKind, BillingPeriod, BudgetMeasure, BudgetPeriod, CostSource, Currency,
    Dataset, EventKind, Plan, PlanFit, QuotaEta, QuotaSizing, Recommendation, SignalKind,
    TokenCounts, UsdMicros, Verdict, WindowId,
};
use tokenmtr_query::{
    AdvisorReport, AgentRecap, AlertView, BackupView, BillingAmounts, BillingMonth, BillingPoint,
    BillingReport, BillingSeries, BranchRow, Breakdown, Bucket, BudgetState, BudgetView,
    COST_EDGES_MICROS, CostBucket, DaemonHealth, DaemonState, DateRange, FxRateView, Granularity,
    GroupBy, GroupValue, HealthLevel, HeatCell, Heatmap, IngestStatus, Live, ModelRow,
    ModelSessions, ModelSlice, NO_GROUP, ParseIssueView, Period, ProjectModelsRow, ProjectRemote,
    ProjectRow, QuotaHistory, QuotaMeter, QuotaPoint, QuotaSpend, QuotaWindowView, STATUS_SCHEMA,
    Series, SeriesPoint, SessionAnalytics, SessionCompactions, SessionDay, SessionDetail,
    SessionEvent, SessionModel, SessionPage, SessionPoint, SessionRow, SessionStartCell,
    SessionTimeline, SessionToolCall, SignalDay, SignalModel, SignalsReport, SinkHealth,
    SinkRunView, SinkState, SparkPoint, Status, SubscriptionLine, SystemHealth, TimelineEvent,
    TimelineSignal, TimelineTool, ToolModelCalls, ToolRow, ToolsReport, Totals, UnresolvedCwd,
    Usage,
};
use tokenmtr_store::{IssueKind, QuotaStatus};

use crate::backend::actions::GeneratedReport;
use crate::backend::queries::recap_document;
use crate::backend::shell::{
    AgentInfo, AlertsBlock, AppState, Capabilities, DaemonBlock, DisplayBlock, FilterOptions,
    FxBlock, GitBlock, Permission, PricingBlock, ProfileInfo, ProtectedFolder, SessionOrigin,
    Settings, TrayBlock,
};
use crate::request::FormatWire;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../src/shared/schemas/__fixtures__")
}

fn json(value: impl Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn day(text: &str) -> Date {
    text.parse().unwrap()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// Token counts `[input_uncached, cache_read, cache_write_5m, cache_write_1h, output, reasoning]`.
type Tokens = [u64; 6];
/// Money `[api_value, paid, subscription_value, subscription_fee]` in micros.
type Money = [i64; 4];
/// Cost sources `[reported, estimated, unpriced]`.
type Sources = [u64; 3];

/// A [`Usage`] whose total and USD value follow from the parts (USD responses).
#[expect(
    clippy::similar_names,
    reason = "the bindings are the cache-write column names"
)]
fn usage(events: u64, tokens: Tokens, money: Money, sources: Sources, approximate: bool) -> Usage {
    let [
        input_uncached,
        cache_read,
        cache_write_5m,
        cache_write_1h,
        output,
        reasoning,
    ] = tokens;
    let [
        api_value_micros,
        paid_micros,
        subscription_value_micros,
        subscription_fee_micros,
    ] = money;
    let [reported_events, estimated_events, unpriced_events] = sources;
    Usage {
        events,
        input_uncached,
        cache_read,
        cache_write_5m,
        cache_write_1h,
        output,
        reasoning,
        total_tokens: input_uncached + cache_read + cache_write_5m + cache_write_1h + output,
        api_value_micros,
        api_value_usd_micros: api_value_micros,
        paid_micros,
        extra_usage_micros: 0,
        subscription_value_micros,
        subscription_fee_micros,
        reported_events,
        estimated_events,
        unpriced_events,
        approximate,
    }
}

const SESSION: &str = "0b7c1d2e-5f60-4a8b-9c1d-2e3f4a5b6c7d";
const PI_SESSION: &str = "2026-05-02T09-14-00";

/// The selected period's usage: the overview and the billing total, with $12.345 of Claude
/// extra usage inside paid.
fn period_usage() -> Usage {
    Usage {
        extra_usage_micros: 12_345_000,
        ..usage(
            1840,
            [412_300, 98_234_500, 2_104_300, 512_000, 1_873_400, 120_500],
            [184_523_100, 24_685_000, 172_183_100, 100_000_000],
            [214, 1618, 8],
            true,
        )
    }
}

fn overview() -> Value {
    json(Totals {
        currency: Currency::Usd,
        currency_fallback: false,
        usage: period_usage(),
        cached_input_percent: 97.4,
        cache_savings_micros: Some(265_233_150),
        roi: Some(1.72),
        sessions: 63,
    })
}

fn models() -> Value {
    json(Breakdown {
        currency: Currency::Usd,
        rows: vec![
            ModelRow {
                model: "claude-sonnet-4-5".to_owned(),
                usage: usage(
                    1402,
                    [301_200, 80_120_400, 1_704_100, 412_000, 1_420_300, 90_200],
                    [121_400_200, 0, 121_400_200, 0],
                    [0, 1402, 0],
                    false,
                ),
                share_percent: 65.8,
                reported_vs_estimated_percent: None,
                divergent: false,
            },
            ModelRow {
                model: "gpt-5".to_owned(),
                usage: usage(
                    214,
                    [61_000, 9_100_000, 0, 0, 210_400, 30_300],
                    [12_340_000, 12_340_000, 0, 0],
                    [214, 0, 0],
                    false,
                ),
                share_percent: 6.7,
                reported_vs_estimated_percent: Some(112.5),
                divergent: true,
            },
        ],
    })
}

fn projects() -> Value {
    json(Breakdown {
        currency: Currency::Usd,
        rows: vec![
            ProjectRow {
                project: Some("tokenmtr".to_owned()),
                name: Some("TokenMtr".to_owned()),
                client: None,
                usage: usage(
                    1310,
                    [290_100, 70_100_200, 1_500_200, 380_000, 1_301_200, 80_100],
                    [131_200_400, 8_100_000, 123_100_400, 0],
                    [150, 1156, 4],
                    false,
                ),
                share_percent: 71.1,
            },
            ProjectRow {
                project: Some("acme-web".to_owned()),
                name: Some("Acme storefront".to_owned()),
                client: Some("Acme Corp".to_owned()),
                usage: usage(
                    410,
                    [98_000, 24_000_100, 504_100, 112_000, 480_100, 30_200],
                    [45_100_300, 4_240_000, 40_860_300, 0],
                    [64, 344, 2],
                    true,
                ),
                share_percent: 24.4,
            },
        ],
    })
}

fn project_models() -> Value {
    json(Breakdown {
        currency: Currency::Usd,
        rows: vec![
            ProjectModelsRow {
                project: Some("tokenmtr".to_owned()),
                name: Some("TokenMtr".to_owned()),
                models: vec![
                    ModelSlice {
                        model: "claude-sonnet-4-5".to_owned(),
                        usage: usage(
                            1180,
                            [260_100, 64_100_200, 1_400_200, 380_000, 1_201_200, 70_100],
                            [118_200_400, 7_100_000, 111_100_400, 0],
                            [100, 1076, 4],
                            false,
                        ),
                    },
                    ModelSlice {
                        model: "gpt-5".to_owned(),
                        usage: usage(
                            130,
                            [30_000, 6_000_000, 100_000, 0, 100_000, 10_000],
                            [13_000_000, 1_000_000, 12_000_000, 0],
                            [50, 80, 0],
                            false,
                        ),
                    },
                ],
            },
            ProjectModelsRow {
                project: None,
                name: None,
                models: vec![ModelSlice {
                    model: "claude-sonnet-4-5".to_owned(),
                    usage: usage(
                        37,
                        [8_000, 1_200_000, 30_000, 0, 20_000, 0],
                        [2_410_300, 0, 2_410_300, 0],
                        [0, 37, 0],
                        true,
                    ),
                }],
            },
        ],
    })
}

fn project_remotes() -> Value {
    json(vec![
        ProjectRemote {
            name: "origin".to_owned(),
            host: "github.com".to_owned(),
            repository: "ada/tokenmtr".to_owned(),
            default_branch: Some("main".to_owned()),
            keys_project: true,
        },
        ProjectRemote {
            name: "upstream".to_owned(),
            host: "gitlab.com".to_owned(),
            repository: "acme/platform/tokenmtr".to_owned(),
            default_branch: None,
            keys_project: false,
        },
    ])
}

fn branches() -> Value {
    json(Breakdown {
        currency: Currency::Usd,
        rows: vec![
            BranchRow {
                branch: Some("main".to_owned()),
                usage: usage(
                    820,
                    [180_200, 44_100_300, 940_100, 230_000, 810_400, 50_100],
                    [82_400_100, 5_100_000, 77_300_100, 0],
                    [90, 728, 2],
                    false,
                ),
            },
            BranchRow {
                branch: None,
                usage: usage(
                    64,
                    [12_100, 2_100_400, 40_200, 0, 30_100, 0],
                    [3_900_200, 0, 3_900_200, 0],
                    [0, 64, 0],
                    false,
                ),
            },
        ],
    })
}

fn unresolved_cwds() -> Value {
    json(vec![
        UnresolvedCwd {
            cwd: "/Users/ada/scratch/spike".to_owned(),
            events: 37,
            api_value_usd_micros: 2_410_300,
            last_at: at("2026-05-02T21:14:09Z"),
        },
        UnresolvedCwd {
            cwd: "/tmp/repro-1234".to_owned(),
            events: 5,
            api_value_usd_micros: 180_200,
            last_at: at("2026-04-29T10:02:00Z"),
        },
    ])
}

fn series() -> Value {
    let group = |total_tokens, api_value_micros, paid_micros| GroupValue {
        total_tokens,
        api_value_micros,
        paid_micros,
    };
    let first = usage(
        610,
        [140_200, 32_100_400, 700_100, 170_000, 620_300, 40_100],
        [61_200_400, 4_100_000, 57_100_400, 3_333_333],
        [70, 538, 2],
        false,
    );
    let second = usage(
        422,
        [98_100, 22_040_100, 480_200, 110_000, 401_200, 30_200],
        [41_800_300, 2_900_000, 38_900_300, 3_333_333],
        [52, 369, 1],
        true,
    );
    let points = vec![
        SeriesPoint {
            start: day("2026-05-02"),
            minutes: 1440,
            cumulative_api_value_micros: first.api_value_micros,
            cumulative_paid_micros: first.paid_micros,
            usage: first.clone(),
            groups: BTreeMap::from([
                (NO_GROUP.to_owned(), group(1_231_000, 2_100_400, 0)),
                (
                    "tokenmtr".to_owned(),
                    group(32_500_000, 59_100_000, 4_100_000),
                ),
            ]),
        },
        SeriesPoint {
            start: day("2026-05-03"),
            minutes: 1440,
            cumulative_api_value_micros: first.api_value_micros + second.api_value_micros,
            cumulative_paid_micros: first.paid_micros + second.paid_micros,
            usage: second,
            groups: BTreeMap::from([
                (
                    "acme-web".to_owned(),
                    group(4_129_600, 7_800_300, 2_900_000),
                ),
                ("tokenmtr".to_owned(), group(19_000_000, 34_000_000, 0)),
            ]),
        },
    ];
    json(Series {
        currency: Currency::Usd,
        bucket: Bucket::Day,
        group_by: GroupBy::Project,
        points,
    })
}

fn heatmap() -> Value {
    let mut cells = Vec::with_capacity(168);
    for weekday in 1..=7_u8 {
        for hour in 0..24_u8 {
            // Weekday office hours ramp up; every evening is quiet but not idle.
            let events: u8 = match hour {
                9..=18 if weekday <= 5 => 3 + weekday + 3 * (hour - 9),
                20..=22 => 2,
                _ => 0,
            };
            cells.push(HeatCell {
                weekday,
                hour,
                events: events.into(),
                total_tokens: u64::from(events) * 56_100,
                api_value_micros: i64::from(events) * 100_300,
            });
        }
    }
    json(Heatmap {
        currency: Currency::Usd,
        cells,
    })
}

fn tools() -> Value {
    let row = |tool: Option<&str>, server: Option<&str>, calls, errors, rate, sessions| ToolRow {
        tool_name: tool.map(str::to_owned),
        mcp_server: server.map(str::to_owned),
        calls,
        errors,
        error_rate_percent: rate,
        sessions,
    };
    json(ToolsReport {
        tools: vec![
            row(Some("Bash"), None, 812, 41, 5.05, 58),
            row(
                Some("mcp__linear__get_issue"),
                Some("linear"),
                24,
                0,
                0.0,
                6,
            ),
        ],
        mcp_servers: vec![row(None, Some("linear"), 31, 2, 6.45, 7)],
        by_model: vec![
            ToolModelCalls {
                tool_name: "Bash".to_owned(),
                mcp_server: None,
                model: Some("claude-opus-4-1".to_owned()),
                calls: 700,
                errors: 40,
            },
            ToolModelCalls {
                tool_name: "Bash".to_owned(),
                mcp_server: None,
                model: Some("claude-sonnet-4-5".to_owned()),
                calls: 112,
                errors: 1,
            },
            ToolModelCalls {
                tool_name: "mcp__linear__get_issue".to_owned(),
                mcp_server: Some("linear".to_owned()),
                model: None,
                calls: 24,
                errors: 0,
            },
        ],
    })
}

fn signals() -> Value {
    json(SignalsReport {
        days: vec![
            SignalDay {
                date: day("2026-05-02"),
                api_errors: 3,
                compactions: 1,
            },
            SignalDay {
                date: day("2026-05-03"),
                api_errors: 0,
                compactions: 2,
            },
        ],
        api_errors: 3,
        compactions: 3,
        top_sessions: vec![
            SessionCompactions {
                agent: AgentId::CLAUDE,
                session: SESSION.to_owned(),
                compactions: 2,
            },
            SessionCompactions {
                agent: AgentId::PI,
                session: PI_SESSION.to_owned(),
                compactions: 1,
            },
        ],
        models: vec![
            SignalModel {
                model: Some("claude-opus-4-1".to_owned()),
                api_errors: 2,
                compactions: 2,
            },
            SignalModel {
                model: None,
                api_errors: 1,
                compactions: 1,
            },
        ],
    })
}

fn sessions() -> Value {
    json(SessionPage {
        currency: Currency::Usd,
        total: 63,
        page: 0,
        page_size: 2,
        rows: vec![
            SessionRow {
                agent: AgentId::CLAUDE,
                session: SESSION.to_owned(),
                profile: "default".to_owned(),
                started_at: at("2026-05-03T12:01:17Z"),
                last_at: at("2026-05-03T14:09:52Z"),
                duration_secs: 7715,
                project: Some("tokenmtr".to_owned()),
                models: vec!["claude-opus-4-1".to_owned(), "claude-sonnet-4-5".to_owned()],
                tool_calls: 214,
                tool_errors: 9,
                api_errors: 1,
                usage: usage(
                    188,
                    [41_200, 10_020_400, 220_100, 51_000, 190_300, 12_100],
                    [19_400_200, 0, 19_400_200, 0],
                    [0, 188, 0],
                    false,
                ),
            },
            SessionRow {
                agent: AgentId::PI,
                session: PI_SESSION.to_owned(),
                profile: "default".to_owned(),
                started_at: at("2026-05-02T09:14:00Z"),
                last_at: at("2026-05-02T09:20:00Z"),
                duration_secs: 360,
                project: None,
                models: vec!["gpt-5".to_owned()],
                tool_calls: 12,
                tool_errors: 0,
                api_errors: 0,
                usage: usage(
                    9,
                    [3100, 210_400, 0, 0, 8100, 2200],
                    [410_300, 410_300, 0, 0],
                    [9, 0, 0],
                    false,
                ),
            },
        ],
    })
}

fn session_detail() -> Value {
    let event =
        |time, model: &str, kind, branch: Option<&str>, tokens, output, api, source| SessionEvent {
            at: at(time),
            model: model.to_owned(),
            kind,
            git_branch: branch.map(str::to_owned),
            total_tokens: tokens,
            output,
            api_value_micros: api,
            cost_source: source,
        };
    let call = |time, tool: &str, server: Option<&str>, is_error| SessionToolCall {
        at: at(time),
        tool_name: tool.to_owned(),
        mcp_server: server.map(str::to_owned),
        is_error,
    };
    json(SessionDetail {
        currency: Currency::Usd,
        events: vec![
            event(
                "2026-05-03T12:01:20Z",
                "claude-opus-4-1",
                EventKind::Message,
                Some("main"),
                48_210,
                1204,
                912_300,
                CostSource::Estimated,
            ),
            event(
                "2026-05-03T12:01:41Z",
                "claude-sonnet-4-5",
                EventKind::ToolResult,
                Some("main"),
                51_022,
                310,
                160_400,
                CostSource::Reported,
            ),
            event(
                "2026-05-03T13:30:02Z",
                "claude-sonnet-4-5",
                EventKind::Compaction,
                None,
                152_004,
                4012,
                512_000,
                CostSource::Estimated,
            ),
            event(
                "2026-05-03T13:30:05Z",
                "claude-sonnet-4-5",
                EventKind::BranchSummary,
                Some("feat/golden"),
                8100,
                800,
                31_000,
                CostSource::Estimated,
            ),
            event(
                "2026-05-03T13:31:00Z",
                "claude-haiku-4-5",
                EventKind::Auxiliary,
                Some("feat/golden"),
                2100,
                40,
                2400,
                CostSource::Estimated,
            ),
            event(
                "2026-05-03T00:00:00Z",
                "internal-preview",
                EventKind::DailyAggregate,
                None,
                12_000,
                900,
                0,
                CostSource::Unpriced,
            ),
        ],
        events_truncated: false,
        tool_calls: vec![
            call("2026-05-03T12:01:35Z", "Bash", None, Some(false)),
            call(
                "2026-05-03T12:02:10Z",
                "mcp__linear__get_issue",
                Some("linear"),
                Some(true),
            ),
            call("2026-05-03T12:03:00Z", "Read", None, None),
        ],
        signals: vec![
            (at("2026-05-03T12:40:11Z"), SignalKind::ApiError),
            (at("2026-05-03T13:30:02Z"), SignalKind::Compaction),
        ],
    })
}

/// Bucket money `[api_value, subscription_fee, extra_usage, pay_as_you_go]`; paid is the sum of
/// the last three.
fn amounts([api, fee, extra, payg]: [i64; 4]) -> BillingAmounts {
    BillingAmounts {
        api_value_micros: api,
        paid_micros: fee + extra + payg,
        subscription_fee_micros: fee,
        extra_usage_micros: extra,
        pay_as_you_go_micros: payg,
    }
}

/// Three Paris days of a $100 monthly subscription: covered usage, a day of extra usage that
/// costs more than its API value, and a future day with nothing paid.
fn billing_series() -> Value {
    let mut cumulative = [0, 0];
    let mut point = |start: &str, local: &str, money, agents: &[(&str, [i64; 4])], approx| {
        let amounts = amounts(money);
        cumulative[0] += amounts.api_value_micros;
        cumulative[1] += amounts.paid_micros;
        BillingPoint {
            start: at(start),
            local: local.parse().unwrap(),
            minutes: 1440,
            amounts,
            savings_micros: amounts.api_value_micros - amounts.paid_micros,
            roi: (amounts.paid_micros != 0).then(|| {
                let ratio = amounts.api_value_micros * 100 / amounts.paid_micros;
                f64::from(i32::try_from(ratio).unwrap()) / 100.0
            }),
            cumulative_api_value_micros: cumulative[0],
            cumulative_paid_micros: cumulative[1],
            agents: agents
                .iter()
                .map(|(agent, money)| ((*agent).to_owned(), self::amounts(*money)))
                .collect(),
            approximate: approx,
        }
    };
    let points = vec![
        point(
            "2026-05-01T22:00:00Z",
            "2026-05-02T00:00:00",
            [4_200_000, 3_225_806, 0, 300_000],
            &[
                ("claude", [3_900_000, 3_225_806, 0, 0]),
                ("pi", [300_000, 0, 0, 300_000]),
            ],
            true,
        ),
        point(
            "2026-05-02T22:00:00Z",
            "2026-05-03T00:00:00",
            [1_000_000, 3_225_806, 12_345_000, 0],
            &[("claude", [1_000_000, 3_225_806, 12_345_000, 0])],
            false,
        ),
        point(
            "2026-05-03T22:00:00Z",
            "2026-05-04T00:00:00",
            [0; 4],
            &[],
            false,
        ),
    ];
    json(BillingSeries {
        currency: Currency::Usd,
        granularity: Granularity::Daily,
        points,
    })
}

fn tokens([input_uncached, cache_read, short, long, output, reasoning]: Tokens) -> TokenCounts {
    TokenCounts {
        input_uncached,
        cache_read,
        cache_write_5m: short,
        cache_write_1h: long,
        output,
        reasoning,
    }
}

fn model_sessions(model: &str) -> ModelSessions {
    ModelSessions {
        model: model.to_owned(),
        sessions: 1,
    }
}

/// Two sessions of a Paris week; the Pi one is pay-as-you-go and has no input.
fn session_analytics() -> Value {
    let rows = vec![
        SessionPoint {
            agent: AgentId::CLAUDE,
            session: SESSION.to_owned(),
            started_at: at("2026-05-03T12:01:20Z"),
            duration_secs: 5380,
            api_value_micros: 1_618_100,
            paid_micros: 0,
            cache_hit_percent: Some(97.5),
            tokens: tokens([2_000, 195_000, 2_500, 500, 6_366, 0]),
            tool_calls: 3,
            tool_errors: 1,
        },
        SessionPoint {
            agent: AgentId::PI,
            session: PI_SESSION.to_owned(),
            started_at: at("2026-05-02T09:14:00Z"),
            duration_secs: 0,
            api_value_micros: 4_000,
            paid_micros: 4_000,
            cache_hit_percent: None,
            tokens: tokens([0, 0, 0, 0, 120, 40]),
            tool_calls: 0,
            tool_errors: 0,
        },
    ];
    let histogram = COST_EDGES_MICROS
        .iter()
        .enumerate()
        .map(|(i, &min_micros)| CostBucket {
            min_micros,
            max_micros: COST_EDGES_MICROS.get(i + 1).copied(),
            sessions: u64::from(min_micros == 0 || min_micros == 1_000_000),
            models: match min_micros {
                0 => vec![model_sessions("gpt-5")],
                1_000_000 => vec![model_sessions("claude-opus-4-1")],
                _ => Vec::new(),
            },
        })
        .collect();
    let days = [("2026-05-01", 0), ("2026-05-02", 1), ("2026-05-03", 1)]
        .into_iter()
        .map(|(d, sessions)| SessionDay {
            day: day(d),
            sessions,
        })
        .collect();
    // Saturday 11:00 and Sunday 14:00 in Paris.
    let starts = (1..=7u8)
        .flat_map(|weekday| (0..24u8).map(move |hour| (weekday, hour)))
        .map(|(weekday, hour)| SessionStartCell {
            weekday,
            hour,
            sessions: u64::from((weekday, hour) == (6, 11) || (weekday, hour) == (7, 14)),
        })
        .collect();
    json(SessionAnalytics {
        currency: Currency::Usd,
        total: 2,
        rows,
        histogram,
        days,
        starts,
    })
}

fn session_timeline() -> Value {
    let event = |time, model: &str, counts, api, paid, cumulative: [i64; 2]| TimelineEvent {
        at: at(time),
        model: model.to_owned(),
        tokens: tokens(counts),
        api_value_micros: api,
        paid_micros: paid,
        cumulative_api_value_micros: cumulative[0],
        cumulative_paid_micros: cumulative[1],
    };
    json(SessionTimeline {
        currency: Currency::Usd,
        events: vec![
            event(
                "2026-05-03T12:01:20Z",
                "claude-opus-4-1",
                [1_200, 45_000, 800, 0, 1_204, 0],
                912_300,
                0,
                [912_300, 0],
            ),
            event(
                "2026-05-03T12:01:41Z",
                "claude-sonnet-4-5",
                [400, 50_000, 300, 0, 310, 0],
                160_400,
                160_400,
                [1_072_700, 160_400],
            ),
        ],
        events_truncated: false,
        signals: vec![
            TimelineSignal {
                at: at("2026-05-03T12:40:11Z"),
                kind: SignalKind::ApiError,
            },
            TimelineSignal {
                at: at("2026-05-03T13:30:02Z"),
                kind: SignalKind::Compaction,
            },
        ],
        tools: vec![
            TimelineTool {
                tool_name: "Bash".to_owned(),
                mcp_server: None,
                calls: 2,
                failed: 0,
            },
            TimelineTool {
                tool_name: "mcp__linear__get_issue".to_owned(),
                mcp_server: Some("linear".to_owned()),
                calls: 1,
                failed: 1,
            },
        ],
        models: vec![
            SessionModel {
                model: "claude-opus-4-1".to_owned(),
                events: 1,
                tokens: tokens([1_200, 45_000, 800, 0, 1_204, 0]),
                api_value_micros: 912_300,
                approximate: false,
            },
            SessionModel {
                model: "claude-sonnet-4-5".to_owned(),
                events: 1,
                tokens: tokens([400, 50_000, 300, 0, 310, 0]),
                api_value_micros: 160_400,
                approximate: false,
            },
        ],
    })
}

fn billing() -> Value {
    let total = period_usage();
    json(BillingReport {
        currency: Currency::Usd,
        total: total.clone(),
        api: usage(
            214,
            [61_000, 9_100_000, 0, 0, 210_400, 30_300],
            [12_340_000, 12_340_000, 0, 0],
            [214, 0, 0],
            false,
        ),
        subscriptions: vec![
            SubscriptionLine {
                id: "claude-max".to_owned(),
                provider: "anthropic".to_owned(),
                plan: Plan::Max5x,
                fee_micros: 100_000_000,
                fee_currency: Currency::Usd,
                period: BillingPeriod::Monthly,
                start: day("2026-01-15"),
                end: None,
                usage: usage(
                    1618,
                    [351_300, 89_134_500, 2_104_300, 512_000, 1_663_000, 90_200],
                    [172_183_100, 0, 172_183_100, 100_000_000],
                    [0, 1610, 8],
                    true,
                ),
                roi: Some(1.72),
            },
            SubscriptionLine {
                id: "chatgpt-pro".to_owned(),
                provider: "openai".to_owned(),
                plan: Plan::Custom,
                fee_micros: 2_000_000_000,
                fee_currency: Currency::Eur,
                period: BillingPeriod::Yearly,
                start: day("2025-06-01"),
                end: Some(day("2026-05-01")),
                usage: usage(0, [0; 6], [0; 4], [0; 3], false),
                roi: None,
            },
        ],
        months: vec![BillingMonth {
            month: day("2026-05-01"),
            usage: total,
            roi: Some(1.72),
        }],
        roi: Some(1.72),
    })
}

fn quota_meters() -> Vec<QuotaMeter> {
    let window =
        |id: &str, used_percent, resets_at: Option<&str>, resets_in_secs, eta| QuotaWindowView {
            window: WindowId::new(id).unwrap(),
            label: WindowId::new(id).unwrap().label(),
            used_percent,
            resets_at: resets_at.map(at),
            resets_in_secs,
            eta,
            spend: None,
        };
    let meter =
        |profile: &str, fetched_at, age_secs, status, account: Option<&str>, windows| QuotaMeter {
            agent: AgentId::CLAUDE,
            profile: profile.parse().unwrap(),
            fetched_at: at(fetched_at),
            age_secs,
            status,
            account: account.map(str::to_owned),
            windows,
        };
    vec![
        meter(
            "default",
            "2026-05-03T14:10:00Z",
            119,
            QuotaStatus::Ok,
            Some("ada@example.com"),
            vec![
                window(
                    WindowId::FIVE_HOUR,
                    63.5,
                    Some("2026-05-03T16:00:00Z"),
                    Some(6481),
                    QuotaEta::WillHit {
                        at: at("2026-05-03T15:31:00Z"),
                    },
                ),
                window(
                    WindowId::SEVEN_DAY,
                    41.0,
                    Some("2026-05-07T09:00:00Z"),
                    Some(327_481),
                    QuotaEta::WontHit,
                ),
                window("seven_day_opus", 2.0, None, None, QuotaEta::NotEnoughData),
                window(
                    "weekly_scoped:fable",
                    74.0,
                    Some("2026-05-07T09:00:00Z"),
                    Some(327_481),
                    QuotaEta::WontHit,
                ),
                QuotaWindowView {
                    spend: Some(QuotaSpend {
                        currency: Currency::Usd,
                        spent_micros: 12_345_000,
                        limit_micros: 50_000_000,
                        approximate: false,
                    }),
                    ..window("extra_usage", 24.69, None, None, QuotaEta::NotEnoughData)
                },
            ],
        ),
        meter(
            "work",
            "2026-05-02T18:00:00Z",
            72_719,
            QuotaStatus::TokenExpired,
            None,
            Vec::new(),
        ),
        meter(
            "side",
            "2026-05-03T14:00:00Z",
            719,
            QuotaStatus::SignedOut,
            None,
            Vec::new(),
        ),
        meter(
            "lab",
            "2026-05-03T14:05:00Z",
            419,
            QuotaStatus::Failed,
            None,
            Vec::new(),
        ),
        meter(
            "locked",
            "2026-05-03T14:06:00Z",
            359,
            QuotaStatus::KeychainDenied,
            None,
            Vec::new(),
        ),
    ]
}

fn quota_latest() -> Value {
    json(quota_meters())
}

fn quota_history() -> Value {
    let point = |time, used_percent| QuotaPoint {
        at: at(time),
        used_percent,
    };
    json(vec![
        QuotaHistory {
            agent: AgentId::CLAUDE,
            profile: "default".to_owned(),
            window: WindowId::FIVE_HOUR.to_owned(),
            label: WindowId::five_hour().label(),
            points: vec![
                point("2026-05-03T11:05:00Z", 4.0),
                point("2026-05-03T12:30:00Z", 28.5),
                point("2026-05-03T14:10:00Z", 63.5),
            ],
            resets: vec![at("2026-05-03T06:00:00Z"), at("2026-05-03T11:00:00Z")],
        },
        QuotaHistory {
            agent: AgentId::CLAUDE,
            profile: "default".to_owned(),
            window: WindowId::SEVEN_DAY.to_owned(),
            label: WindowId::seven_day().label(),
            points: vec![
                point("2026-04-30T09:00:00Z", 0.0),
                point("2026-05-03T14:10:00Z", 41.0),
            ],
            resets: vec![at("2026-04-30T09:00:00Z")],
        },
    ])
}

#[expect(
    clippy::too_many_lines,
    reason = "one literal report per advisor case, read top to bottom"
)]
fn advisor() -> Value {
    let fit = |plan, fee, five_hour_peak, seven_day_peak, fits| PlanFit {
        plan,
        monthly_fee: UsdMicros::new(fee),
        five_hour_peak,
        seven_day_peak,
        fits,
    };
    // A catalog plan's current fee is its list fee.
    let recommend = |verdict, (current, fee), fits| {
        Advice::Recommend(Recommendation {
            verdict,
            current,
            sizing: QuotaSizing::Catalog,
            current_monthly_fee: Some(UsdMicros::new(fee)),
            headroom: 80,
            fits,
        })
    };
    let report = |agent, profile: &str, current_plan, snapshots, api, advice| AdvisorReport {
        agent,
        profile: profile.to_owned(),
        current_plan,
        organization: None,
        snapshots,
        monthly_api_value_usd_micros: api,
        advice,
    };
    json(vec![
        report(
            AgentId::CLAUDE,
            "default",
            Some(Plan::Max5x),
            412,
            486_250_000,
            recommend(
                Verdict::Upgrade(Plan::Max20x),
                (Plan::Max5x, 100_000_000),
                vec![
                    fit(Plan::Pro, 20_000_000, 548.0, 310.5, false),
                    fit(Plan::Max5x, 100_000_000, 109.6, 62.1, false),
                    fit(Plan::Max20x, 200_000_000, 27.4, 15.5, true),
                ],
            ),
        ),
        report(
            AgentId::CLAUDE,
            "work",
            Some(Plan::Max20x),
            380,
            61_200_000,
            recommend(
                Verdict::Downgrade(Plan::Pro),
                (Plan::Max20x, 200_000_000),
                vec![fit(Plan::Pro, 20_000_000, 72.0, 40.0, true)],
            ),
        ),
        report(
            AgentId::CLAUDE,
            "side",
            Some(Plan::Pro),
            290,
            31_500_000,
            recommend(
                Verdict::Keep,
                (Plan::Pro, 20_000_000),
                vec![fit(Plan::Pro, 20_000_000, 66.0, 51.0, true)],
            ),
        ),
        report(
            AgentId::CLAUDE,
            "lab",
            Some(Plan::Pro),
            150,
            4_200_000,
            recommend(
                Verdict::PayAsYouGo,
                (Plan::Pro, 20_000_000),
                vec![fit(Plan::Pro, 20_000_000, 9.0, 3.5, true)],
            ),
        ),
        seat_report(
            "team",
            Plan::Team,
            Advice::Recommend(Recommendation {
                verdict: Verdict::Keep,
                current: Plan::Team,
                sizing: QuotaSizing::RateTier(Plan::Max5x),
                current_monthly_fee: None,
                headroom: 80,
                fits: vec![
                    fit(Plan::Pro, 20_000_000, 350.0, 250.0, false),
                    fit(Plan::Max5x, 100_000_000, 70.0, 50.0, true),
                    fit(Plan::Max20x, 200_000_000, 17.5, 12.5, true),
                ],
            }),
        ),
        seat_report("enterprise", Plan::Enterprise, Advice::UnknownQuota),
        report(
            AgentId::CLAUDE,
            "new",
            None,
            6,
            1_800_000,
            Advice::NotEnoughData { span_days: 2 },
        ),
        report(AgentId::PI, "default", None, 0, 92_300_000, Advice::NoPlan),
    ])
}

/// A Claude seat of the `Acme` organization: Team sized by its rate tier, Enterprise unsized.
fn seat_report(profile: &str, plan: Plan, advice: Advice) -> AdvisorReport {
    AdvisorReport {
        agent: AgentId::CLAUDE,
        profile: profile.to_owned(),
        current_plan: Some(plan),
        organization: Some("Acme".to_owned()),
        snapshots: 330,
        monthly_api_value_usd_micros: 212_000_000,
        advice,
    }
}

fn budget_views() -> Vec<BudgetView> {
    let budget = |state, id: &str, period, measure, currency, start, end, money: [i64; 3], pct| {
        let [limit_micros, spent_micros, projected_micros] = money;
        let [percent, projected_percent]: [u32; 2] = pct;
        BudgetView {
            state,
            id: id.to_owned(),
            period,
            measure,
            currency,
            period_start: day(start),
            period_end: day(end),
            limit_micros,
            spent_micros,
            percent,
            projected_micros,
            projected_percent,
        }
    };
    vec![
        budget(
            BudgetState::Measured,
            "monthly-paid",
            BudgetPeriod::Month,
            BudgetMeasure::Paid,
            Currency::Usd,
            "2026-05-01",
            "2026-06-01",
            [150_000_000, 23_400_000, 241_800_000],
            [15, 161],
        ),
        budget(
            BudgetState::Measured,
            "daily-api",
            BudgetPeriod::Day,
            BudgetMeasure::ApiValue,
            Currency::Usd,
            "2026-05-03",
            "2026-05-04",
            [50_000_000, 18_250_000, 31_280_000],
            [36, 62],
        ),
        budget(
            BudgetState::NoFxRate,
            "weekly-eur",
            BudgetPeriod::Week,
            BudgetMeasure::ApiValue,
            Currency::Eur,
            "2026-04-27",
            "2026-05-04",
            [200_000_000, 0, 0],
            [0, 0],
        ),
    ]
}

fn budgets() -> Value {
    json(budget_views())
}

fn alerts() -> Value {
    let alert = |fired_at, dedup_key: &str, alert| AlertView {
        fired_at: at(fired_at),
        dedup_key: dedup_key.to_owned(),
        alert,
    };
    json(vec![
        alert(
            "2026-05-03T13:42:00Z",
            "quota_threshold:claude:default:five_hour:80:2026-05-03T16:00:00Z",
            AlertKind::QuotaThreshold {
                window: WindowId::five_hour(),
                percent: 80,
            },
        ),
        alert(
            "2026-05-03T11:00:05Z",
            "quota_reset:claude:default:five_hour:2026-05-03T11:00:00Z",
            AlertKind::QuotaReset {
                window: WindowId::five_hour(),
            },
        ),
        alert(
            "2026-05-02T19:20:00Z",
            "budget_threshold:monthly-paid:80:2026-05-01",
            AlertKind::BudgetThreshold {
                budget: "monthly-paid".parse().unwrap(),
                percent: 80,
            },
        ),
        alert(
            "2026-05-02T08:00:00Z",
            "sink_failing:warehouse",
            AlertKind::SinkFailing {
                sink: "warehouse".parse().unwrap(),
                since: at("2026-05-02T07:30:00Z"),
            },
        ),
        alert(
            "2026-05-01T09:12:44Z",
            "format_drift:claude",
            AlertKind::FormatDrift {
                agent: AgentId::CLAUDE,
            },
        ),
        alert(
            "2026-05-02T03:00:01Z",
            "backup_failed:2026-05-02",
            AlertKind::BackupFailed,
        ),
    ])
}

fn backup_views() -> Vec<BackupView> {
    vec![
        BackupView {
            started_at: at("2026-05-03T03:00:00Z"),
            finished_at: at("2026-05-03T03:00:04Z"),
            path: Some(
                "/Users/ada/Library/Application Support/TokenMtr/backups/tokenmtr-2026-05-03.db"
                    .to_owned(),
            ),
            size_bytes: Some(48_234_496),
            error: None,
        },
        BackupView {
            started_at: at("2026-05-02T03:00:00Z"),
            finished_at: at("2026-05-02T03:00:01Z"),
            path: None,
            size_bytes: None,
            error: Some("disk full: No space left on device (os error 28)".to_owned()),
        },
    ]
}

fn backups() -> Value {
    json(backup_views())
}

fn fx_rate_views() -> Vec<FxRateView> {
    let rate = |date, eur_per_usd_scaled| FxRateView {
        date: day(date),
        eur_per_usd_scaled,
        source: "ecb".to_owned(),
    };
    vec![
        rate("2026-05-01", 881_600_000),
        rate("2026-04-30", 883_200_000),
    ]
}

fn fx_rates() -> Value {
    json(fx_rate_views())
}

fn ingest_status() -> Value {
    let status = |agent, last_run_at, counts: [u64; 5]| {
        let [
            files_parsed,
            lines_read,
            events_found,
            events_changed,
            open_issues,
        ] = counts;
        IngestStatus {
            agent,
            last_run_at: at(last_run_at),
            files_parsed,
            lines_read,
            events_found,
            events_changed,
            open_issues,
        }
    };
    json(vec![
        status(
            AgentId::CLAUDE,
            "2026-05-03T14:10:00Z",
            [12, 4821, 1840, 37, 1],
        ),
        status(AgentId::PI, "2026-05-03T14:10:01Z", [3, 912, 214, 0, 0]),
    ])
}

fn parse_issues() -> Value {
    let issue = |agent, kind, subject: &str, occurrences, sample: Option<&str>, seen: [&str; 2]| {
        ParseIssueView {
            agent,
            kind,
            subject: subject.to_owned(),
            occurrences,
            sample: sample.map(str::to_owned),
            first_seen: at(seen[0]),
            last_seen: at(seen[1]),
            resolved_at: None,
        }
    };
    let mut resolved = issue(
        AgentId::CLAUDE,
        IssueKind::UnknownVersion,
        "2.9.0",
        1,
        None,
        ["2026-05-02T10:00:00Z", "2026-05-02T10:00:00Z"],
    );
    resolved.resolved_at = Some(at("2026-05-02T18:30:00Z"));
    json(vec![
        issue(
            AgentId::CLAUDE,
            IssueKind::FormatDrift,
            "/Users/ada/.claude/projects/-Users-ada-src-tokenmtr/0b7c1d2e.jsonl",
            4,
            Some(r#"{"type":"assistant","message":{"usage":{"input_tokens":"12"}}}"#),
            ["2026-05-01T09:12:44Z", "2026-05-03T14:10:00Z"],
        ),
        resolved,
        issue(
            AgentId::CLAUDE,
            IssueKind::ProtectedFolder,
            "/Users/ada/Documents/client-work",
            12,
            None,
            ["2026-04-28T08:00:00Z", "2026-05-03T14:10:00Z"],
        ),
        issue(
            AgentId::PI,
            IssueKind::MissingCwd,
            "/Users/ada/.pi/agent/sessions/2026-05-02T09-14-00.jsonl",
            2,
            None,
            ["2026-05-02T09:14:00Z", "2026-05-02T09:20:00Z"],
        ),
        issue(
            AgentId::PI,
            IssueKind::ParseError,
            "/Users/ada/.pi/agent/sessions/2026-05-01T17-02-11.jsonl",
            1,
            Some(r#"{"type":"message","#),
            ["2026-05-01T17:02:11Z", "2026-05-01T17:02:11Z"],
        ),
        issue(
            AgentId::PI,
            IssueKind::IntegrationDrift,
            "superset",
            1,
            Some("no such column: w.worktree_path"),
            ["2026-05-03T14:10:00Z", "2026-05-03T14:10:00Z"],
        ),
    ])
}

fn daemon_health() -> DaemonHealth {
    DaemonHealth {
        state: DaemonState::Running,
        last_heartbeat: Some(at("2026-05-03T14:11:30Z")),
        age_secs: Some(29),
    }
}

fn sinks() -> Vec<SinkHealth> {
    let run = |dataset, started_at, finished_at, rows, error: Option<&str>| SinkRunView {
        dataset,
        started_at: at(started_at),
        finished_at: at(finished_at),
        rows,
        error: error.map(str::to_owned),
    };
    let refused = "connection refused (os error 61)";
    let sink = |id: &str, state, lag_rows, lag_secs, last_success_at: Option<&str>, recent_runs| {
        SinkHealth {
            sink: id.parse().unwrap(),
            state,
            lag_rows,
            lag_secs,
            last_success_at: last_success_at.map(at),
            recent_runs,
        }
    };
    vec![
        sink(
            "local-parquet",
            SinkState::Healthy,
            0,
            None,
            Some("2026-05-03T14:10:05Z"),
            vec![
                run(
                    Dataset::UsageEvents,
                    "2026-05-03T14:10:02Z",
                    "2026-05-03T14:10:05Z",
                    37,
                    None,
                ),
                run(
                    Dataset::ToolCalls,
                    "2026-05-03T14:10:02Z",
                    "2026-05-03T14:10:04Z",
                    112,
                    None,
                ),
            ],
        ),
        sink(
            "warehouse",
            SinkState::Failing {
                since: at("2026-05-02T07:30:00Z"),
                last_error: refused.to_owned(),
            },
            5120,
            Some(110_400),
            Some("2026-05-02T07:15:00Z"),
            vec![run(
                Dataset::Subscriptions,
                "2026-05-03T14:10:02Z",
                "2026-05-03T14:10:03Z",
                0,
                Some(refused),
            )],
        ),
        sink(
            "s3-archive",
            SinkState::Lagging,
            840,
            Some(3600),
            Some("2026-05-03T13:10:00Z"),
            vec![
                run(
                    Dataset::QuotaSnapshots,
                    "2026-05-03T13:09:58Z",
                    "2026-05-03T13:10:00Z",
                    24,
                    None,
                ),
                run(
                    Dataset::FxRates,
                    "2026-05-03T13:09:58Z",
                    "2026-05-03T13:09:59Z",
                    1,
                    None,
                ),
            ],
        ),
        sink("legacy-csv", SinkState::Disabled, 0, None, None, Vec::new()),
    ]
}

fn system_health() -> Value {
    json(SystemHealth {
        level: HealthLevel::Warning,
        daemon: daemon_health(),
        sinks: sinks(),
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "one response document, three tabs written out"
)]
fn recap() -> Value {
    let today = DateRange::day(day("2026-05-03")).unwrap();
    let sparkline = |scale: i64| {
        let days = [
            "2026-04-27",
            "2026-04-28",
            "2026-04-29",
            "2026-04-30",
            "2026-05-01",
            "2026-05-02",
            "2026-05-03",
        ];
        let values = [270, 280, 290, 300, 40, 80, 120];
        days.iter()
            .zip(values)
            .map(|(date, value)| SparkPoint {
                date: day(date),
                api_value_micros: value * scale,
            })
            .collect()
    };
    let live = Live {
        tokens_per_minute: 184_200,
        api_value_per_hour_usd_micros: 8_120_000,
        active_sessions: 2,
    };
    let tab =
        |agent, totals: Totals, projected_paid_micros, top_model: &str, last_activity| AgentRecap {
            agent,
            period: Period::Today,
            range: today,
            currency: Currency::Usd,
            currency_fallback: false,
            totals,
            projected_paid_micros,
            quota: Vec::new(),
            budgets: Vec::new(),
            sparkline: Vec::new(),
            live: None,
            top_model: Some(top_model.to_owned()),
            last_activity: Some(at(last_activity)),
        };
    let totals = |usage, cache_savings_micros, roi| Totals {
        currency: Currency::Usd,
        currency_fallback: false,
        usage,
        cached_input_percent: 97.6,
        cache_savings_micros,
        roi,
        sessions: 7,
    };
    let budgets: Vec<BudgetView> = budget_views().into_iter().take(2).collect();
    let quota: Vec<QuotaMeter> = quota_meters().into_iter().take(1).collect();
    let all = AgentRecap {
        quota: quota.clone(),
        budgets: budgets.clone(),
        sparkline: sparkline(110_000),
        live: Some(live),
        ..tab(
            None,
            totals(
                usage(
                    422,
                    [91_520, 22_048_000, 457_600, 116_480, 416_000, 24_960],
                    [41_184_000, 2_900_000, 38_284_000, 3_333_333],
                    [0, 422, 0],
                    false,
                ),
                Some(59_488_000),
                Some(5.4),
            ),
            6_400_000,
            "claude-sonnet-4-5",
            "2026-05-03T14:09:52Z",
        )
    };
    let claude = AgentRecap {
        quota,
        budgets,
        sparkline: sparkline(100_000),
        live: Some(live),
        ..tab(
            Some(AgentId::CLAUDE),
            totals(
                usage(
                    401,
                    [86_900, 20_935_000, 434_500, 110_600, 395_000, 23_700],
                    [39_105_000, 0, 39_105_000, 3_333_333],
                    [0, 401, 0],
                    false,
                ),
                Some(56_485_000),
                Some(5.4),
            ),
            0,
            "claude-sonnet-4-5",
            "2026-05-03T14:09:52Z",
        )
    };
    let pi = AgentRecap {
        sparkline: sparkline(10_000),
        ..tab(
            Some(AgentId::PI),
            totals(
                usage(
                    21,
                    [4620, 1_113_000, 23_100, 5880, 21_000, 1260],
                    [2_079_000, 2_900_000, -821_000, 0],
                    [0, 21, 0],
                    false,
                ),
                None,
                None,
            ),
            6_400_000,
            "gpt-5",
            "2026-05-03T10:40:00Z",
        )
    };
    let mut system_sinks = sinks();
    system_sinks.truncate(2);
    let status = Status {
        status_schema: STATUS_SCHEMA,
        generated_at: at("2026-05-03T14:11:59Z"),
        system: SystemHealth {
            level: HealthLevel::Error,
            daemon: daemon_health(),
            sinks: system_sinks,
        },
        agents: vec![claude, pi],
    };
    recap_document(status, Some(all))
}

fn agent_infos(onboarding: bool) -> Vec<AgentInfo> {
    let profile =
        |root: &str, account: Option<&str>, retention_days, keeps_ten_years| ProfileInfo {
            profile: "default".to_owned(),
            root: root.to_owned(),
            account: account.map(str::to_owned),
            retention_days,
            keeps_ten_years,
        };
    let mut claude_profiles = vec![profile(
        "/Users/ada/.claude",
        Some("ada@example.com"),
        Some(30),
        false,
    )];
    if onboarding {
        claude_profiles.push(ProfileInfo {
            profile: "work".to_owned(),
            ..profile("/Users/ada/.claude-work", None, None, true)
        });
    }
    vec![
        AgentInfo {
            agent: "claude".to_owned(),
            label: "Claude Code".to_owned(),
            followed: true,
            capabilities: Capabilities {
                quota: true,
                reported_cost: false,
                subscription: true,
            },
            profiles: claude_profiles,
        },
        AgentInfo {
            agent: "pi".to_owned(),
            label: "pi".to_owned(),
            followed: !onboarding,
            capabilities: Capabilities {
                quota: false,
                reported_cost: true,
                subscription: true,
            },
            profiles: vec![profile("/Users/ada/.pi/agent", None, None, !onboarding)],
        },
    ]
}

fn app_state() -> Value {
    json(AppState::Onboarding {
        agents: agent_infos(true),
        daemon_installed: false,
        notifications: Permission::Undetermined,
    })
}

fn app_state_ready() -> Value {
    json(AppState::Ready {
        last_scan_at: Some(at("2026-05-03T14:10:00Z")),
        scan_interval_secs: 300,
        failed_collectors: 1,
    })
}

fn app_state_daemon_offline() -> Value {
    json(AppState::DaemonOffline {})
}

fn app_state_incompatible() -> Value {
    json(AppState::Incompatible {
        app_version: "0.4.0".to_owned(),
        daemon_version: "0.3.2".to_owned(),
        app_schema: 5,
        daemon_schema: 4,
    })
}

fn filter_options() -> Value {
    json(FilterOptions {
        agents: strings(&["claude", "pi"]),
        profiles: strings(&["claude/default", "claude/work", "pi/default"]),
        models: strings(&["claude-opus-4-1", "claude-sonnet-4-5", "gpt-5"]),
        projects: strings(&["tokenmtr", "acme-web"]),
        clients: strings(&["Acme Corp"]),
    })
}

fn settings() -> Value {
    let support = "/Users/ada/Library/Application Support/TokenMtr";
    json(Settings {
        config_path: format!("{support}/config.toml"),
        exports_dir: format!("{support}/exports"),
        machine: "ada-mbp".to_owned(),
        agents: agent_infos(false),
        daemon: DaemonBlock {
            health: daemon_health(),
            version: "0.4.0".to_owned(),
        },
        display: DisplayBlock {
            currency: Currency::Eur,
            time_zone: "Europe/Paris".to_owned(),
        },
        fx: FxBlock {
            fixed_eur_per_usd: Some(0.92),
            latest: fx_rate_views().into_iter().next(),
            offline: false,
        },
        pricing: PricingBlock {
            snapshot_date: "2026-04-15".to_owned(),
            models: 214,
        },
        tray: TrayBlock {
            rings: vec![MenubarRing::Session, MenubarRing::Weekly],
            hidden_profiles: vec!["work".to_owned()],
            shortcut: "CmdOrCtrl+Shift+T".to_owned(),
        },
        git: GitBlock {
            default_remote: "origin".to_owned(),
            default_branch: Some("main".to_owned()),
        },
        alerts: AlertsBlock {
            quota_thresholds: vec![80, 95],
            budget_thresholds: vec![50, 80, 100],
            quota_reset: true,
        },
        launch_at_login: true,
        protected_folders: vec![ProtectedFolder {
            agent: "claude".to_owned(),
            path: "/Users/ada/Documents/client-work".to_owned(),
            accessible: false,
        }],
        backups: backup_views().into_iter().take(1).collect(),
    })
}

const TRANSCRIPTS: &str = "/Users/ada/.claude/projects/-Users-ada-src-tokenmtr";
const REPORT_FILE: &str = "tokenmtr-tokenmtr-2026-05-01-2026-05-30.csv";

fn session_origin() -> Value {
    json(SessionOrigin {
        cwd: Some("/Users/ada/src/tokenmtr".to_owned()),
        session_dir: Some(TRANSCRIPTS.to_owned()),
        transcript_path: Some(format!("{TRANSCRIPTS}/{SESSION}.jsonl")),
        transcript_exists: true,
    })
}

fn generate_report() -> Value {
    json(GeneratedReport {
        format: FormatWire::Csv,
        filename: REPORT_FILE.to_owned(),
        content: "date,agent,model,events,total_tokens,api_value_usd\n2026-05-01,claude,claude-sonnet-4-5,42,1834500,3.412000\n".to_owned(),
    })
}

fn project_rule_for() -> Value {
    json(tokenmtr_config::RawProject {
        pattern: "/Users/ada/.superset/worktrees/tokenmtr/quiet-otter".to_owned(),
        name: tokenmtr_core::ProjectKey::try_from("tokenmtr".to_owned()).unwrap(),
        client: None,
    })
}

fn save_report() -> Value {
    json(Some(format!("/Users/ada/Documents/TokenMtr/{REPORT_FILE}")))
}

/// Every fixture: file stem and content.
fn goldens() -> Vec<(&'static str, Value)> {
    vec![
        ("app_state", app_state()),
        ("app_state.ready", app_state_ready()),
        ("app_state.daemon_offline", app_state_daemon_offline()),
        ("app_state.incompatible", app_state_incompatible()),
        ("filter_options", filter_options()),
        ("settings", settings()),
        ("session_origin", session_origin()),
        ("generate_report", generate_report()),
        ("save_report", save_report()),
        ("project_rule_for", project_rule_for()),
        ("billing", billing()),
        ("quota_latest", quota_latest()),
        ("quota_history", quota_history()),
        ("advisor", advisor()),
        ("budgets", budgets()),
        ("alerts", alerts()),
        ("backups", backups()),
        ("fx_rates", fx_rates()),
        ("ingest_status", ingest_status()),
        ("parse_issues", parse_issues()),
        ("system_health", system_health()),
        ("recap", recap()),
        ("overview", overview()),
        ("series", series()),
        ("models", models()),
        ("projects", projects()),
        ("project_models", project_models()),
        ("project_remotes", project_remotes()),
        ("branches", branches()),
        ("unresolved_cwds", unresolved_cwds()),
        ("heatmap", heatmap()),
        ("tools", tools()),
        ("signals", signals()),
        ("sessions", sessions()),
        ("session_detail", session_detail()),
        ("billing_series", billing_series()),
        ("session_analytics", session_analytics()),
        ("session_timeline", session_timeline()),
    ]
}

/// Rewrites the fixtures with `UPDATE_GOLDENS=1` (`just goldens`); otherwise fails when one is
/// stale, missing, or not produced here.
#[test]
fn ui_fixtures_are_the_serialized_responses() {
    let update = std::env::var_os("UPDATE_GOLDENS").is_some();
    let dir = dir();
    let goldens = goldens();
    let mut stale = Vec::new();
    for (name, value) in &goldens {
        let wanted = serde_json::to_string_pretty(value).unwrap() + "\n";
        let path = dir.join(format!("{name}.json"));
        if update {
            std::fs::write(&path, &wanted).unwrap();
        } else if std::fs::read_to_string(&path).ok().as_deref() != Some(wanted.as_str()) {
            stale.push(*name);
        }
    }
    let produced: BTreeSet<String> = goldens
        .iter()
        .map(|(name, _)| format!("{name}.json"))
        .collect();
    let extra: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !produced.contains(name))
        .collect();
    assert!(
        stale.is_empty(),
        "stale UI fixtures {stale:?}; run `just goldens`"
    );
    assert!(
        extra.is_empty(),
        "UI fixtures not produced by Rust: {extra:?}"
    );
}
