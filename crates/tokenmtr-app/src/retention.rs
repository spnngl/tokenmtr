//! Claude transcript retention (D53): `cleanupPeriodDays` in the profile's `settings.json`.

use std::io;

use camino::Utf8Path;
use serde_json::{Map, Value};
use tokenmtr_platform::fs::replace_atomically;

use crate::error::AppError;

/// The retention `TokenMtr` sets: ten years, so transcripts outlive any history window.
pub const RETENTION_DAYS: u64 = 3650;

const KEY: &str = "cleanupPeriodDays";

/// Sets `cleanupPeriodDays` to [`RETENTION_DAYS`] in the `settings.json` of a Claude config
/// directory, keeping every other key and their order. A missing file is created. This is the
/// one write `TokenMtr` makes to another tool's files, and the user asks for it (D53).
///
/// # Errors
///
/// [`AppError::AgentSettings`] when the file is not a JSON object (it is left untouched);
/// [`AppError::Io`] on read or write failures.
pub fn set_claude_retention(config_dir: &Utf8Path) -> Result<(), AppError> {
    let path = config_dir.join("settings.json");
    let mut settings = match std::fs::read(&path) {
        Ok(body) => match serde_json::from_slice::<Value>(&body) {
            Ok(Value::Object(map)) => map,
            Ok(_) => {
                return Err(AppError::AgentSettings {
                    path,
                    reason: "not a JSON object".to_owned(),
                });
            }
            Err(error) => {
                return Err(AppError::AgentSettings {
                    path,
                    reason: error.to_string(),
                });
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Map::new(),
        Err(error) => return Err(AppError::io(path, error)),
    };
    settings.insert(KEY.to_owned(), Value::from(RETENTION_DAYS));
    let mut body = serde_json::to_vec_pretty(&Value::Object(settings)).map_err(|error| {
        AppError::AgentSettings {
            path: path.clone(),
            reason: error.to_string(),
        }
    })?;
    body.push(b'\n');
    std::fs::create_dir_all(config_dir).map_err(|error| AppError::io(config_dir, error))?;
    replace_atomically(&path, &body).map_err(|error| AppError::io(path, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn dir() -> Result<(tempfile::TempDir, camino::Utf8PathBuf), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
            .map_err(|_| "temp dir is not UTF-8")?;
        Ok((dir, path))
    }

    #[test]
    fn other_keys_survive_in_order() -> TestResult {
        let (_guard, dir) = dir()?;
        std::fs::write(
            dir.join("settings.json"),
            r#"{"theme":"dark","cleanupPeriodDays":30,"permissions":{"allow":["Bash"]},"zeta":1}"#,
        )?;
        set_claude_retention(&dir)?;
        let written: Value = serde_json::from_slice(&std::fs::read(dir.join("settings.json"))?)?;
        assert_eq!(written["cleanupPeriodDays"], 3650);
        assert_eq!(written["theme"], "dark");
        assert_eq!(written["permissions"]["allow"][0], "Bash");
        let keys: Vec<&String> = written.as_object().ok_or("object")?.keys().collect();
        assert_eq!(keys, ["theme", "cleanupPeriodDays", "permissions", "zeta"]);
        Ok(())
    }

    #[test]
    fn a_missing_file_is_created() -> TestResult {
        let (_guard, dir) = dir()?;
        set_claude_retention(&dir.join("fresh"))?;
        let written: Value =
            serde_json::from_slice(&std::fs::read(dir.join("fresh/settings.json"))?)?;
        assert_eq!(written["cleanupPeriodDays"], 3650);
        Ok(())
    }

    #[test]
    fn a_file_that_is_not_an_object_is_left_alone() -> TestResult {
        let (_guard, dir) = dir()?;
        std::fs::write(dir.join("settings.json"), "[1]")?;
        assert!(matches!(
            set_claude_retention(&dir),
            Err(AppError::AgentSettings { .. })
        ));
        assert_eq!(std::fs::read_to_string(dir.join("settings.json"))?, "[1]");
        Ok(())
    }
}
