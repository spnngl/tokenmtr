//! The harness: a sandboxed `HOME`, the real `tokenmtr` binary, a mock server for the three
//! remote services and helpers to read the sinks.
//!
//! Nothing here reads the real home, the real Keychain or the real network: the child process has
//! a cleared environment that only names the sandbox, and `TOKENMTR_TEST_ENDPOINT_BASE` points it
//! at the mock server (which also makes it skip the Keychain).
#![allow(dead_code, reason = "each test binary uses a subset of the helpers")]

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use camino::Utf8PathBuf;
use serde_json::Value;
use tempfile::TempDir;
use tokenmtr_platform::Paths;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

pub type TestResult = Result<(), Box<dyn Error>>;

/// A file's path and its bytes.
pub type FileBytes = (PathBuf, Vec<u8>);

/// The access token in the sandbox's credentials file. It must appear nowhere else.
pub const FAKE_ACCESS_TOKEN: &str = "sk-ant-oat01-FAKE-E2E-TOKEN";
/// The refresh token next to it; same rule.
pub const FAKE_REFRESH_TOKEN: &str = "sk-ant-ort01-FAKE-E2E-REFRESH";

pub const USAGE_PATH: &str = "/api/oauth/usage";
pub const PROFILE_PATH: &str = "/api/oauth/profile";
pub const ECB_DAILY_PATH: &str = "/stats/eurofxref/eurofxref-daily.xml";
pub const ECB_HISTORY_PATH: &str = "/stats/eurofxref/eurofxref-hist.xml";
pub const LITELLM_PATH: &str = "/BerriAI/litellm/main/model_prices_and_context_window.json";

/// A daemon that has not stopped within this is killed and the test fails.
const STOP_LIMIT: Duration = Duration::from_secs(10);

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// Polls `check` every 50 ms until it returns `Some` or `limit` passes.
pub fn eventually<T>(
    limit: Duration,
    what: &str,
    mut check: impl FnMut() -> Result<Option<T>, Box<dyn Error>>,
) -> Result<T, Box<dyn Error>> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(value) = check()? {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err(format!("timed out after {limit:?} waiting for {what}").into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The `tokenmtr` binary, built once per test process with `cargo build -p tokenmtr`.
///
/// `cargo test -p tokenmtr-e2e` does not build the binary of another package, and the e2e crate
/// cannot depend on a binary crate. The test executable sits in `target/<profile>/deps`, which
/// tells which target directory and profile to build into.
fn binary() -> Result<&'static Path, String> {
    static BINARY: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    BINARY
        .get_or_init(|| {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let profile_dir = exe
                .parent()
                .and_then(Path::parent)
                .ok_or("unexpected test executable location")?;
            let target_dir = profile_dir
                .parent()
                .ok_or("unexpected test executable location")?;
            let profile = profile_dir
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("unexpected profile directory")?;
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let mut build = Command::new(cargo);
            build
                .args(["build", "--quiet", "-p", "tokenmtr", "--target-dir"])
                .arg(target_dir);
            if profile != "debug" {
                build.args(["--profile", profile]);
            }
            let status = build.status().map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("cargo build -p tokenmtr failed: {status}"));
            }
            Ok(profile_dir.join("tokenmtr"))
        })
        .as_ref()
        .map(PathBuf::as_path)
        .map_err(Clone::clone)
}

/// A throwaway home with the agents' files and a credentials file, and the means to run `tokenmtr`
/// against it.
pub struct Sandbox {
    _dir: TempDir,
    pub root: Utf8PathBuf,
    pub paths: Paths,
    /// Everything `tokenmtr` printed, for the credential-leak scan.
    transcript: RefCell<Vec<Vec<u8>>>,
}

