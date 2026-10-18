//! Golden renderings and the invariants of the paid-cost allocation.

use jiff::Timestamp;
use jiff::civil::date;
use proptest::prelude::*;
use tokenmtr_core::Currency;
use tokenmtr_report::{
    Amount, DayInput, ModelInput, Period, ProjectInput, Report, ReportError, ReportInput, Scope,
    SubscriptionInput, SubscriptionShare, allocate, render_csv, render_html, render_markdown,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const fn micros(value: u64) -> Amount {
    Amount::from_micros(value)
}

fn share(subscription: &str, value: u64) -> SubscriptionShare {
    SubscriptionShare {
        subscription: subscription.to_owned(),
        api_value: micros(value),
    }
}

/// May 2026: two projects (one of a client), one subscription, one approximate day and model.
fn may() -> TestResult<ReportInput> {
    Ok(ReportInput {
        period: Period {
            label: "2026-05".to_owned(),
            first: date(2026, 5, 1),
            last: date(2026, 5, 31),
        },
        scope: Scope::All,
        currency: Currency::Eur,
        generated_at: "2026-06-01T08:00:00Z".parse::<Timestamp>()?,
        sessions: 12,
        projects: vec![
            ProjectInput {
                name: "beta".to_owned(),
                client: None,
                approximate: false,
                pay_as_you_go: micros(0),
                subscriptions: vec![share("claude-max", 10_000_000)],
            },
            ProjectInput {
                name: "alpha".to_owned(),
                client: Some("Acme".to_owned()),
                approximate: true,
                pay_as_you_go: micros(5_500_000),
                subscriptions: vec![share("claude-max", 20_000_000)],
            },
        ],
        models: vec![
            ModelInput {
                model: "claude-sonnet-4".to_owned(),
                tokens: 3_000_000,
                api_value: micros(10_000_000),
                approximate: false,
            },
            ModelInput {
                model: "claude-opus-4".to_owned(),
                tokens: 1_500_000,
                api_value: micros(25_500_000),
                approximate: true,
            },
        ],
        days: vec![
            DayInput {
                date: date(2026, 5, 4),
                api_value: micros(3_500_000),
                approximate: false,
            },
            DayInput {
                date: date(2026, 5, 1),
                api_value: micros(12_000_000),
                approximate: false,
            },
            DayInput {
                date: date(2026, 5, 2),
                api_value: micros(20_000_000),
                approximate: true,
            },
        ],
        subscriptions: vec![SubscriptionInput {
            id: "claude-max".to_owned(),
            paid: micros(100_000_000),
            outside_scope: micros(0),
        }],
    })
}

#[test]
fn paid_is_allocated_by_api_value_share_and_adds_up() -> TestResult {
    let report = Report::from_input(may()?)?;
    let alpha = report
        .projects
        .iter()
        .find(|p| p.name == "alpha")
        .ok_or("alpha")?;
    let beta = report
        .projects
        .iter()
        .find(|p| p.name == "beta")
        .ok_or("beta")?;
    // 100 EUR split 20:10; the odd micro goes to the larger remainder.
    assert_eq!(alpha.paid, micros(5_500_000 + 66_666_667));
    assert_eq!(beta.paid, micros(33_333_333));
    assert_eq!(report.totals.api_value, micros(35_500_000));
    assert_eq!(report.totals.paid, micros(105_500_000));
    let allocated: u64 = report.projects.iter().map(|p| p.paid.micros()).sum();
    assert_eq!(allocated, report.totals.paid.micros());
    assert_eq!(report.projects[0].name, "alpha", "highest API value first");
    assert_eq!(report.days[0].date, date(2026, 5, 1), "oldest day first");
    assert!(report.totals.approximate);
    Ok(())
}

#[test]
fn outside_scope_usage_takes_its_share_out_of_the_report() -> TestResult {
    let mut input = may()?;
    input.scope = Scope::Client("Acme".to_owned());
    input.projects.remove(0);
    input.models = vec![ModelInput {
        model: "claude-opus-4".to_owned(),
        tokens: 1,
        api_value: micros(25_500_000),
        approximate: false,
    }];
    input.days = vec![DayInput {
        date: date(2026, 5, 1),
        api_value: micros(25_500_000),
        approximate: false,
    }];
    input.subscriptions[0].outside_scope = micros(10_000_000);
    let report = Report::from_input(input)?;
    assert_eq!(report.totals.paid, micros(5_500_000 + 66_666_667));
    Ok(())
}

#[test]
fn a_subscription_nobody_used_is_counted_once_for_all_scope() -> TestResult {
    let mut input = may()?;
    input.subscriptions.push(SubscriptionInput {
        id: "idle".to_owned(),
        paid: micros(20_000_000),
        outside_scope: micros(0),
    });
    let report = Report::from_input(input.clone())?;
    assert_eq!(report.unused_paid, micros(20_000_000));
    assert_eq!(report.totals.paid, micros(125_500_000));

    input.scope = Scope::Project("alpha".to_owned());
    let report = Report::from_input(input)?;
    assert_eq!(
        report.totals.paid,
        micros(105_500_000),
        "unused fees belong to no scope"
    );
    Ok(())
}

#[test]
fn contradicting_inputs_are_refused() -> TestResult {
    let mut unknown = may()?;
    unknown.projects[0].subscriptions[0].subscription = "nope".to_owned();
    assert!(matches!(
        Report::from_input(unknown),
        Err(ReportError::UnknownSubscription { .. })
    ));

    let mut duplicate = may()?;
    duplicate
        .subscriptions
        .push(duplicate.subscriptions[0].clone());
    assert!(matches!(
        Report::from_input(duplicate),
        Err(ReportError::DuplicateSubscription(_))
    ));

    let mut twice = may()?;
    twice.projects[0].subscriptions.push(share("claude-max", 1));
    assert!(matches!(
        Report::from_input(twice),
        Err(ReportError::DuplicateShare { .. })
    ));

    let mut disagree = may()?;
    disagree.days[0].api_value = micros(1);
    assert!(matches!(
        Report::from_input(disagree),
        Err(ReportError::TotalsDisagree { .. })
    ));

    let mut overflow = may()?;
    overflow.projects[0].pay_as_you_go = micros(u64::MAX);
    assert_eq!(Report::from_input(overflow), Err(ReportError::Overflow));
    Ok(())
}

#[test]
fn markdown_golden() -> TestResult {
    insta::assert_snapshot!(render_markdown(&Report::from_input(may()?)?));
    Ok(())
}

#[test]
fn csv_golden() -> TestResult {
    insta::assert_snapshot!(render_csv(&Report::from_input(may()?)?)?);
    Ok(())
}

#[test]
fn html_golden() -> TestResult {
    insta::assert_snapshot!(render_html(&Report::from_input(may()?)?));
    Ok(())
}

/// A report whose names try to break out of every format.
fn hostile() -> TestResult<Report> {
    let mut input = may()?;
    "<script>alert(1)</script> & \"x\" | y".clone_into(&mut input.projects[1].name);
    input.projects[1].client = Some("=HYPERLINK(\"http://evil\")".to_owned());
    "<img src=x onerror=alert(1)>".clone_into(&mut input.models[0].model);
    Ok(Report::from_input(input)?)
}

#[test]
fn html_escapes_every_text_and_loads_nothing_external() -> TestResult {
    let html = render_html(&hostile()?);
    assert!(!html.contains("<script"), "{html}");
    assert!(!html.contains("<img"), "{html}");
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;x&quot; | y"));
    for external in [" src=\"", " href=\"", "@import", "url(", "<link"] {
        assert!(!html.contains(external), "{external}");
    }
    Ok(())
}

#[test]
fn markdown_escapes_table_breakers() -> TestResult {
    let md = render_markdown(&hostile()?);
    assert!(
        md.contains("\\<script\\>alert(1)\\</script\\> & \"x\" \\| y"),
        "{md}"
    );
    Ok(())
}

#[test]
fn csv_neutralizes_formulas_and_round_trips() -> TestResult {
    let text = render_csv(&hostile()?)?;
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    let rows = reader.records().collect::<Result<Vec<_>, _>>()?;
    assert_eq!(rows.len(), 1 + 2 + 2 + 3);
    let client = rows
        .iter()
        .find(|r| &r[0] == "project" && r[1].contains("script"))
        .map(|r| r[2].to_owned())
        .ok_or("project row")?;
    assert!(client.starts_with("'="), "{client}");
    let total = &rows[0];
    assert_eq!(
        (&total[0], &total[5], &total[6], &total[8]),
        ("total", "35.500000", "105.500000", "EUR")
    );
    Ok(())
}

proptest! {
    #[test]
    fn allocation_adds_up_exactly(
        paid in 0_u64..=u64::MAX / 2,
        weights in proptest::collection::vec(0_u64..=1_000_000_000_000, 1..8),
    ) {
        let weights: Vec<Amount> = weights.into_iter().map(Amount::from_micros).collect();
        let parts = allocate(micros(paid), &weights);
        prop_assert_eq!(parts.len(), weights.len());
        let total: u128 = weights.iter().map(|w| u128::from(w.micros())).sum();
        let sum: u128 = parts.iter().map(|p| u128::from(p.micros())).sum();
        prop_assert_eq!(sum, if total == 0 { 0 } else { u128::from(paid) });
        for (part, weight) in parts.iter().zip(&weights) {
            if weight.micros() == 0 {
                prop_assert_eq!(part.micros(), 0);
            }
        }
    }
}
