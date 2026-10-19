use std::fs;

use super::*;
use crate::testkit::{add_worktree, init_repo, rules, utf8_dir};

fn at() -> Timestamp {
    Timestamp::UNIX_EPOCH
}

fn resolve(resolver: &ProjectResolver, cwd: &Utf8Path) -> Resolved {
    resolver
        .resolve(cwd, at(), &mut Hints::off())
        .expect("non-blank cwd")
}

#[test]
fn remotes_normalize_to_one_key() {
    let keys = [
        "https://github.com/Owner/repo.git",
        "git@github.com:Owner/repo.git",
        "ssh://git@github.com:22/Owner/repo",
        "https://user:token@GitHub.com/Owner/repo/",
    ]
    .map(normalize_remote);
    for key in &keys[1..] {
        assert_eq!(key.as_deref(), Some("github.com/Owner/repo"));
    }
    assert_eq!(keys[0].as_deref(), Some("github.com/Owner/repo"));
}

#[test]
fn local_remotes_identify_nothing() {
    assert_eq!(normalize_remote("/srv/git/repo.git"), None);
    assert_eq!(normalize_remote("../repo"), None);
    assert_eq!(normalize_remote("file:///srv/git/repo.git"), None);
    assert_eq!(normalize_remote(""), None);
}

#[test]
fn ssh_and_https_clones_are_one_project() {
    let dir = tempfile::tempdir().unwrap();
    let root = utf8_dir(&dir);
    init_repo(
        root.join("a").as_std_path(),
        Some("git@github.com:acme/tool.git"),
    );
    init_repo(
        root.join("b").as_std_path(),
        Some("https://github.com/acme/tool"),
    );
    let resolver = ProjectResolver::default();
    let a = resolve(&resolver, &root.join("a"));
    let b = resolve(&resolver, &root.join("b"));
    assert_eq!(a.resolution.project, b.resolution.project);
    assert_eq!(a.resolution.project.as_str(), "github.com/acme/tool");
    assert_eq!(a.resolution.name, "tool");
    assert_eq!(a.resolution.source, ProjectSource::Remote);
    assert_eq!(a.resolution.remote.as_deref(), Some("github.com/acme/tool"));
    assert_eq!(a.problem, None);
}

#[test]
fn main_checkout_and_worktrees_are_one_project() {
    let dir = tempfile::tempdir().unwrap();
    let root = utf8_dir(&dir);
    let main = root.join("main");
    init_repo(main.as_std_path(), None);
    add_worktree(main.as_std_path(), root.join("wt1").as_std_path(), "wt1");
    add_worktree(main.as_std_path(), root.join("wt2").as_std_path(), "wt2");
    fs::create_dir_all(main.join("src/deep")).unwrap();
    let resolver = ProjectResolver::default();
    let projects: Vec<_> = [
        main.clone(),
        main.join("src/deep"),
        root.join("wt1"),
        root.join("wt2"),
    ]
    .iter()
    .map(|cwd| resolve(&resolver, cwd).resolution)
    .collect();
    for project in &projects {
        assert_eq!(project.project.as_str(), main.as_str());
        assert_eq!(project.source, ProjectSource::RepoRoot);
        assert_eq!(project.repo_root.as_deref(), Some(main.as_path()));
        assert_eq!(project.name, "main");
    }
}

#[test]
fn worktrees_of_a_remote_repo_share_its_remote_key() {
    let dir = tempfile::tempdir().unwrap();
    let root = utf8_dir(&dir);
    let main = root.join("main");
    init_repo(main.as_std_path(), Some("git@github.com:acme/tool.git"));
    add_worktree(main.as_std_path(), root.join("wt").as_std_path(), "wt");
    let resolved = resolve(&ProjectResolver::default(), &root.join("wt")).resolution;
    assert_eq!(resolved.project.as_str(), "github.com/acme/tool");
}

#[test]
fn a_plain_directory_resolves_to_itself() {
    let dir = tempfile::tempdir().unwrap();
    let root = utf8_dir(&dir);
    let resolved = resolve(&ProjectResolver::default(), &root);
    assert_eq!(resolved.resolution.source, ProjectSource::Cwd);
    assert_eq!(resolved.resolution.project.as_str(), root.as_str());
    assert_eq!(resolved.resolution.repo_root, None);
    assert_eq!(resolved.problem, None);
}

#[test]
fn a_deleted_cwd_falls_back_and_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let gone = utf8_dir(&dir).join("deleted");
    let resolved = resolve(&ProjectResolver::default(), &gone);
    assert_eq!(resolved.resolution.source, ProjectSource::Cwd);
    assert_eq!(resolved.resolution.project.as_str(), gone.as_str());
    assert_eq!(resolved.problem, Some(CwdProblem::Missing));
}

#[test]
fn a_rule_beats_git_and_needs_no_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let root = utf8_dir(&dir);
    init_repo(
        root.join("repo").as_std_path(),
        Some("git@github.com:acme/tool.git"),
    );
    let toml = format!(
        "[[projects]]\nmatch = \"{root}/repo\"\nname = \"ruled\"\nclient = \"acme\"\n\n\
         [[projects]]\nmatch = \"{root}/**\"\nname = \"later\"\n"
    );
    let resolver = ProjectResolver::new(rules(&toml), Vec::new());
    let found = resolve(&resolver, &root.join("repo"));
    assert_eq!(found.resolution.project.as_str(), "ruled");
    assert_eq!(found.resolution.client.as_deref(), Some("acme"));
    assert!(matches!(found.resolution.source, ProjectSource::Rule(_)));
    let missing = resolve(&resolver, &root.join("nowhere"));
    assert_eq!(missing.resolution.project.as_str(), "later");
    assert_eq!(missing.problem, None, "a rule match never touches the disk");
}

