//! `EcbFx` parsing and fetching against a mock ECB server.

mod common;

use common::{TestResult, endpoints, fixture};
use jiff::Span;
use jiff::civil::{Date, Weekday, date};
use rust_decimal::Decimal;
use tokenmtr_core::{EurPerUsd, FxTable};
use tokenmtr_remote::{
    EcbFx, EcbParseError, FxError, FxFetch, RemoteError, Validators, build_client, parse_ecb_xml,
};
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const DAILY: &str = "/stats/eurofxref/eurofxref-daily.xml";
const HIST: &str = "/stats/eurofxref/eurofxref-hist.xml";

fn client(server: &MockServer) -> Result<EcbFx, Box<dyn std::error::Error>> {
    Ok(EcbFx::new(build_client()?, endpoints(server)?.ecb))
}

fn usd_per_eur(quote: &str) -> Result<EurPerUsd, Box<dyn std::error::Error>> {
    Ok(EurPerUsd::from_usd_per_eur(quote.parse::<Decimal>()?)?)
}

#[tokio::test]
async fn daily_parses_usd_only_and_inverts_the_quote() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(DAILY))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"daily-1\"")
                .insert_header("last-modified", "Fri, 15 May 2026 14:00:00 GMT")
                .set_body_bytes(fixture("ecb_daily.xml")?),
        )
        .mount(&server)
        .await;

    let fetch = client(&server)?.fetch_daily(&Validators::default()).await?;

    let FxFetch::New { rates, validators } = fetch else {
        return Err("expected New".into());
    };
    assert_eq!(rates.len(), 1);
    assert_eq!(rates[0].date, date(2026, 5, 15));
    assert_eq!(rates[0].eur_per_usd, usd_per_eur("1.1620")?);
    assert_eq!(validators.etag.as_deref(), Some("\"daily-1\""));
    assert_eq!(
        validators.last_modified.as_deref(),
        Some("Fri, 15 May 2026 14:00:00 GMT")
    );
    Ok(())
}

#[tokio::test]
async fn daily_replays_validators_and_honours_304() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(DAILY))
        .and(header("if-none-match", "\"daily-1\""))
        .and(header_exists("if-modified-since"))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&server)
        .await;
    let validators = Validators {
        etag: Some("\"daily-1\"".to_owned()),
        last_modified: Some("Fri, 15 May 2026 14:00:00 GMT".to_owned()),
    };

    let fetch = client(&server)?.fetch_daily(&validators).await?;

    assert!(matches!(fetch, FxFetch::NotModified));
    Ok(())
}

#[tokio::test]
async fn history_has_every_business_day_and_no_weekend() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(HIST))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(fixture("ecb_hist.xml")?))
        .mount(&server)
        .await;

    let rates = client(&server)?.fetch_history().await?;

    let dates: Vec<Date> = rates.iter().map(|r| r.date).collect();
    let (first, last) = (date(2026, 5, 4), date(2026, 5, 15));
    let mut day = first;
    while day <= last {
        let is_weekend = matches!(day.weekday(), Weekday::Saturday | Weekday::Sunday);
        assert_eq!(dates.contains(&day), !is_weekend, "{day}");
        day = day.checked_add(Span::new().days(1))?;
    }
    assert_eq!(dates.len(), 10);
    Ok(())
}

#[tokio::test]
async fn weekend_gap_resolves_to_friday_through_the_fx_table() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(HIST))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(fixture("ecb_hist.xml")?))
        .mount(&server)
        .await;
    let table = FxTable::new(client(&server)?.fetch_history().await?);

    // Monday 11 May uses the rate strictly before it: Friday 8 May (no weekend rate exists).
    let selected = table.select(date(2026, 5, 11)).ok_or("no rate")?;

    assert_eq!(selected.date, date(2026, 5, 8));
    assert_eq!(selected.rate, usd_per_eur("1.1500")?);
    Ok(())
}

#[tokio::test]
async fn http_errors_are_typed() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(HIST))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let result = client(&server)?.fetch_history().await;

    assert!(matches!(
        result,
        Err(FxError::Remote(RemoteError::Status(404)))
    ));
    Ok(())
}

#[tokio::test]
async fn unusable_body_is_a_parse_error() -> TestResult {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(DAILY))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<html><body>maintenance</body></html>"),
        )
        .mount(&server)
        .await;

    let result = client(&server)?.fetch_daily(&Validators::default()).await;

    assert!(matches!(
        result,
        Err(FxError::Parse(EcbParseError::NoUsdRates))
    ));
    Ok(())
}

#[test]
fn parser_is_tolerant_of_unusable_entries() -> TestResult {
    let xml = br"<Envelope><Cube><Cube time='2026-05-15'>
        <Cube currency='USD' rate='N/A'/>
        </Cube><Cube time='not-a-date'><Cube currency='USD' rate='1.1'/></Cube>
        <Cube time='2026-05-14'><Cube currency='USD' rate='0'/><Cube currency='USD' rate='1.25'/></Cube>
        <Cube currency='USD' rate='1.5'/>
        <Unknown/></Cube></Envelope>";

    let rates = parse_ecb_xml(xml)?;

    // Only the valid 14 May entry survives; the one under the bad date has no date.
    assert_eq!(rates.len(), 1);
    assert_eq!(rates[0].date, date(2026, 5, 14));
    assert_eq!(rates[0].eur_per_usd, usd_per_eur("1.25")?);
    Ok(())
}

#[test]
fn ill_formed_xml_and_junk_are_errors_not_panics() {
    for junk in [&b""[..], b"\xff\xfe\x00", b"<a><b></a>", b"<Cube time='"] {
        assert!(parse_ecb_xml(junk).is_err(), "{junk:?}");
    }
}
