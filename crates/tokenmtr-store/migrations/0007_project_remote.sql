-- Every git remote of a repository, keyed by its root (`project_map.repo_root`). Only remotes
-- whose URL names a shared host are kept, and only in normalized form: credentials never reach
-- the store. Rewritten whenever a working directory of the repository is resolved.
CREATE TABLE project_remote (
    repo_root      TEXT NOT NULL,
    name           TEXT NOT NULL,  -- remote name, such as `origin` or `upstream`
    url            TEXT NOT NULL,  -- normalized `host/owner/repo`
    default_branch TEXT,           -- target of `refs/remotes/<name>/HEAD`, short (`main`)
    PRIMARY KEY (repo_root, name)
) STRICT, WITHOUT ROWID;

-- A remote-keyed project is named after its URL path (`owner/repo`), not its last segment.
UPDATE project_map SET name = substr(remote, instr(remote, '/') + 1)
WHERE source IN ('remote', 'inferred') AND remote IS NOT NULL AND project = remote
  AND instr(remote, '/') > 0;
