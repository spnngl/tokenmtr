//! The two text exposition formats (P16, P21): Prometheus text 0.0.4 and `OpenMetrics` 1.0.0.
//!
//! Output is deterministic: [`Folded`] is sorted by family then labels, every family gets one
//! `# HELP` and one `# TYPE`, and a family without series is not written at all.
//!
//! `OpenMetrics` differs in three ways: a counter family is declared without `_total` while its
//! samples keep it; every counter series is followed by a `_created` sample (P21); and the body
//! ends with `# EOF`.

use std::fmt::Write as _;
use std::io::Write as _;

use flate2::Compression;
use flate2::write::GzEncoder;
use jiff::Timestamp;

use crate::catalog::{Family, Kind};
use crate::scope::{Folded, Series, Value};

/// `Content-Type` of [`Format::Plain`].
pub const PLAIN_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
/// `Content-Type` of [`Format::OpenMetrics`].
pub const OPENMETRICS_CONTENT_TYPE: &str =
    "application/openmetrics-text; version=1.0.0; charset=utf-8";

/// An exposition format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Prometheus text format 0.0.4.
    Plain,
    /// `OpenMetrics` 1.0.0, with `_created` samples.
    OpenMetrics,
}

impl Format {
    /// The `Content-Type` header value.
    #[must_use]
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Plain => PLAIN_CONTENT_TYPE,
            Self::OpenMetrics => OPENMETRICS_CONTENT_TYPE,
        }
    }

    /// The format an `Accept` header asks for: `OpenMetrics` when it is listed with a non-zero
    /// quality at least as high as `text/plain`'s, else the plain text format. No header, `*/*`
    /// and everything unknown mean plain.
    #[must_use]
    pub fn negotiate(accept: Option<&str>) -> Self {
        let Some(accept) = accept else {
            return Self::Plain;
        };
        let (mut openmetrics, mut plain) = (None, None);
        for range in accept.split(',') {
            let mut parts = range.split(';').map(str::trim);
            let media = parts.next().unwrap_or_default().to_ascii_lowercase();
            let quality = parts
                .find_map(|p| p.strip_prefix("q=").or_else(|| p.strip_prefix("Q=")))
                .map_or(Some(1.0), |q| q.trim().parse::<f32>().ok())
                .unwrap_or(0.0);
            let slot = match media.as_str() {
                "application/openmetrics-text" => &mut openmetrics,
                "text/plain" => &mut plain,
                _ => continue,
            };
            if slot.is_none_or(|best| quality > best) {
                *slot = Some(quality);
            }
        }
        match (openmetrics, plain) {
            (Some(om), Some(pl)) if om > 0.0 && om >= pl => Self::OpenMetrics,
            (Some(om), None) if om > 0.0 => Self::OpenMetrics,
            _ => Self::Plain,
        }
    }
}

/// Encodes `folded` in `format`.
#[must_use]
pub fn encode(folded: &Folded, format: Format) -> String {
    let mut out = String::new();
    let mut at = 0;
    while at < folded.series.len() {
        let family = folded.series[at].family;
        let end = folded.series[at..]
            .iter()
            .position(|s| s.family != family)
            .map_or(folded.series.len(), |n| at + n);
        write_family(&mut out, family, &folded.series[at..end], format);
        at = end;
    }
    if format == Format::OpenMetrics {
        out.push_str("# EOF\n");
    }
    out
}

fn write_family(out: &mut String, family: Family, series: &[Series], format: Format) {
    let name = family.name();
    let declared = match (format, family.kind()) {
        (Format::OpenMetrics, Kind::Counter) => name.strip_suffix("_total").unwrap_or(name),
        _ => name,
    };
    let kind = match family.kind() {
        Kind::Counter => "counter",
        Kind::Gauge => "gauge",
    };
    // Writing to a `String` cannot fail.
    let _ = writeln!(out, "# HELP {declared} {}", escape_help(family.help()));
    let _ = writeln!(out, "# TYPE {declared} {kind}");
    for s in series {
        write_sample(out, name, s, &s.value);
        if let (Format::OpenMetrics, Some(created)) = (format, s.created) {
            write_created(out, name, s, created);
        }
    }
}

fn write_sample(out: &mut String, name: &str, series: &Series, value: &Value) {
    out.push_str(name);
    write_labels(out, series);
    out.push(' ');
    match *value {
        Value::Count(n) => {
            let _ = write!(out, "{n}");
        }
        Value::Gauge(g) => write_float(out, g),
    }
    out.push('\n');
}

fn write_created(out: &mut String, name: &str, series: &Series, created: Timestamp) {
    out.push_str(name.strip_suffix("_total").unwrap_or(name));
    out.push_str("_created");
    write_labels(out, series);
    let millis = created.as_millisecond();
    let _ = writeln!(
        out,
        " {}.{:03}",
        millis.div_euclid(1000),
        millis.rem_euclid(1000)
    );
}

fn write_labels(out: &mut String, series: &Series) {
    if series.labels.is_empty() {
        return;
    }
    out.push('{');
    for (n, (name, value)) in series.labels.iter().enumerate() {
        if n > 0 {
            out.push(',');
        }
        out.push_str(name);
        out.push_str("=\"");
        escape_into(out, value, true);
        out.push('"');
    }
    out.push('}');
}

fn write_float(out: &mut String, value: f64) {
    if value.is_nan() {
        out.push_str("NaN");
    } else if value.is_infinite() {
        out.push_str(if value > 0.0 { "+Inf" } else { "-Inf" });
    } else {
        let _ = write!(out, "{value}");
    }
}

/// `\\`, `\"` (label values only) and `\n`.
fn escape_into(out: &mut String, text: &str, quotes: bool) {
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '"' if quotes => out.push_str("\\\""),
            c => out.push(c),
        }
    }
}

fn escape_help(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    escape_into(&mut out, text, false);
    out
}

/// A body and its gzip, built once per refresh so a scrape costs a clone of two byte buffers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Body {
    /// The text.
    pub identity: Vec<u8>,
    /// The same text, gzipped.
    pub gzip: Vec<u8>,
}

/// Both formats of one fold, plain and gzipped (P12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// Prometheus text format 0.0.4.
    pub plain: Body,
    /// `OpenMetrics` 1.0.0.
    pub openmetrics: Body,
}

impl Rendered {
    /// Encodes and compresses `folded` in both formats.
    #[must_use]
    pub fn new(folded: &Folded) -> Self {
        Self {
            plain: body(&encode(folded, Format::Plain)),
            openmetrics: body(&encode(folded, Format::OpenMetrics)),
        }
    }

    /// The body of `format`.
    #[must_use]
    pub const fn body(&self, format: Format) -> &Body {
        match format {
            Format::Plain => &self.plain,
            Format::OpenMetrics => &self.openmetrics,
        }
    }
}

impl Default for Rendered {
    /// What an exporter serves before its first refresh: no series.
    fn default() -> Self {
        Self::new(&Folded {
            taken_at: Timestamp::UNIX_EPOCH,
            series: Vec::new(),
        })
    }
}

fn body(text: &str) -> Body {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    // Writing to a `Vec` cannot fail.
    let _ = encoder.write_all(text.as_bytes());
    Body {
        identity: text.as_bytes().to_vec(),
        gzip: encoder.finish().unwrap_or_default(),
    }
}
