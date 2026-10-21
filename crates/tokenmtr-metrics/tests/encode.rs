//! The encoders: both text formats (goldens, validity, negotiation, gzip) and remote write
//! (round trip, batching, zero samples, `Seen`).
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "clippy's allow-*-in-tests covers #[test] functions, not the shared helpers"
)]

use std::io::Read as _;

use flate2::read::GzDecoder;
use jiff::{SignedDuration, Timestamp};
use prost::Message as _;
use tokenmtr_metrics::text::{Format, Rendered, encode};
use tokenmtr_metrics::{
    Family, Folded, MAX_SERIES_PER_REQUEST, Seen, Series, Value, WriteRequest, requests,
};

const WINDOW: SignedDuration = SignedDuration::from_hours(48);
const NOW: i64 = 1_772_323_200;

fn at(seconds: i64) -> Timestamp {
    Timestamp::from_second(seconds).unwrap()
}

fn labelled(family: Family, values: &[&str]) -> Vec<(&'static str, String)> {
    assert_eq!(family.labels().len(), values.len(), "{}", family.name());
    family
        .labels()
        .iter()
        .copied()
        .zip(values.iter().map(|v| (*v).to_owned()))
        .collect()
}

fn count(family: Family, values: &[&str], n: u64, created: Timestamp) -> Series {
    Series {
        family,
        labels: labelled(family, values),
        value: Value::Count(n),
        created: Some(created),
    }
}

fn gauge(family: Family, values: &[&str], g: f64) -> Series {
    Series {
        family,
        labels: labelled(family, values),
        value: Value::Gauge(g),
        created: None,
    }
}

fn fold(mut series: Vec<Series>) -> Folded {
    let key = |s: &Series| {
        (
            s.family,
            s.labels.iter().map(|l| l.1.clone()).collect::<Vec<_>>(),
        )
    };
    series.sort_by_key(key);
    Folded {
        taken_at: at(NOW),
        series,
    }
}

/// A token counter by account, agent, billing, client, model, profile, project, provider,
/// service tier, sidechain, type.
fn tokens(project: &str, kind: &str, n: u64, created: Timestamp) -> Series {
    count(
        Family::Tokens,
        &[
            "me@example.com",
            "claude",
            "subscription",
            "cli",
            "opus",
            "default",
            project,
            "anthropic",
            "",
            "false",
            kind,
        ],
        n,
        created,
    )
}

/// Counters (one with every character that needs escaping) and gauges.
fn sample() -> Folded {
    fold(vec![
        tokens("github.com/acme/app", "output", 1500, at(1_772_000_000)),
        tokens(
            "we\"ird\\path\nx",
            "input_uncached",
            7,
            Timestamp::from_millisecond(1_772_000_123_456).unwrap(),
        ),
        gauge(
            Family::QuotaUsedRatio,
            &["me@example.com", "claude", "default", "5h"],
            0.42,
        ),
        gauge(Family::Series, &[], 5.0),
        gauge(Family::SnapshotTimestamp, &[], 1_772_323_200.0),
        gauge(Family::BuildInfo, &["0.3.0"], 1.0),
    ])
}

#[test]
fn plain_text_golden() {
    insta::assert_snapshot!(encode(&sample(), Format::Plain));
}

#[test]
fn openmetrics_golden() {
    insta::assert_snapshot!(encode(&sample(), Format::OpenMetrics));
}

#[test]
fn an_empty_fold_is_an_empty_body_or_just_eof() {
    let empty = fold(Vec::new());
    assert_eq!(encode(&empty, Format::Plain), "");
    assert_eq!(encode(&empty, Format::OpenMetrics), "# EOF\n");
}

#[test]
fn non_finite_gauges_use_the_exposition_spelling() {
    let folded = fold(vec![
        gauge(Family::QuotaUsedRatio, &["", "a", "p", "w1"], f64::NAN),
        gauge(Family::QuotaUsedRatio, &["", "a", "p", "w2"], f64::INFINITY),
        gauge(
            Family::QuotaUsedRatio,
            &["", "a", "p", "w3"],
            f64::NEG_INFINITY,
        ),
    ]);
    let text = encode(&folded, Format::Plain);
    assert!(
        text.contains("w1\"} NaN\n")
            && text.contains("w2\"} +Inf\n")
            && text.contains("w3\"} -Inf\n")
    );
}

/// One series per family, every label value full of characters that need escaping.
fn nasty() -> Folded {
    fold(
        Family::ALL
            .iter()
            .map(|&family| {
                let values = vec!["v \"q\" \\ \n {x=y}, é"; family.labels().len()];
                match family.kind() {
                    tokenmtr_metrics::Kind::Counter => count(family, &values, 3, at(1_772_000_000)),
                    tokenmtr_metrics::Kind::Gauge => gauge(family, &values, 0.5),
                }
            })
            .collect(),
    )
}

fn valid_metric_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == ':')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
}

fn valid_label_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with("__")
}

