//! ECB reference rates: `eurofxref-daily.xml` and `eurofxref-hist.xml`.

use std::str::FromStr;

use jiff::civil::Date;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use reqwest::{Client, Url};
use rust_decimal::Decimal;
use tokenmtr_core::{EurPerUsd, FxRate};

use crate::error::RemoteError;
use crate::http::{Fetched, Validators, get_conditional, join};

const DAILY_PATH: &str = "/stats/eurofxref/eurofxref-daily.xml";
const HIST_PATH: &str = "/stats/eurofxref/eurofxref-hist.xml";
const DAILY_LIMIT: usize = 1 << 20;
/// The history file is ~6 MB and grows by ~100 KB a year.
const HIST_LIMIT: usize = 64 << 20;
const USD: &str = "USD";

/// Why an ECB document was rejected.
#[derive(Debug, thiserror::Error)]
pub enum EcbParseError {
    /// The XML is not well-formed.
    #[error("ECB XML is malformed: {0}")]
    Xml(#[from] quick_xml::Error),
    /// The document is well-formed but holds no usable USD rate.
    #[error("ECB XML holds no usable USD rate")]
    NoUsdRates,
}

/// Failure to fetch ECB rates.
#[derive(Debug, thiserror::Error)]
pub enum FxError {
    /// The HTTP exchange failed.
    #[error(transparent)]
    Remote(#[from] RemoteError),
    /// The body is not a usable ECB document.
    #[error(transparent)]
    Parse(#[from] EcbParseError),
}

/// The attributes of one `Cube` element that matter.
#[derive(Default)]
struct Cube {
    time: Option<String>,
    currency: Option<String>,
    rate: Option<String>,
}

impl Cube {
    fn read(element: &BytesStart<'_>) -> Self {
        let mut cube = Self::default();
        for attribute in element.attributes().flatten() {
            let Ok(value) = attribute.normalized_value(XmlVersion::Explicit1_0) else {
                continue;
            };
            match attribute.key.local_name().as_ref() {
                "time" => cube.time = Some(value.into_owned()),
                "currency" => cube.currency = Some(value.into_owned()),
                "rate" => cube.rate = Some(value.into_owned()),
                _ => {}
            }
        }
        cube
    }

    fn is_usd(&self) -> bool {
        self.currency.as_deref() == Some(USD)
    }

    /// The rate this element carries; `None` when there is no enclosing date or the rate is not
    /// a positive number.
    fn rate(&self, date: Option<Date>) -> Option<FxRate> {
        let (date, rate) = date.zip(self.rate.as_deref())?;
        let usd_per_eur = Decimal::from_str(rate.trim()).ok()?;
        let eur_per_usd = EurPerUsd::from_usd_per_eur(usd_per_eur).ok()?;
        Some(FxRate { date, eur_per_usd })
    }
}

/// The USD rates found so far and how many USD entries were unusable.
#[derive(Default)]
struct UsdRates {
    rates: Vec<FxRate>,
    unusable: usize,
}

impl UsdRates {
    fn collect(&mut self, cube: &Cube, date: Option<Date>) {
        if !cube.is_usd() {
            return;
        }
        match cube.rate(date) {
            Some(rate) => self.rates.push(rate),
            None => self.unusable = self.unusable.saturating_add(1),
        }
    }
}

/// Parses an ECB daily or history document into EUR-per-USD rates, in document order (the
/// history file is newest first; [`tokenmtr_core::FxTable::new`] sorts). Streaming and
/// tolerant: other currencies, unknown elements and unusable USD entries are skipped.
///
/// # Errors
/// [`EcbParseError::Xml`] for ill-formed XML, [`EcbParseError::NoUsdRates`] when no rate
/// survives.
pub fn parse_ecb_xml(xml: &[u8]) -> Result<Vec<FxRate>, EcbParseError> {
    let mut reader = Reader::from_reader(xml);
    let mut buffer = Vec::new();
    // A date applies to the rate elements nested in the `Cube` that declares it: track the
    // nesting depth so a stray rate outside any dated `Cube` is not given a stale date.
    let (mut depth, mut dated_depth, mut date) = (0_usize, 0_usize, None);
    let mut usd = UsdRates::default();
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Eof => break,
            Event::Start(e) if e.local_name().as_ref() == "Cube" => {
                depth += 1;
                let cube = Cube::read(&e);
                if let Some(time) = &cube.time {
                    date = time.parse::<Date>().ok();
                    dated_depth = depth;
                }
                usd.collect(&cube, date);
            }
            Event::Empty(e) if e.local_name().as_ref() == "Cube" => {
                usd.collect(&Cube::read(&e), date);
            }
            Event::End(e) if e.local_name().as_ref() == "Cube" => {
                if depth == dated_depth {
                    date = None;
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        buffer.clear();
    }
    if usd.unusable > 0 {
        tracing::warn!(
            unusable = usd.unusable,
            "skipped unusable USD rates in the ECB document"
        );
    }
    if usd.rates.is_empty() {
        return Err(EcbParseError::NoUsdRates);
    }
    Ok(usd.rates)
}

/// The result of [`EcbFx::fetch_daily`].
#[derive(Debug)]
pub enum FxFetch {
    /// HTTP 304: the stored rates are current.
    NotModified,
    /// A new document: its rates and the validators to send next time.
    New {
        /// The parsed rates.
        rates: Vec<FxRate>,
        /// `ETag` / `Last-Modified` of this response.
        validators: Validators,
    },
}

/// ECB rate client.
#[derive(Debug, Clone)]
pub struct EcbFx {
    client: Client,
    base: Url,
}

impl EcbFx {
    /// Builds the client over a shared HTTP client and the ECB base URL
    /// ([`crate::Endpoints::ecb`]).
    #[must_use]
    pub fn new(client: Client, base: Url) -> Self {
        Self { client, base }
    }

    /// Fetches `eurofxref-daily.xml`, conditionally on `validators` from the previous fetch.
    ///
    /// # Errors
    /// [`FxError`] on transport, status or parse failure.
    pub async fn fetch_daily(&self, validators: &Validators) -> Result<FxFetch, FxError> {
        let url = join(&self.base, DAILY_PATH)?;
        match get_conditional(&self.client, url, validators, DAILY_LIMIT).await? {
            Fetched::NotModified => Ok(FxFetch::NotModified),
            Fetched::Body { body, validators } => Ok(FxFetch::New {
                rates: parse_ecb_xml(&body)?,
                validators,
            }),
        }
    }

    /// Fetches `eurofxref-hist.xml`, the full backfill. Call once on first run.
    ///
    /// # Errors
    /// [`FxError`] on transport, status or parse failure.
    pub async fn fetch_history(&self) -> Result<Vec<FxRate>, FxError> {
        let url = join(&self.base, HIST_PATH)?;
        match get_conditional(&self.client, url, &Validators::default(), HIST_LIMIT).await? {
            Fetched::NotModified => Err(RemoteError::NotModified.into()),
            Fetched::Body { body, .. } => Ok(parse_ecb_xml(&body)?),
        }
    }
}
