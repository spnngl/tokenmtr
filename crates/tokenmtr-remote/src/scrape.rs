//! The scrape endpoint: `GET /metrics`, nothing else.
//!
//! A scrape never touches the store. It serves the bodies the daemon pre-rendered into a
//! `watch` channel (plain and `OpenMetrics`, each identity and gzipped), so its cost is a header
//! parse and a buffer copy, however slow the store is.
//!
//! Authentication, when configured, is one constant-time comparison of digests
//! ([`crate::credential`]). There is no TLS; a reverse proxy provides it.

use std::convert::Infallible;
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{
    ACCEPT, ACCEPT_ENCODING, AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE, VARY, WWW_AUTHENTICATE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use tokenmtr_metrics::text::{Format, Rendered};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};

use crate::credential::{Credential, Expected};

/// Open connections served at once; a further one waits in the accept queue.
const MAX_CONNECTIONS: usize = 64;
/// How long a client may take to send its request headers, idle keep-alive included.
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest a connection lives, keep-alive included; a scraper reconnects.
const CONNECTION_LIFETIME: Duration = Duration::from_secs(60);
/// How long a shutdown waits for in-flight scrapes.
const DRAIN: Duration = Duration::from_secs(2);
const PATH: &str = "/metrics";

/// What every connection shares.
struct State {
    rendered: watch::Receiver<Arc<Rendered>>,
    guard: Option<Guard>,
}

struct Guard {
    expected: Expected,
    challenge: &'static str,
}

/// Serves `GET /metrics` on `listener` until `shutdown` completes, then lets in-flight scrapes
/// finish (at most two seconds).
///
/// Binding is the caller's job, so a busy port is the caller's to retry.
///
/// # Errors
///
/// Never in practice; accept failures are logged and retried.
pub async fn serve(
    listener: TcpListener,
    rendered: watch::Receiver<Arc<Rendered>>,
    auth: Option<&Credential>,
    shutdown: impl Future<Output = ()>,
) -> io::Result<()> {
    let state = Arc::new(State {
        rendered,
        guard: auth.map(|credential| Guard {
            expected: Expected::of(credential),
            challenge: credential.challenge(),
        }),
    });
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let graceful = GracefulShutdown::new();
    tokio::pin!(shutdown);
    loop {
        let permit = tokio::select! {
            () = &mut shutdown => break,
            permit = Arc::clone(&slots).acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_closed) => break,
            },
        };
        let stream = tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _peer)) => stream,
                Err(error) => {
                    tracing::warn!(kind = ?error.kind(), "scrape accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
        };
        let state = Arc::clone(&state);
        let service = service_fn(move |request: Request<Incoming>| {
            let state = Arc::clone(&state);
            async move { Ok::<_, Infallible>(respond(&state, &request)) }
        });
        let connection = http1::Builder::new()
            .timer(TokioTimer::new())
            .header_read_timeout(HEADER_TIMEOUT)
            .serve_connection(TokioIo::new(stream), service);
        let connection = graceful.watch(connection);
        tokio::spawn(async move {
            // A client that hangs up mid-scrape is routine; the kind is all worth a trace.
            // The deadline keeps a client that never reads its answer from holding a slot.
            match tokio::time::timeout(CONNECTION_LIFETIME, connection).await {
                Ok(Err(error)) => tracing::debug!(error = %error, "scrape connection ended"),
                Ok(Ok(())) => {}
                Err(_elapsed) => tracing::debug!("scrape connection closed after its lifetime"),
            }
            drop(permit);
        });
    }
    if tokio::time::timeout(DRAIN, graceful.shutdown())
        .await
        .is_err()
    {
        tracing::debug!("scrape connections still open at shutdown");
    }
    Ok(())
}

fn respond(state: &State, request: &Request<Incoming>) -> Response<Full<Bytes>> {
    if request.uri().path() != PATH || request.method() != Method::GET {
        return reply(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            None,
            "not found\n",
        );
    }
    let headers = request.headers();
    if let Some(guard) = &state.guard {
        let presented = headers
            .get(AUTHORIZATION)
            .map(hyper::header::HeaderValue::as_bytes);
        if !guard.expected.accepts(presented) {
            let mut response = reply(
                StatusCode::UNAUTHORIZED,
                "text/plain; charset=utf-8",
                None,
                "unauthorized\n",
            );
            if let Ok(challenge) = guard.challenge.parse() {
                response.headers_mut().insert(WWW_AUTHENTICATE, challenge);
            }
            return response;
        }
    }
    let format = Format::negotiate(text(headers, &ACCEPT));
    let gzip = accepts_gzip(text(headers, &ACCEPT_ENCODING));
    let rendered = Arc::clone(&state.rendered.borrow());
    let body = rendered.body(format);
    let (bytes, encoding) = if gzip {
        (body.gzip.clone(), Some("gzip"))
    } else {
        (body.identity.clone(), None)
    };
    let mut response = reply(StatusCode::OK, format.content_type(), encoding, bytes);
    response.headers_mut().insert(
        VARY,
        hyper::header::HeaderValue::from_static("Accept, Accept-Encoding"),
    );
    response
}

fn text<'a>(headers: &'a HeaderMap, name: &hyper::header::HeaderName) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

fn reply(
    status: StatusCode,
    content_type: &'static str,
    encoding: Option<&'static str>,
    body: impl Into<Bytes>,
) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(body.into()));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        hyper::header::HeaderValue::from_static(content_type),
    );
    if let Some(encoding) = encoding {
        headers.insert(
            CONTENT_ENCODING,
            hyper::header::HeaderValue::from_static(encoding),
        );
    }
    response
}

/// Whether an `Accept-Encoding` header lists `gzip` with a non-zero quality. `*` does not count:
/// the exporter only compresses for a client that names it.
#[must_use]
pub fn accepts_gzip(accept_encoding: Option<&str>) -> bool {
    accept_encoding.is_some_and(|header| {
        header.split(',').any(|item| {
            let mut parts = item.split(';').map(str::trim);
            parts
                .next()
                .is_some_and(|coding| coding.eq_ignore_ascii_case("gzip"))
                && parts
                    .find_map(|p| p.strip_prefix("q=").or_else(|| p.strip_prefix("Q=")))
                    .is_none_or(|q| q.trim().parse::<f32>().is_ok_and(|q| q > 0.0))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::accepts_gzip;

    #[test]
    fn gzip_is_used_only_when_named_with_a_quality_above_zero() {
        for (header, expected) in [
            (None, false),
            (Some(""), false),
            (Some("gzip"), true),
            (Some("GZIP"), true),
            (Some("deflate, gzip;q=0.5"), true),
            (Some("gzip;q=0"), false),
            (Some("gzip;q=0.0"), false),
            (Some("gzip;q=x"), false),
            (Some("identity"), false),
            (Some("*"), false),
            (Some("x-gzip"), false),
        ] {
            assert_eq!(accepts_gzip(header), expected, "{header:?}");
        }
    }
}
