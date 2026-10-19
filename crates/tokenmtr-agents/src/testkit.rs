//! Test helpers shared by the unit tests: temp git repos, config rules, JSONL transcripts.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use camino::{Utf8Path, Utf8PathBuf};
use tokenmtr_config::{ConfigFormat, ProjectRule, load_str};
use tokenmtr_core::{
    AgentId, DedupKey, EventKind, ModelId, ParsedEvent, ProfileId, ProviderId, SessionId,
    TokenCounts,
};

/// UTF-8 path of a temp dir, canonicalized so macOS `/var` → `/private/var` does not leak into
/// assertions.
pub fn utf8_dir(dir: &tempfile::TempDir) -> Utf8PathBuf {
    let canonical = dir.path().canonicalize().expect("canonical temp dir");
    Utf8PathBuf::from_path_buf(canonical).expect("utf-8 temp dir")
}

/// `[[projects]]` rules parsed from `toml` through the real config loader.
pub fn rules(toml: &str) -> Vec<ProjectRule> {
    let format = ConfigFormat::from_path(Utf8Path::new("config.toml")).expect("toml");
    load_str(toml, format, &|_: &str| None::<String>)
        .expect("valid config")
        .projects
}

/// A Claude message event of session `s1` with the given tokens; `at` is RFC 3339.
pub fn parsed_event(key: &str, at: &str, input: u64, output: u64) -> ParsedEvent {
    ParsedEvent {
        agent: AgentId::CLAUDE,
        profile: ProfileId::default_profile(),
        dedup_key: DedupKey::try_from(key).expect("key"),
        at: at.parse().expect("timestamp"),
        session: SessionId::try_from("s1").expect("session"),
        parent_session: None,
        subagent: None,
        is_sidechain: false,
        cwd: None,
        git_branch: None,
        source_file: None,
        approximate: false,
        provider: ProviderId::try_from("anthropic").ok(),
        model: ModelId::try_from("claude-sonnet-4-5").expect("model"),
        kind: EventKind::Message,
        tokens: TokenCounts {
            input_uncached: input,
            output,
            ..TokenCounts::default()
        },
        reported_cost: None,
        service_tier: None,
        modifiers: None,
    }
}

/// Initializes a git repository at `dir`, with `remote` as `origin` when given.
pub fn init_repo(dir: &Path, remote: Option<&str>) {
    fs::create_dir_all(dir).expect("create repo dir");
    gix::init(dir).expect("git init");
    if let Some(url) = remote {
        let mut config = fs::OpenOptions::new()
            .append(true)
            .open(dir.join(".git").join("config"))
            .expect("open git config");
        writeln!(config, "[remote \"origin\"]\n\turl = {url}").expect("write remote");
    }
}

/// Adds remote `name` with `url` to the repository at `dir`; with `head`, records the remote's
/// default branch (`refs/remotes/<name>/HEAD`) the way `git clone` does.
pub fn add_remote(dir: &Path, name: &str, url: &str, head: Option<&str>) {
    let mut config = fs::OpenOptions::new()
        .append(true)
        .open(dir.join(".git").join("config"))
        .expect("open git config");
    writeln!(config, "[remote \"{name}\"]\n\turl = {url}").expect("write remote");
    if let Some(branch) = head {
        let refs = dir.join(".git").join("refs").join("remotes").join(name);
        fs::create_dir_all(&refs).expect("create remote refs");
        fs::write(
            refs.join("HEAD"),
            format!("ref: refs/remotes/{name}/{branch}\n"),
        )
        .expect("remote HEAD");
    }
}

/// Lays out a linked worktree of `main` at `worktree`, the way `git worktree add` does.
pub fn add_worktree(main: &Path, worktree: &Path, name: &str) {
    let admin = main.join(".git").join("worktrees").join(name);
    fs::create_dir_all(&admin).expect("create worktree admin dir");
    fs::create_dir_all(worktree).expect("create worktree dir");
    fs::write(admin.join("HEAD"), format!("ref: refs/heads/{name}\n")).expect("HEAD");
    fs::write(admin.join("commondir"), "../..\n").expect("commondir");
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", worktree.join(".git").display()),
    )
    .expect("gitdir");
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", admin.display()),
    )
    .expect("worktree .git file");
}
