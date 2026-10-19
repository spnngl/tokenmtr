//! The git remotes of one project, for its detail screen.

use serde::Serialize;
use tokenmtr_core::ProjectKey;
use tokenmtr_store::{StoreRead as _, StoreReader};

use crate::error::QueryError;

/// One git remote of a project's repositories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectRemote {
    /// Remote name, such as `origin`.
    pub name: String,
    /// Host of the normalized URL, such as `github.com`.
    pub host: String,
    /// Path after the host: `owner/repo`, or `group/sub/repo`.
    pub repository: String,
    /// The remote's default branch, when the clone recorded it.
    pub default_branch: Option<String>,
    /// Whether this remote's URL is the project key.
    pub keys_project: bool,
}

/// The remotes of every repository `project` spans, by name. Empty for a project that is not
/// a git repository, or whose paths are redacted.
///
/// # Errors
///
/// Store failures.
pub fn project_remotes(
    reader: &StoreReader,
    project: &ProjectKey,
) -> Result<Vec<ProjectRemote>, QueryError> {
    Ok(reader
        .project_remotes(project)?
        .into_iter()
        .map(|remote| {
            let (host, repository) = remote
                .url
                .split_once('/')
                .map_or((remote.url.as_str(), ""), |(host, path)| (host, path));
            ProjectRemote {
                keys_project: remote.url == project.as_str(),
                host: host.to_owned(),
                repository: repository.to_owned(),
                name: remote.name,
                default_branch: remote.default_branch,
            }
        })
        .collect())
}
