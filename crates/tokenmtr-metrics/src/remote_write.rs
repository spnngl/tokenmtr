//! Prometheus Remote Write 1.0 payloads (P13): protobuf, raw snappy, batching, zero samples.
//!
//! The wire structs are written by hand with `prost`'s derive, so the build needs no `protoc`.
//! Everything here is pure; sending is `tokenmtr-remote`'s job.
//!
//! A push carries one sample per series, stamped `now`. A counter series that appears while the
//! process runs gets a `0` sample one millisecond earlier, so Prometheus' `increase()` counts its
//! first value instead of losing it. The first push after a start or reload adds none: its series
//! may already exist at the receiver, and a zero would count them twice.

use std::collections::HashMap;

use jiff::{SignedDuration, Timestamp};
use prost::Message;

use crate::catalog::{Family, Kind};
use crate::error::MetricsError;
use crate::scope::{Folded, Series, Value};

/// Series per request; Prometheus' own `max_samples_per_send` default.
pub const MAX_SERIES_PER_REQUEST: usize = 2000;

/// `WriteRequest` of `prompb/remote.proto`.
#[derive(Clone, PartialEq, Message)]
pub struct WriteRequest {
    /// The series.
    #[prost(message, repeated, tag = "1")]
    pub timeseries: Vec<TimeSeries>,
}

/// `TimeSeries` of `prompb/types.proto`.
#[derive(Clone, PartialEq, Message)]
pub struct TimeSeries {
    /// Sorted by name, `__name__` included.
    #[prost(message, repeated, tag = "1")]
    pub labels: Vec<Label>,
    /// In time order.
    #[prost(message, repeated, tag = "2")]
    pub samples: Vec<Sample>,
}

/// `Label` of `prompb/types.proto`.
#[derive(Clone, PartialEq, Message)]
pub struct Label {
    /// The name.
    #[prost(string, tag = "1")]
    pub name: String,
    /// The value.
    #[prost(string, tag = "2")]
    pub value: String,
}

/// `Sample` of `prompb/types.proto`.
#[derive(Clone, Copy, PartialEq, Message)]
pub struct Sample {
    /// The value.
    #[prost(double, tag = "1")]
    pub value: f64,
    /// Milliseconds since the Unix epoch.
    #[prost(int64, tag = "2")]
    pub timestamp: i64,
}

/// What identifies a counter series across pushes: its family and label values (the names are
/// the family's).
type SeriesKey = (Family, Vec<String>);

/// A request to send, with the counters it carries so [`Seen::accept`] can record them.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    body: WriteRequest,
    counters: Vec<(SeriesKey, Timestamp)>,
}

impl Request {
    /// Series in the request.
    #[must_use]
    pub const fn series(&self) -> usize {
        self.body.timeseries.len()
    }

    /// The decoded payload.
    #[must_use]
    pub const fn payload(&self) -> &WriteRequest {
        &self.body
    }

    /// The request body: the protobuf, snappy-compressed as a raw block (not the framed format).
    ///
    /// # Errors
    ///
    /// The payload exceeds what snappy can compress.
    pub fn encode(&self) -> Result<Vec<u8>, MetricsError> {
        Ok(snap::raw::Encoder::new().compress_vec(&self.body.encode_to_vec())?)
    }
}

/// The counter series this process has pushed recently, and whether it has pushed at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    primed: bool,
    /// Counter series whose `created` is still inside the window, with that `created`.
    recent: HashMap<SeriesKey, Timestamp>,
}

impl Seen {
    /// Records the counters of a request the receiver accepted. Entries whose `created` left the
    /// window are dropped: such a series is old, and old series get no zero sample anyway.
    pub fn accept(&mut self, request: &Request, now: Timestamp, window: SignedDuration) {
        self.primed = true;
        for (key, created) in &request.counters {
            self.recent.insert(key.clone(), *created);
        }
        let oldest = now.checked_sub(window).unwrap_or(Timestamp::MIN);
        self.recent.retain(|_, created| *created >= oldest);
    }

    /// Series remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.recent.len()
    }

    /// Whether no series is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.recent.is_empty()
    }
}

/// Splits `folded` into requests of at most [`MAX_SERIES_PER_REQUEST`] series, every sample
/// stamped `now`, with `external` labels merged into each series.
///
/// A counter missing from `seen` whose `created` is within `window` of `now` also gets a `0`
/// sample at `now - 1 ms`, unless `seen` has never accepted a request.
#[must_use]
pub fn requests(
    folded: &Folded,
    seen: &Seen,
    external: &[(String, String)],
    now: Timestamp,
    window: SignedDuration,
) -> Vec<Request> {
    let stamp = now.as_millisecond();
    let oldest = now.checked_sub(window).unwrap_or(Timestamp::MIN);
    let mut out = Vec::new();
    for chunk in folded.series.chunks(MAX_SERIES_PER_REQUEST) {
        let mut body = WriteRequest::default();
        let mut counters = Vec::new();
        for series in chunk {
            let mut samples = Vec::with_capacity(2);
            if let (Kind::Counter, Some(created)) = (series.family.kind(), series.created) {
                let key: SeriesKey = (
                    series.family,
                    series.labels.iter().map(|(_, v)| v.clone()).collect(),
                );
                if seen.primed && created >= oldest && !seen.recent.contains_key(&key) {
                    samples.push(Sample {
                        value: 0.0,
                        timestamp: stamp.saturating_sub(1),
                    });
                }
                counters.push((key, created));
            }
            samples.push(Sample {
                value: number(series),
                timestamp: stamp,
            });
            body.timeseries.push(TimeSeries {
                labels: labels(series, external),
                samples,
            });
        }
        out.push(Request { body, counters });
    }
    out
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a counter above 2^53 is not a realistic token count, and the wire value is a double"
)]
const fn number(series: &Series) -> f64 {
    match series.value {
        Value::Count(n) => n as f64,
        Value::Gauge(g) => g,
    }
}

fn labels(series: &Series, external: &[(String, String)]) -> Vec<Label> {
    let mut labels: Vec<Label> = std::iter::once(("__name__", series.family.name()))
        .map(|(n, v)| Label {
            name: n.to_owned(),
            value: v.to_owned(),
        })
        .chain(series.labels.iter().map(|(n, v)| Label {
            name: (*n).to_owned(),
            value: v.clone(),
        }))
        .chain(external.iter().map(|(n, v)| Label {
            name: n.clone(),
            value: v.clone(),
        }))
        .collect();
    labels.sort_by(|a, b| a.name.cmp(&b.name));
    labels
}
