//! A daemon on temporary directories, fixture agent logs and no real network.
#![allow(dead_code, reason = "each test binary uses a subset")]

use std::error::Error;
use std::future::Future;
use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use camino::{Utf8Path, Utf8PathBuf};
use secrecy::SecretString;
use tokenmtr_agents::ingest::watch::Debounce;
use tokenmtr_control::{Client, ClientInfo};
use tokenmtr_daemon::{Daemon, Settings, Timing};
use tokenmtr_platform::{ControlTransport, Paths, SecretError, SecretStore};
use tokenmtr_remote::Endpoints;
use tokenmtr_store::{Dataset, Revision, StoreRead as _, StoreReader};

pub type TestResult = Result<(), Box<dyn Error>>;

/// No Keychain in tests: credentials come from files.
pub struct NoSecrets;

impl SecretStore for NoSecrets {
    fn get(&self, _service: &str) -> Result<Option<SecretString>, SecretError> {
        Ok(None)
    }
}

/// Timings short enough for tests; production values stay the defaults.
pub fn fast_timing() -> Timing {
    Timing {
        watch: Debounce {
            quiet: Duration::from_millis(100),
            max_wait: Duration::from_millis(500),
        },
        root_poll: Duration::from_millis(100),
        event_coalesce: Duration::from_millis(300),
        reload_debounce: Duration::from_millis(100),
        min_slack: Duration::from_millis(50),
        ..Timing::default()
    }
}

/// A throwaway `HOME` with one Claude and one pi profile inside it.
pub struct Env {
    _dir: tempfile::TempDir,
    pub home: Utf8PathBuf,
    pub paths: Paths,
    pub config_path: Utf8PathBuf,
}

impl Env {
    /// `/tmp` keeps the control socket path under the 104-byte `sun_path` limit.
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let dir = tempfile::Builder::new().prefix("tm").tempdir_in("/tmp")?;
        let home = Utf8PathBuf::from_path_buf(dir.path().canonicalize()?)
            .map_err(|p| format!("not utf-8: {}", p.display()))?;
        let lookup_home = home.clone();
        let paths =
            Paths::from_lookup(move |key| (key == "HOME").then(|| lookup_home.to_string()))?;
        std::fs::create_dir_all(home.join("claude/projects/-tmp-app"))?;
        std::fs::create_dir_all(home.join("pi/--tmp-app--"))?;
        let config_path = home.join("config.toml");
        let env = Self {
            _dir: dir,
            home,
            paths,
            config_path,
        };
        env.write_config("")?;
        Ok(env)
    }

    pub fn store_path(&self) -> Utf8PathBuf {
        self.paths.data_dir().join("usage.db")
    }

    pub fn claude_log(&self) -> Utf8PathBuf {
        self.home.join("claude/projects/-tmp-app/s1.jsonl")
    }

    pub fn pi_log(&self) -> Utf8PathBuf {
        self.home
            .join("pi/--tmp-app--/2026-03-02T09-00-00-000Z_sess-1.jsonl")
    }

    /// The test config: both agents on the temp dirs and every networked or slow task off,
    /// followed by `extra` (more sections or keys of the tables opened last: avoid, add whole
    /// tables).
    pub fn config_text(&self, extra: &str) -> String {
        let home = &self.home;
        format!(
            "[daemon]\nscan_interval = \"2s\"\n\
             [quota]\nenabled = false\n\
             [fx]\nenabled = false\n\
             [pricing]\nauto_update = false\n\
             [backup]\nenabled = false\n\
             [[agents.claude.profiles]]\nid = \"default\"\nconfig_dir = \"{home}/claude\"\n\
             [[agents.pi.profiles]]\nid = \"default\"\nroots = [\"{home}/pi\"]\n\
             {extra}\n"
        )
    }

    /// The test config with the named sections (`quota`, `fx`, `pricing`, `backup`) switched on.
    pub fn write_config_with(&self, on: &[&str], extra: &str) -> Result<(), Box<dyn Error>> {
        let mut text = self.config_text(extra);
        for section in on {
            text = match *section {
                "pricing" => text.replace("auto_update = false", "auto_update = true"),
                other => text.replace(
                    &format!("[{other}]\nenabled = false"),
                    &format!("[{other}]\nenabled = true"),
                ),
            };
        }
        std::fs::write(&self.config_path, text)?;
        Ok(())
    }

    pub fn write_config(&self, extra: &str) -> Result<(), Box<dyn Error>> {
        std::fs::write(&self.config_path, self.config_text(extra))?;
        Ok(())
    }

    pub fn transport(&self) -> ControlTransport {
        ControlTransport::unix(self.paths.data_dir().join("control.sock"))
    }

    /// Settings that can never reach the real network: endpoints default to a closed port.
    pub fn settings(&self, timing: Timing) -> Settings {
        let mut settings = Settings::new(
            self.paths.clone(),
            self.config_path.clone(),
            self.transport(),
            Arc::new(NoSecrets),
        );
        settings.timing = timing;
        settings.endpoints = Endpoints::at("http://127.0.0.1:9").ok();
        settings
    }

    pub async fn start(&self, timing: Timing) -> Result<Daemon, Box<dyn Error>> {
        Ok(Daemon::start(self.settings(timing)).await?)
    }

    pub async fn client(&self) -> Result<Client, Box<dyn Error>> {
        Ok(Client::connect(
            &self.transport(),
            ClientInfo {
                name: "test".to_owned(),
                version: "0".to_owned(),
                schema: None,
            },
        )
        .await?)
    }

    pub fn reader(&self) -> Result<StoreReader, Box<dyn Error>> {
        Ok(StoreReader::open(&self.store_path())?)
    }

    pub fn usage_events(&self) -> usize {
        self.reader()
            .and_then(|r| Ok(r.usage_events_after(Revision::ZERO, 100_000)?.len()))
            .unwrap_or(0)
    }

    pub fn head(&self, dataset: Dataset) -> Revision {
        self.reader()
            .and_then(|r| Ok(r.head_revision(dataset)?))
            .unwrap_or(Revision::ZERO)
    }
}