impl Sandbox {
    /// `/tmp` keeps the control socket path under the 104-byte `sun_path` limit.
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let dir = tempfile::Builder::new().prefix("tm").tempdir_in("/tmp")?;
        let root = Utf8PathBuf::from_path_buf(dir.path().canonicalize()?)
            .map_err(|p| format!("not utf-8: {}", p.display()))?;
        let home = root.to_string();
        let paths = Paths::from_lookup(move |key| (key == "HOME").then(|| home.clone()))?;
        let sandbox = Self {
            _dir: dir,
            root,
            paths,
            transcript: RefCell::new(Vec::new()),
        };
        sandbox.seed()?;
        Ok(sandbox)
    }

    /// Fixture Claude and pi files, and the fake OAuth credentials.
    fn seed(&self) -> Result<(), Box<dyn Error>> {
        let fixtures = fixtures();
        copy_dir(&fixtures.join("claude/projects"), &self.claude_projects())?;
        copy_dir(&fixtures.join("pi/sessions"), &self.pi_sessions())?;
        let credentials = fs::read_to_string(fixtures.join("http/claude_credentials.json"))?
            .replace(
                "sk-ant-oat01-FAKE-TOKEN-0000000000000000",
                FAKE_ACCESS_TOKEN,
            )
            .replace(
                "sk-ant-ort01-FAKE-REFRESH-0000000000000000",
                FAKE_REFRESH_TOKEN,
            );
        fs::write(self.claude_dir().join(".credentials.json"), credentials)?;
        Ok(())
    }

    pub fn claude_dir(&self) -> PathBuf {
        self.root.join(".claude").into_std_path_buf()
    }

    pub fn claude_projects(&self) -> PathBuf {
        self.claude_dir().join("projects")
    }

    pub fn pi_sessions(&self) -> PathBuf {
        self.root.join("pi/sessions").into_std_path_buf()
    }

    pub fn socket(&self) -> PathBuf {
        self.paths
            .data_dir()
            .join("control.sock")
            .into_std_path_buf()
    }

    pub fn out_dir(&self) -> Utf8PathBuf {
        self.root.join("out")
    }

    pub fn csv_dir(&self) -> Utf8PathBuf {
        self.out_dir().join("csv")
    }

    pub fn sqlite_path(&self) -> Utf8PathBuf {
        self.out_dir().join("export.db")
    }

    pub fn write_config(&self, text: &str) -> Result<(), Box<dyn Error>> {
        let path = self.paths.config_file();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, text)?;
        Ok(())
    }

    /// The config of both scenarios: a CSV and a SQLite sink on `usage_events`, fast intervals and
    /// the most verbose logging, so a leak has the best chance to show.
    pub fn config(&self, networked: bool) -> String {
        format!(
            "[daemon]\nlog_level = \"trace\"\n\n\
             [quota]\nenabled = {networked}\n\n\
             [fx]\nenabled = {networked}\n\n\
             [pricing]\nauto_update = {networked}\n\n\
             [[sinks]]\nid = \"csv\"\nkind = \"csv\"\ndir = \"{csv}\"\npartition = \"all\"\n\
             datasets = [\"usage_events\"]\ninterval = \"1s\"\n\n\
             [[sinks]]\nid = \"sqlite\"\nkind = \"sqlite\"\npath = \"{sqlite}\"\n\
             datasets = [\"usage_events\"]\ninterval = \"1s\"\n",
            csv = self.csv_dir(),
            sqlite = self.sqlite_path(),
        )
    }

    /// `tokenmtr` with an environment that only knows this sandbox.
    fn command(&self, endpoint_base: &str) -> Result<Command, Box<dyn Error>> {
        let mut command = Command::new(binary()?);
        command
            .env_clear()
            .env("HOME", self.root.as_str())
            .env("XDG_CONFIG_HOME", self.root.join(".config"))
            .env("XDG_DATA_HOME", self.root.join(".local/share"))
            .env("XDG_STATE_HOME", self.root.join(".local/state"))
            .env("XDG_CACHE_HOME", self.root.join(".cache"))
            .env("CLAUDE_CONFIG_DIR", self.claude_dir())
            .env("PI_CODING_AGENT_DIR", self.root.join("pi"))
            .env("TZ", "UTC")
            .env("TOKENMTR_TEST_ENDPOINT_BASE", endpoint_base);
        Ok(command)
    }

    /// Runs `tokenmtr args` to completion and remembers its output for [`Sandbox::transcript`].
    pub fn cli(&self, endpoint_base: &str, args: &[&str]) -> Result<Output, Box<dyn Error>> {
        let output = self.command(endpoint_base)?.args(args).output()?;
        let mut transcript = self.transcript.borrow_mut();
        transcript.push(output.stdout.clone());
        transcript.push(output.stderr.clone());
        Ok(output)
    }

    /// Like [`Sandbox::cli`], failing unless the command exits 0, and parsing stdout as JSON.
    pub fn cli_json(&self, endpoint_base: &str, args: &[&str]) -> Result<Value, Box<dyn Error>> {
        let output = self.cli(endpoint_base, args)?;
        if !output.status.success() {
            return Err(format!(
                "tokenmtr {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    /// Starts `tokenmtr run` and waits until its control socket answers.
    pub fn start_daemon(&self, endpoint_base: &str) -> Result<Daemon, Box<dyn Error>> {
        let captured = self.root.join("captured");
        fs::create_dir_all(&captured)?;
        let child = self
            .command(endpoint_base)?
            .arg("run")
            .stdin(Stdio::null())
            .stdout(File::create(captured.join("daemon.stdout"))?)
            .stderr(File::create(captured.join("daemon.stderr"))?)
            .spawn()?;
        let mut daemon = Daemon { child: Some(child) };
        let socket = self.socket();
        eventually(Duration::from_secs(20), "the control socket", || {
            if let Some(child) = daemon.child.as_mut()
                && let Some(status) = child.try_wait()?
            {
                return Err(format!("the daemon exited early: {status}").into());
            }
            Ok(UnixStream::connect(&socket).ok().map(|_| ()))
        })?;
        Ok(daemon)
    }

    /// Sends the given request lines after a hello and returns every raw answer line.
    pub fn control(&self, requests: &[&str]) -> Result<Vec<String>, Box<dyn Error>> {
        let mut stream = UnixStream::connect(self.socket())?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let hello =
            r#"{"type":"hello","protocol":4,"client":{"name":"e2e","version":"0","schema":null}}"#;
        let mut lines = Vec::new();
        for request in std::iter::once(&hello).chain(requests) {
            writeln!(stream, "{request}")?;
            let mut line = String::new();
            reader.read_line(&mut line)?;
            lines.push(line.trim_end().to_owned());
        }
        let mut transcript = self.transcript.borrow_mut();
        transcript.extend(lines.iter().map(|l| l.clone().into_bytes()));
        Ok(lines)
    }

    /// Everything `tokenmtr` printed and every control answer so far.
    pub fn transcript(&self) -> Vec<u8> {
        self.transcript.borrow().concat()
    }

    /// The `event_id` column of every CSV file of the CSV sink.
    pub fn csv_event_ids(&self) -> Result<BTreeSet<String>, Box<dyn Error>> {
        let mut ids = BTreeSet::new();
        let Ok(entries) = fs::read_dir(self.csv_dir()) else {
            return Ok(ids);
        };
        for entry in entries.flatten() {
            let Ok(mut reader) = csv::Reader::from_path(entry.path()) else {
                continue;
            };
            let headers = reader.headers()?.clone();
            let Some(column) = headers.iter().position(|h| h == "event_id") else {
                continue;
            };
            // A file being written may end in a partial line: ignore what does not parse.
            for record in reader.records().flatten() {
                if let Some(id) = record.get(column) {
                    ids.insert(id.to_owned());
                }
            }
        }
        Ok(ids)
    }

    /// The `event_id` column of the SQLite sink's `usage_events` table.
    pub fn sqlite_event_ids(&self) -> Result<BTreeSet<String>, Box<dyn Error>> {
        let path = self.sqlite_path();
        if !path.is_file() {
            return Ok(BTreeSet::new());
        }
        let connection = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let Ok(mut statement) = connection.prepare("SELECT event_id FROM usage_events") else {
            return Ok(BTreeSet::new());
        };
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<BTreeSet<_>, _>>()?;
        Ok(ids)
    }

    /// Every file under the sandbox except `except`, as `(path, bytes)`.
    pub fn all_files(&self, except: &[PathBuf]) -> Result<Vec<FileBytes>, Box<dyn Error>> {
        let mut files = Vec::new();
        let mut pending = vec![self.root.clone().into_std_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    pending.push(path);
                } else if kind.is_file() && !except.contains(&path) {
                    // Sockets and files that vanish mid-walk (WAL, temp files) are not data.
                    if let Ok(bytes) = fs::read(&path) {
                        files.push((path, bytes));
                    }
                }
            }
        }
        Ok(files)
    }
}

/// A running `tokenmtr run`. Killed on drop, so a failing test leaves no daemon behind.
pub struct Daemon {
    child: Option<Child>,
}

impl Daemon {
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Sends SIGTERM and waits for the exit; returns the exit code and how long it took.
    pub fn terminate(mut self) -> Result<(Option<i32>, Duration), Box<dyn Error>> {
        let mut child = self.child.take().ok_or("already stopped")?;
        let started = Instant::now();
        let sent = Command::new("/bin/kill")
            .args(["-TERM", &child.id().to_string()])
            .status()?;
        if !sent.success() {
            child.kill().ok();
            return Err("kill -TERM failed".into());
        }
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if started.elapsed() >= STOP_LIMIT {
                child.kill().ok();
                return Err(format!("the daemon ignored SIGTERM for {STOP_LIMIT:?}").into());
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        Ok((status.code(), started.elapsed()))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            child.kill().ok();
            child.wait().ok();
        }
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// A mock server for Anthropic, the ECB and `LiteLLM`. With `expect_each` every endpoint must be
/// hit exactly once ([`Mock::expect`], checked by [`MockServer::verify`]); without it any request
/// fails the test.
pub async fn mock_servers(expect_each: bool) -> Result<MockServer, Box<dyn Error>> {
    let server = MockServer::start().await;
    if !expect_each {
        return Ok(server);
    }
    let http = fixtures().join("http");
    let bearer = format!("Bearer {FAKE_ACCESS_TOKEN}");
    let routes: [(&str, &str, Option<&str>); 4] = [
        (USAGE_PATH, "claude_usage.json", Some(bearer.as_str())),
        (PROFILE_PATH, "claude_profile.json", Some(bearer.as_str())),
        (ECB_HISTORY_PATH, "ecb_hist.xml", None),
        (LITELLM_PATH, "litellm_small.json", None),
    ];
    for (route, file, auth) in routes {
        let body = fs::read(http.join(file))?;
        let mut template = ResponseTemplate::new(200).set_body_bytes(body);
        if route == LITELLM_PATH {
            template = template.insert_header("etag", "\"e2e-1\"");
        }
        let mock = Mock::given(method("GET")).and(path(route));
        match auth {
            // The credentials must actually be sent, as a bearer token and nowhere else.
            Some(value) => mock.and(header("authorization", value)),
            None => mock,
        }
        .respond_with(template)
        .expect(1)
        .mount(&server)
        .await;
    }
    Ok(server)
}

/// Neither token appears in any file of the sandbox (logs, store, exports, backups, captured
/// stdio) except the credentials file itself, nor in `extra` (a decompressed backup, say), nor in
/// what `tokenmtr` printed.
pub fn assert_no_secret(sandbox: &Sandbox, extra: &[(String, Vec<u8>)]) -> TestResult {
    let credentials = sandbox.claude_dir().join(".credentials.json");
    let mut files = sandbox.all_files(std::slice::from_ref(&credentials))?;
    let logged = files
        .iter()
        .any(|(path, bytes)| path.starts_with(sandbox.paths.logs_dir()) && !bytes.is_empty());
    if !logged {
        return Err("sanity: the daemon wrote no log, the scan would prove nothing".into());
    }
    let secret_file = fs::read(&credentials)?;
    if !contains(&secret_file, FAKE_ACCESS_TOKEN.as_bytes()) {
        return Err("sanity: the credentials file lost its token".into());
    }
    files.push((PathBuf::from("<transcript>"), sandbox.transcript()));
    files.extend(
        extra
            .iter()
            .map(|(name, bytes)| (PathBuf::from(name), bytes.clone())),
    );
    let leaks: Vec<String> = files
        .iter()
        .filter(|(_, bytes)| {
            [FAKE_ACCESS_TOKEN, FAKE_REFRESH_TOKEN, "FAKE-E2E"]
                .iter()
                .any(|secret| contains(bytes, secret.as_bytes()))
        })
        .map(|(path, _)| path.display().to_string())
        .collect();
    if leaks.is_empty() {
        Ok(())
    } else {
        Err(format!("a credential leaked into {leaks:?}").into())
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
