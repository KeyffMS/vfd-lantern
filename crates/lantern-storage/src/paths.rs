use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use lantern_app::PathOverrides;
use lantern_domain::{LoggingId, SessionId};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppPaths {
    pub config_file: PathBuf,
    pub user_profiles: PathBuf,
    pub profile_trust_store: PathBuf,
    pub data_root: PathBuf,
    pub backup_directory: PathBuf,
    pub csv_directory: PathBuf,
    pub fault_report_directory: PathBuf,
    pub diagnostics_directory: PathBuf,
    pub state_root: PathBuf,
    pub log_directory: PathBuf,
    pub audit_directory: PathBuf,
    pub session_runtime_directory: PathBuf,
    pub panic_directory: PathBuf,
    pub cache_root: PathBuf,
}

impl AppPaths {
    pub fn resolve(overrides: &PathOverrides) -> Result<Self, PathError> {
        let project = ProjectDirs::from("pl", "aiteracja", "vfd-lantern")
            .ok_or(PathError::Unavailable)?;
        let config_root = project.config_dir().to_path_buf();
        let data_root = overrides
            .data
            .clone()
            .unwrap_or_else(|| project.data_dir().to_path_buf());
        let state_root = overrides.state.clone().unwrap_or_else(|| {
            project
                .state_dir()
                .unwrap_or(project.data_local_dir())
                .to_path_buf()
        });
        let log_directory = overrides
            .log
            .clone()
            .unwrap_or_else(|| state_root.join("logs"));
        Ok(Self::from_roots(
            config_root,
            data_root,
            state_root,
            project.cache_dir().to_path_buf(),
            log_directory,
        ))
    }

    #[must_use]
    pub fn from_roots(
        config_root: PathBuf,
        data_root: PathBuf,
        state_root: PathBuf,
        cache_root: PathBuf,
        log_directory: PathBuf,
    ) -> Self {
        Self {
            config_file: config_root.join("config.toml"),
            user_profiles: config_root.join("profiles"),
            profile_trust_store: config_root.join("profile-trust.json"),
            backup_directory: data_root.join("backups"),
            csv_directory: data_root.join("csv"),
            fault_report_directory: data_root.join("fault-reports"),
            diagnostics_directory: data_root.join("diagnostics"),
            audit_directory: state_root.join("audit"),
            session_runtime_directory: state_root.join("sessions"),
            panic_directory: state_root.join("panic"),
            config_file,
            data_root,
            state_root,
            log_directory,
            cache_root,
        }
    }

    #[must_use]
    pub fn final_csv_sidecar(csv: &Path) -> PathBuf {
        PathBuf::from(format!("{}.session.json", csv.display()))
    }

    #[must_use]
    pub fn runtime_logging_checkpoint(
        &self,
        session_id: SessionId,
        logging_id: LoggingId,
    ) -> PathBuf {
        self.session_runtime_directory.join(format!(
            "session-runtime-{}-{}.json",
            session_id.get(),
            logging_id.get()
        ))
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum PathError {
    #[error("XDG project directories are unavailable")]
    Unavailable,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use lantern_domain::{LoggingId, SessionId};

    use super::AppPaths;

    #[test]
    fn data_and_state_artifacts_are_not_conflated() {
        let paths = AppPaths::from_roots(
            PathBuf::from("/cfg"),
            PathBuf::from("/data"),
            PathBuf::from("/state"),
            PathBuf::from("/cache"),
            PathBuf::from("/logs"),
        );
        let csv = paths.csv_directory.join("capture.csv");
        assert_eq!(
            AppPaths::final_csv_sidecar(&csv),
            PathBuf::from("/data/csv/capture.csv.session.json")
        );
        assert_eq!(
            paths.runtime_logging_checkpoint(SessionId::new(7), LoggingId::new(3)),
            PathBuf::from("/state/sessions/session-runtime-7-3.json")
        );
        assert_ne!(
            paths.runtime_logging_checkpoint(SessionId::new(7), LoggingId::new(3)),
            paths.runtime_logging_checkpoint(SessionId::new(7), LoggingId::new(4))
        );
    }
}