#[test]
fn a_cwd_under_a_protected_folder_is_never_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = utf8_dir(&dir);
    init_repo(
        root.join("Documents/repo").as_std_path(),
        Some("git@github.com:acme/tool.git"),
    );
    let resolver = ProjectResolver::new(Vec::new(), vec![root.join("documents")]);
    let found = resolve(&resolver, &root.join("Documents/repo"));
    assert_eq!(found.problem, Some(CwdProblem::Protected));
    assert_eq!(
        found.resolution.source,
        ProjectSource::Cwd,
        "git was not read"
    );
}

#[test]
fn a_file_as_cwd_is_unreadable_not_missing() {
    let dir = tempfile::tempdir().unwrap();
    let file = utf8_dir(&dir).join("file");
    fs::write(&file, "").unwrap();
    let resolved = resolve(&ProjectResolver::default(), &file);
    assert_eq!(resolved.problem, Some(CwdProblem::Unreadable));
}

#[test]
fn blank_cwds_resolve_to_nothing() {
    assert_eq!(
        ProjectResolver::default().resolve(Utf8Path::new(" "), at(), &mut Hints::off()),
        None
    );
}

#[cfg(unix)]
#[test]
fn a_protected_cwd_falls_back_and_is_reported() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let root = utf8_dir(&dir);
    let locked = root.join("locked");
    init_repo(locked.as_std_path(), Some("git@github.com:acme/tool.git"));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(&locked).is_ok();
    let resolved = resolve(&ProjectResolver::default(), &locked);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return; // running as root: permissions do not apply
    }
    assert_eq!(resolved.problem, Some(CwdProblem::Protected));
    assert_eq!(resolved.resolution.source, ProjectSource::Cwd);
    assert_eq!(resolved.resolution.project.as_str(), locked.as_str());
}

fn superset_hints(root: &Utf8Path, worktree: &Utf8Path, repo: &Utf8Path) -> Hints {
    let dir = root.join("superset");
    superset::fixture::fake_superset(&dir, worktree, repo);
    Hints::superset(dir)
}

#[test]
fn a_deleted_worktree_is_inferred_from_its_live_main_repository() {
    let dir = tempfile::tempdir().unwrap();
    let root = utf8_dir(&dir);
    let main = root.join("main");
    init_repo(main.as_std_path(), Some("git@github.com:acme/tool.git"));
    let gone = root.join("worktrees/brave-otter");
    let mut hints = superset_hints(&root, &gone, &main);
    let resolved = ProjectResolver::default()
        .resolve(&gone.join("crates"), at(), &mut hints)
        .unwrap();
    assert_eq!(resolved.problem, None, "inferred: nothing to report");
    let resolution = resolved.resolution;
    assert_eq!(resolution.source, ProjectSource::Inferred);
    assert_eq!(
        resolution.project.as_str(),
        "github.com/acme/tool",
        "the live repository's origin, as its live worktrees resolve"
    );
    assert_eq!(resolution.name, "tool");
    assert_eq!(resolution.repo_root.as_deref(), Some(main.as_path()));
}

#[test]
fn a_deleted_worktree_of_a_deleted_repository_takes_the_recorded_remote() {
    let dir = tempfile::tempdir().unwrap();
    let root = utf8_dir(&dir);
    let gone = root.join("worktrees/brave-otter");
    let mut hints = superset_hints(&root, &gone, &root.join("gone-main"));
    let resolution = ProjectResolver::default()
        .resolve(&gone, at(), &mut hints)
        .unwrap()
        .resolution;
    assert_eq!(resolution.source, ProjectSource::Inferred);
    assert_eq!(resolution.project.as_str(), "github.com/acme/widgets");
    assert_eq!(resolution.name, "widgets");
}

#[test]
fn a_live_folder_outside_git_is_inferred_and_git_still_wins() {
    let dir = tempfile::tempdir().unwrap();
    let root = utf8_dir(&dir);
    let plain = root.join("sessions/s1");
    fs::create_dir_all(&plain).unwrap();
    let mut hints = superset_hints(&root, &plain, &root.join("gone-main"));
    let resolver = ProjectResolver::default();
    let inferred = resolver.resolve(&plain, at(), &mut hints).unwrap();
    assert_eq!(inferred.resolution.source, ProjectSource::Inferred);

    let repo = root.join("repo");
    init_repo(repo.as_std_path(), None);
    let mut hints = superset_hints(&root.join("other"), &repo, &root.join("gone-main"));
    let git = resolver.resolve(&repo, at(), &mut hints).unwrap();
    assert_eq!(git.resolution.source, ProjectSource::RepoRoot);
}

#[test]
fn head_branch_reads_the_checked_out_branch() {
    let dir = tempfile::tempdir().unwrap();
    let root = utf8_dir(&dir);
    let main = root.join("main");
    init_repo(main.as_std_path(), None);
    add_worktree(main.as_std_path(), root.join("wt").as_std_path(), "feature");
    let resolver = ProjectResolver::new(Vec::new(), vec![root.join("protected")]);
    assert_eq!(
        resolver.head_branch(&root.join("wt")).as_deref(),
        Some("feature")
    );
    assert_eq!(resolver.head_branch(&root.join("gone")), None);
    fs::create_dir_all(root.join("protected/repo")).unwrap();
    init_repo(root.join("protected/repo").as_std_path(), None);
    assert_eq!(resolver.head_branch(&root.join("protected/repo")), None);
}