/// Splits `name{k="v",…} value` and checks escaping; returns the name.
fn check_sample(line: &str) -> String {
    let (head, value) = line.rsplit_once(' ').unwrap();
    assert!(
        value.parse::<f64>().is_ok() || matches!(value, "NaN" | "+Inf" | "-Inf"),
        "{line}"
    );
    let Some((name, labels)) = head.split_once('{') else {
        assert!(valid_metric_name(head), "{line}");
        return head.to_owned();
    };
    assert!(valid_metric_name(name), "{line}");
    let mut rest = labels
        .strip_suffix('}')
        .unwrap_or_else(|| panic!("no closing brace: {line}"));
    while !rest.is_empty() {
        let (key, after) = rest
            .split_once("=\"")
            .unwrap_or_else(|| panic!("bad label: {line}"));
        assert!(valid_label_name(key), "{line}");
        let mut chars = after.char_indices();
        let end = loop {
            match chars.next() {
                Some((_, '\\')) => assert!(
                    matches!(chars.next(), Some((_, '\\' | '"' | 'n'))),
                    "{line}"
                ),
                Some((n, '"')) => break n,
                Some(_) => {}
                None => panic!("unterminated value: {line}"),
            }
        };
        rest = after[end + 1..]
            .strip_prefix(',')
            .unwrap_or(&after[end + 1..]);
    }
    name.to_owned()
}

fn check_body(text: &str, format: Format) {
    let mut declared: Vec<(String, String)> = Vec::new();
    let mut helped: Vec<String> = Vec::new();
    for line in text.lines() {
        if line == "# EOF" {
            continue;
        }
        if let Some(help) = line.strip_prefix("# HELP ") {
            let (name, _) = help.split_once(' ').unwrap();
            assert!(!helped.contains(&name.to_owned()), "two HELP for {name}");
            helped.push(name.to_owned());
        } else if let Some(kind) = line.strip_prefix("# TYPE ") {
            let (name, kind) = kind.split_once(' ').unwrap();
            assert!(valid_metric_name(name), "{name}");
            assert!(matches!(kind, "counter" | "gauge"));
            assert!(
                !declared.iter().any(|(n, _)| n == name),
                "two TYPE for {name}"
            );
            declared.push((name.to_owned(), kind.to_owned()));
        } else {
            let name = check_sample(line);
            let (family, kind) = declared.last().unwrap();
            let allowed = match (format, kind.as_str()) {
                (Format::OpenMetrics, "counter") => {
                    vec![format!("{family}_total"), format!("{family}_created")]
                }
                _ => vec![family.clone()],
            };
            assert!(allowed.contains(&name), "{name} under {family}");
        }
    }
    assert_eq!(declared.len(), Family::ALL.len());
    assert_eq!(helped.len(), Family::ALL.len());
    assert_eq!(format == Format::OpenMetrics, text.ends_with("# EOF\n"));
}

#[test]
fn both_formats_are_valid_whatever_the_label_values() {
    for format in [Format::Plain, Format::OpenMetrics] {
        check_body(&encode(&nasty(), format), format);
    }
}

#[test]
fn counters_are_declared_without_total_only_in_openmetrics() {
    let plain = encode(&sample(), Format::Plain);
    assert!(plain.contains("# TYPE tokenmtr_tokens_total counter\n"));
    assert!(!plain.contains("_created"));
    let om = encode(&sample(), Format::OpenMetrics);
    assert!(om.contains("# TYPE tokenmtr_tokens counter\n"));
    assert!(om.contains("tokenmtr_tokens_created{"));
    assert!(om.contains(" 1772000123.456\n"));
}

#[test]
fn accept_negotiates_openmetrics_only_when_it_is_asked_for() {
    for (accept, expected) in [
        (None, Format::Plain),
        (Some("*/*"), Format::Plain),
        (Some("text/plain"), Format::Plain),
        (Some("application/openmetrics-text"), Format::OpenMetrics),
        (
            Some(
                "application/openmetrics-text;version=1.0.0,text/plain;version=0.0.4;q=0.5,*/*;q=0.1",
            ),
            Format::OpenMetrics,
        ),
        (
            Some("text/plain, application/openmetrics-text;q=0.5"),
            Format::Plain,
        ),
        (Some("application/openmetrics-text;q=0"), Format::Plain),
        (Some("application/openmetrics-text;q=abc"), Format::Plain),
        (Some("APPLICATION/OpenMetrics-Text"), Format::OpenMetrics),
    ] {
        assert_eq!(Format::negotiate(accept), expected, "{accept:?}");
    }
}

#[test]
fn rendered_bodies_gunzip_to_the_text() {
    let rendered = Rendered::new(&sample());
    for format in [Format::Plain, Format::OpenMetrics] {
        let body = rendered.body(format);
        assert_eq!(
            String::from_utf8(body.identity.clone()).unwrap(),
            encode(&sample(), format)
        );
        let mut text = Vec::new();
        GzDecoder::new(body.gzip.as_slice())
            .read_to_end(&mut text)
            .unwrap();
        assert_eq!(text, body.identity);
    }
    let empty = Rendered::default();
    assert_eq!(empty.plain.identity, Vec::<u8>::new());
    assert_eq!(empty.openmetrics.identity, b"# EOF\n");
}

