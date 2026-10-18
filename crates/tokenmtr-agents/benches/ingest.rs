//! Ingest benchmarks: cold scan, 1 MB append, parser throughput with and without the
//! line pre-filter, and `upsert_events`.
//!
//! The corpus is generated Claude transcripts. `TOKENMTR_BENCH_MB` sets its size (default 2, so
//! the suite runs in seconds; use 100 for the plan's reference corpus). Run with
//! `cargo bench -p tokenmtr-agents --bench ingest`.

#![expect(
    clippy::expect_used,
    reason = "a benchmark has no caller to report setup failures to"
)]

use std::fs;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use camino::{Utf8Path, Utf8PathBuf};
use divan::Bencher;
use divan::counter::{BytesCount, ItemsCount};
use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::json;
use tokenmtr_agents::ingest::{IngestSettings, Ingestor, ScanPriority, ScanRequest};
use tokenmtr_agents::{
    AgentKind, AgentSource as _, ByteOffset, ClaudeSource, ParseBatch, Profile, SourceFile,
};
use tokenmtr_core::{ParsedEvent, ProfileId};
use tokenmtr_store::Store;

/// Transcript files in the corpus.
const FILES: u64 = 8;
/// Epoch seconds of the first generated event (2026-03-01T00:00:00Z).
const BASE_SECONDS: i64 = 1_772_323_200;
/// Most events one `upsert_events` call gets, like the engine's transaction size.
const UPSERT_BATCH: usize = 5000;

struct Corpus {
    /// Claude config dir; transcripts are under `projects/-bench`.
    config_dir: Utf8PathBuf,
    files: Vec<Utf8PathBuf>,
    bytes: u64,
}

static CORPUS: OnceLock<Corpus> = OnceLock::new();
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn bench_dir() -> Utf8PathBuf {
    let tmp = Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("utf-8 temp dir");
    tmp.join(format!("tokenmtr-bench-{}", std::process::id()))
}

fn megabytes() -> u64 {
    std::env::var("TOKENMTR_BENCH_MB")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2)
}

/// An assistant line with usage, about 650 bytes.
fn assistant_line(n: u64) -> String {
    let at = Timestamp::from_second(BASE_SECONDS + i64::try_from(n).expect("small"))
        .expect("valid time");
    json!({
        "parentUuid": null, "isSidechain": false, "userType": "external",
        "cwd": "/bench/app", "sessionId": format!("s{}", n / 500), "version": "2.1.0",
        "gitBranch": "main", "type": "assistant",
        "message": {
            "id": format!("msg_{n}"), "type": "message", "role": "assistant",
            "model": "claude-sonnet-4-5",
            "content": [{"type": "text", "text": "benchmark filler text ".repeat(8)}],
            "stop_reason": null,
            "usage": {
                "input_tokens": 100 + n % 50, "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 9000,
                "cache_creation": {"ephemeral_5m_input_tokens": 0, "ephemeral_1h_input_tokens": 0},
                "output_tokens": 20 + n % 30, "service_tier": "standard"
            }
        },
        "requestId": format!("req_{n}"), "uuid": format!("u-{n}"), "timestamp": at.to_string()
    })
    .to_string()
}

/// A user line without usage: what the pre-filter should skip.
fn user_line(n: u64) -> String {
    json!({
        "parentUuid": null, "isSidechain": false, "cwd": "/bench/app",
        "sessionId": format!("s{}", n / 500), "version": "2.1.0", "type": "user",
        "message": {"role": "user", "content": "benchmark prompt ".repeat(12)},
        "uuid": format!("p-{n}"), "timestamp": "2026-03-01T00:00:00.000Z"
    })
    .to_string()
}

/// About `bytes` bytes of lines numbered from `first` (two assistant lines per user line).
/// Returns the text and the next free number.
fn body(first: u64, bytes: u64) -> (String, u64) {
    let mut text = String::new();
    let mut n = first;
    while (text.len() as u64) < bytes {
        let line = if n % 3 == 2 {
            user_line(n)
        } else {
            assistant_line(n)
        };
        text.push_str(&line);
        text.push('\n');
        n += 1;
    }
    (text, n)
}

fn corpus() -> &'static Corpus {
    CORPUS.get_or_init(|| {
        let config_dir = bench_dir().join("corpus");
        let project = config_dir.join("projects").join("-bench");
        fs::create_dir_all(&project).expect("create corpus dir");
        let per_file = megabytes() * 1_000_000 / FILES;
        let mut first = 0;
        let mut files = Vec::new();
        let mut bytes = 0;
        for index in 0..FILES {
            let (text, next) = body(first, per_file);
            first = next;
            let path = project.join(format!("session-{index}.jsonl"));
            fs::write(&path, &text).expect("write transcript");
            bytes += text.len() as u64;
            files.push(path);
        }
        Corpus {
            config_dir,
            files,
            bytes,
        }
    })
}

fn profile(config_dir: &Utf8Path) -> Profile {
    Profile::new(
        ProfileId::default_profile(),
        vec![config_dir.join("projects")],
    )
}