/// One Claude assistant record; `n` makes ids and the request unique.
pub fn claude_line(n: u32) -> String {
    claude_line_for(n, "claude-sonnet-4-5")
}

/// Like [`claude_line`] for another model.
pub fn claude_line_for(n: u32, model: &str) -> String {
    format!(
        "{{\"parentUuid\":null,\"isSidechain\":false,\"userType\":\"external\",\"cwd\":\"/tmp/app\",\
         \"sessionId\":\"s1\",\"version\":\"2.1.0\",\"gitBranch\":\"main\",\"type\":\"assistant\",\
         \"message\":{{\"id\":\"msg_{n}\",\"type\":\"message\",\"role\":\"assistant\",\
         \"model\":\"{model}\",\"content\":[{{\"type\":\"text\",\"text\":\"hi\"}}],\
         \"stop_reason\":\"end_turn\",\"usage\":{{\"input_tokens\":10,\"cache_creation_input_tokens\":20,\
         \"cache_read_input_tokens\":30,\"output_tokens\":40}}}},\"requestId\":\"req_{n}\",\
         \"uuid\":\"u{n}\",\"timestamp\":\"2026-03-05T11:00:{:02}.000Z\"}}\n",
        n % 60
    )
}

/// The header and first assistant message of a pi session.
pub fn pi_session() -> String {
    "{\"type\":\"session\",\"version\":3,\"id\":\"sess-1\",\"timestamp\":\"2026-03-02T09:00:00.000Z\",\"cwd\":\"/tmp/app\"}\n\
     {\"type\":\"message\",\"id\":\"p0000001\",\"parentId\":null,\"timestamp\":\"2026-03-02T09:00:10.000Z\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Done.\"}],\"api\":\"openai-responses\",\"provider\":\"openai\",\"model\":\"gpt-5\",\"usage\":{\"input\":200,\"output\":200,\"cacheRead\":1000,\"cacheWrite\":0,\"reasoning\":64,\"totalTokens\":1400,\"cost\":{\"input\":0.0003,\"output\":0.002,\"cacheRead\":0.0001,\"cacheWrite\":0,\"total\":0.0024}},\"stopReason\":\"stop\",\"timestamp\":1772442010000}}\n"
        .to_owned()
}

/// Writes one Claude and one pi event.
pub fn seed_logs(env: &Env) -> TestResult {
    std::fs::write(env.claude_log(), claude_line(1))?;
    std::fs::write(env.pi_log(), pi_session())?;
    Ok(())
}

pub fn append(path: &Utf8Path, text: &str) -> TestResult {
    let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// Polls `check` every 50 ms until it returns true or `limit` passes.
pub async fn eventually<F: FnMut() -> bool>(limit: Duration, mut check: F) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if check() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Like [`eventually`] for an async condition.
pub async fn eventually_async<F, Fut>(limit: Duration, mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if check().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