fn decode(request: &tokenmtr_metrics::Request) -> WriteRequest {
    let raw = snap::raw::Decoder::new()
        .decompress_vec(&request.encode().unwrap())
        .unwrap();
    WriteRequest::decode(raw.as_slice()).unwrap()
}

#[test]
fn a_request_round_trips_through_snappy_and_protobuf() {
    let external = vec![("machine".to_owned(), "laptop".to_owned())];
    let sent = requests(&sample(), &Seen::default(), &external, at(NOW), WINDOW);
    assert_eq!(sent.len(), 1);
    let decoded = decode(&sent[0]);
    assert_eq!(&decoded, sent[0].payload());
    assert_eq!(decoded.timeseries.len(), 6);
    for series in &decoded.timeseries {
        let names: Vec<_> = series.labels.iter().map(|l| l.name.as_str()).collect();
        assert!(names.is_sorted(), "{names:?}");
        assert!(names.contains(&"__name__") && names.contains(&"machine"));
        let last = series.samples.last().unwrap();
        assert_eq!(last.timestamp, NOW * 1000);
    }
    let first = &decoded.timeseries[0];
    assert_eq!(
        first
            .labels
            .iter()
            .find(|l| l.name == "__name__")
            .unwrap()
            .value,
        "tokenmtr_build_info"
    );
}

#[test]
fn requests_hold_at_most_2000_series() {
    let many: Vec<_> = (0..=2 * MAX_SERIES_PER_REQUEST)
        .map(|n| tokens(&format!("p{n}"), "output", 1, at(NOW)))
        .collect();
    assert_eq!(many.len(), 4001);
    let sent = requests(&fold(many), &Seen::default(), &[], at(NOW), WINDOW);
    assert_eq!(
        sent.iter()
            .map(tokenmtr_metrics::Request::series)
            .collect::<Vec<_>>(),
        [2000, 2000, 1]
    );
    assert_eq!(
        requests(&fold(Vec::new()), &Seen::default(), &[], at(NOW), WINDOW).len(),
        0
    );
}

fn samples_of(sent: &[tokenmtr_metrics::Request], project: &str) -> Vec<(f64, i64)> {
    sent.iter()
        .flat_map(|r| &r.payload().timeseries)
        .find(|t| t.labels.iter().any(|l| l.value == project))
        .unwrap()
        .samples
        .iter()
        .map(|s| (s.value, s.timestamp))
        .collect()
}

#[test]
fn a_new_recent_counter_gets_one_zero_sample_once_the_receiver_has_been_primed() {
    let now = at(NOW);
    let mut seen = Seen::default();
    // First push: nothing is "new", so no zero samples (the receiver may know every series).
    let first = requests(
        &fold(vec![tokens("old", "output", 5, at(NOW - 10 * 86_400))]),
        &seen,
        &[],
        now,
        WINDOW,
    );
    assert_eq!(samples_of(&first, "old"), [(5.0, NOW * 1000)]);
    seen.accept(&first[0], now, WINDOW);
    assert!(
        seen.is_empty(),
        "a series older than the window is not remembered"
    );

    let folded = fold(vec![
        tokens("old", "output", 6, at(NOW - 10 * 86_400)),
        tokens("fresh", "output", 2, at(NOW - 3600)),
        gauge(Family::Series, &[], 3.0),
    ]);
    let second = requests(&folded, &seen, &[], now, WINDOW);
    assert_eq!(
        samples_of(&second, "old"),
        [(6.0, NOW * 1000)],
        "old series: no zero"
    );
    assert_eq!(
        samples_of(&second, "fresh"),
        [(0.0, NOW * 1000 - 1), (2.0, NOW * 1000)]
    );
    let gauges = second[0]
        .payload()
        .timeseries
        .iter()
        .filter(|t| t.samples.len() == 1)
        .count();
    assert_eq!(gauges, 2, "gauges and old counters never get a zero");

    seen.accept(&second[0], now, WINDOW);
    assert_eq!(seen.len(), 1);
    let third = requests(&folded, &seen, &[], at(NOW + 60), WINDOW);
    assert_eq!(
        samples_of(&third, "fresh"),
        [(2.0, (NOW + 60) * 1000)],
        "seen: no second zero"
    );
}

#[test]
fn seen_forgets_series_that_age_out_of_the_window() {
    let mut seen = Seen::default();
    let folded = fold(vec![tokens("p", "output", 1, at(NOW - 3600))]);
    let sent = requests(&folded, &seen, &[], at(NOW), WINDOW);
    seen.accept(&sent[0], at(NOW), WINDOW);
    assert_eq!(seen.len(), 1);
    seen.accept(&sent[0], at(NOW + 3 * 86_400), WINDOW);
    assert!(seen.is_empty());
}

#[test]
fn a_rejected_push_leaves_seen_alone() {
    let seen = Seen::default();
    let folded = fold(vec![tokens("p", "output", 1, at(NOW))]);
    let _ = requests(&folded, &seen, &[], at(NOW), WINDOW);
    assert_eq!(seen, Seen::default(), "only `accept` records");
}