fn ingestor(config_dir: &Utf8Path, store_path: &Utf8Path) -> Ingestor {
    let settings = IngestSettings {
        sources: vec![AgentKind::Claude(ClaudeSource::new(vec![profile(
            config_dir,
        )]))],
        rules: Vec::new(),
        overrides: Vec::new(),
        zone: TimeZone::UTC,
        import_claude_stats_cache: false,
        protected: Vec::new(),
        redact_paths: false,
    };
    Ingestor::open(settings, store_path).expect("open ingestor")
}

fn fresh_store() -> (Store, Utf8PathBuf) {
    let path = bench_dir().join(format!(
        "store-{}.db",
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(bench_dir()).expect("create bench dir");
    (Store::open(&path).expect("open store"), path)
}

fn cold_scan(bencher: Bencher<'_, '_>, priority: ScanPriority) {
    let corpus = corpus();
    bencher
        .counter(BytesCount::new(corpus.bytes))
        .with_inputs(|| {
            let (store, path) = fresh_store();
            (store, ingestor(&corpus.config_dir, &path))
        })
        .bench_local_values(|(mut store, mut ingestor)| {
            ingestor
                .scan(&mut store, &ScanRequest::incremental(priority))
                .expect("scan")
        });
}

/// First scan of the whole corpus, files parsed in parallel (`scan` / first run).
#[divan::bench(sample_count = 3, sample_size = 1)]
fn cold_scan_parallel(bencher: Bencher<'_, '_>) {
    cold_scan(bencher, ScanPriority::UserInitiated);
}

/// First scan of the whole corpus, one file at a time (daemon background).
#[divan::bench(sample_count = 3, sample_size = 1)]
fn cold_scan_sequential(bencher: Bencher<'_, '_>) {
    cold_scan(bencher, ScanPriority::Background);
}

/// An incremental scan that finds 1 MB appended to a file that was already ingested.
#[divan::bench(sample_count = 5, sample_size = 1)]
fn append_one_megabyte(bencher: Bencher<'_, '_>) {
    bencher
        .counter(BytesCount::new(1_000_000_u64))
        .with_inputs(|| {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let config_dir = bench_dir().join(format!("append-{sequence}"));
            let project = config_dir.join("projects").join("-bench");
            fs::create_dir_all(&project).expect("create dir");
            let path = project.join("session.jsonl");
            let (text, next) = body(0, 1_000_000);
            fs::write(&path, text).expect("write transcript");
            let (mut store, store_path) = fresh_store();
            let mut ingestor = ingestor(&config_dir, &store_path);
            ingestor
                .scan(
                    &mut store,
                    &ScanRequest::incremental(ScanPriority::UserInitiated),
                )
                .expect("first scan");
            let (more, _) = body(next, 1_000_000);
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .expect("open");
            std::io::Write::write_all(&mut file, more.as_bytes()).expect("append");
            (store, ingestor)
        })
        .bench_local_values(|(mut store, mut ingestor)| {
            ingestor
                .scan(
                    &mut store,
                    &ScanRequest::incremental(ScanPriority::Background),
                )
                .expect("scan")
        });
}

/// Parser throughput on the corpus, with and without the byte pre-filter.
#[divan::bench(args = [true, false], sample_count = 5, sample_size = 1)]
fn parse_corpus(bencher: Bencher<'_, '_>, prefilter: bool) {
    let corpus = corpus();
    let profile = profile(&corpus.config_dir);
    let source = if prefilter {
        ClaudeSource::new(vec![profile.clone()])
    } else {
        ClaudeSource::new(vec![profile.clone()]).without_prefilter()
    };
    bencher
        .counter(BytesCount::new(corpus.bytes))
        .bench_local(|| {
            for path in &corpus.files {
                let file = SourceFile::new(path.clone(), profile.id.clone());
                let mut batch = ParseBatch::new();
                source
                    .parse(&file, ByteOffset::ZERO, &mut batch)
                    .expect("parse");
                divan::black_box(batch);
            }
        });
}

/// One `upsert_events` transaction of up to 5000 new events.
#[divan::bench(sample_count = 5, sample_size = 1)]
fn upsert_events(bencher: Bencher<'_, '_>) {
    let corpus = corpus();
    let profile = profile(&corpus.config_dir);
    let source = ClaudeSource::new(vec![profile.clone()]);
    let mut parsed: Vec<ParsedEvent> = Vec::new();
    for path in &corpus.files {
        let file = SourceFile::new(path.clone(), profile.id.clone());
        let mut batch = ParseBatch::new();
        source
            .parse(&file, ByteOffset::ZERO, &mut batch)
            .expect("parse");
        parsed.extend(batch.into_parts().events);
        if parsed.len() >= UPSERT_BATCH {
            break;
        }
    }
    parsed.truncate(UPSERT_BATCH);
    bencher
        .counter(ItemsCount::new(parsed.len()))
        .with_inputs(|| {
            let (store, _) = fresh_store();
            let machine = tokenmtr_store::StoreRead::machine_id(&store);
            let events: Vec<_> = parsed
                .iter()
                .cloned()
                .map(|event| event.into_usage_event(&machine, None))
                .collect();
            (store, events)
        })
        .bench_local_values(|(mut store, events)| {
            store.upsert_events(&events).expect("upsert");
        });
}

fn main() {
    divan::main();
    // Best effort: temp files of the run.
    let _ = fs::remove_dir_all(bench_dir());
}
