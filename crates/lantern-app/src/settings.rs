use std::{fmt, path::PathBuf, str::FromStr};

use lantern_domain::SlaveId;
use serde::Deserialize;
use thiserror::Error;

use crate::{SettingsSourceError, SettingsSourcePort};

pub const MAX_SETTINGS_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ColorMode {
    #[default]
    Auto,
    Enabled,
    Disabled,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        })
    }
}

impl FromStr for LogLevel {
    type Err = LogLevelParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            _ => Err(LogLevelParseError(value.to_owned())),
        }
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("invalid log level {0}; expected trace, debug, info, warn or error")]
pub struct LogLevelParseError(String);

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QueueCapacityDocument {
    pub safety_one_shot: Option<usize>,
    pub interactive: Option<usize>,
    pub telemetry_critical: Option<usize>,
    pub telemetry: Option<usize>,
    pub background: Option<usize>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PollingDocument {
    pub telemetry_critical_ms: Option<u64>,
    pub telemetry_ms: Option<u64>,
    pub background_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PathOverridesDocument {
    pub data: Option<PathBuf>,
    pub state: Option<PathBuf>,
    pub log: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SettingsDocumentV1 {
    pub render_fps: Option<u8>,
    pub color: Option<ColorMode>,
    pub history_samples: Option<usize>,
    pub memory_limit_mib: Option<usize>,
    pub log_retention_files: Option<usize>,
    pub suggested_profile: Option<PathBuf>,
    pub suggested_device: Option<PathBuf>,
    pub suggested_slave: Option<u8>,
    pub queues: Option<QueueCapacityDocument>,
    pub polling: Option<PollingDocument>,
    pub paths: Option<PathOverridesDocument>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CliSettingsOverrides {
    pub profile: Option<PathBuf>,
    pub device: Option<PathBuf>,
    pub log_level: Option<LogLevel>,
    pub enable_writes: bool,
    pub no_color: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueCapacities {
    pub safety_one_shot: usize,
    pub interactive: usize,
    pub telemetry_critical: usize,
    pub telemetry: usize,
    pub background: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PollingIntervals {
    pub telemetry_critical_ms: u64,
    pub telemetry_ms: u64,
    pub background_ms: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PathOverrides {
    pub data: Option<PathBuf>,
    pub state: Option<PathBuf>,
    pub log: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedSettings {
    pub render_fps: u8,
    pub color: ColorMode,
    pub history_samples: usize,
    pub memory_limit_mib: usize,
    pub log_retention_files: usize,
    pub queues: QueueCapacities,
    pub polling: PollingIntervals,
    pub paths: PathOverrides,
    pub suggested_profile: Option<PathBuf>,
    pub suggested_device: Option<PathBuf>,
    pub suggested_slave: Option<SlaveId>,
    pub log_level: LogLevel,
    pub process_writes_enabled: bool,
}

impl Default for ValidatedSettings {
    fn default() -> Self {
        Self {
            render_fps: 5,
            color: ColorMode::Auto,
            history_samples: 3_600,
            memory_limit_mib: 128,
            log_retention_files: 10,
            queues: QueueCapacities {
                safety_one_shot: 16,
                interactive: 64,
                telemetry_critical: 64,
                telemetry: 256,
                background: 32,
            },
            polling: PollingIntervals {
                telemetry_critical_ms: 250,
                telemetry_ms: 1_000,
                background_ms: 5_000,
            },
            paths: PathOverrides::default(),
            suggested_profile: None,
            suggested_device: None,
            suggested_slave: None,
            log_level: LogLevel::Info,
            process_writes_enabled: false,
        }
    }
}

pub struct SettingsLoader;

impl SettingsLoader {
    pub fn load(
        source: &dyn SettingsSourcePort,
        cli: CliSettingsOverrides,
        application_log_environment: Option<&str>,
    ) -> Result<ValidatedSettings, SettingsError> {
        let mut settings = ValidatedSettings::default();
        if let Some(bytes) = source.load_settings()? {
            if bytes.len() > MAX_SETTINGS_BYTES {
                return Err(SettingsError::TooLarge {
                    actual: bytes.len(),
                    maximum: MAX_SETTINGS_BYTES,
                });
            }
            let text = std::str::from_utf8(&bytes).map_err(|error| SettingsError::Parse {
                message: error.to_string(),
            })?;
            let document: SettingsDocumentV1 =
                toml::from_str(text).map_err(|error| SettingsError::Parse {
                    message: error.to_string(),
                })?;
            apply_document(&mut settings, document)?;
        }

        if let Some(profile) = cli.profile {
            settings.suggested_profile = Some(profile);
        }
        if let Some(device) = cli.device {
            settings.suggested_device = Some(device);
        }
        if let Some(level) = application_log_environment {
            settings.log_level = level.parse().map_err(|error: LogLevelParseError| {
                SettingsError::Validation(error.to_string())
            })?;
        }
        if let Some(level) = cli.log_level {
            settings.log_level = level;
        }
        if cli.no_color {
            settings.color = ColorMode::Disabled;
        }
        settings.process_writes_enabled = cli.enable_writes;
        Ok(settings)
    }
}

fn apply_document(
    settings: &mut ValidatedSettings,
    document: SettingsDocumentV1,
) -> Result<(), SettingsError> {
    if let Some(render_fps) = document.render_fps {
        settings.render_fps = bounded_u8("render_fps", render_fps, 1, 10)?;
    }
    if let Some(color) = document.color {
        settings.color = color;
    }
    if let Some(history) = document.history_samples {
        settings.history_samples = bounded("history_samples", history, 1, 1_000_000)?;
    }
    if let Some(memory) = document.memory_limit_mib {
        settings.memory_limit_mib = bounded("memory_limit_mib", memory, 16, 4_096)?;
    }
    if let Some(retention) = document.log_retention_files {
        settings.log_retention_files = bounded("log_retention_files", retention, 1, 1_000)?;
    }
    if let Some(slave) = document.suggested_slave {
        settings.suggested_slave = Some(
            SlaveId::new(slave).map_err(|error| SettingsError::Validation(error.to_string()))?,
        );
    }
    settings.suggested_profile = document.suggested_profile;
    settings.suggested_device = document.suggested_device;

    if let Some(queues) = document.queues {
        apply_queue(settings, queues)?;
    }
    if let Some(polling) = document.polling {
        apply_polling(settings, polling)?;
    }
    if let Some(paths) = document.paths {
        settings.paths = PathOverrides {
            data: paths.data,
            state: paths.state,
            log: paths.log,
        };
    }
    Ok(())
}

fn apply_queue(
    settings: &mut ValidatedSettings,
    document: QueueCapacityDocument,
) -> Result<(), SettingsError> {
    macro_rules! set {
        ($field:ident, $maximum:expr) => {
            if let Some(value) = document.$field {
                settings.queues.$field = bounded(stringify!($field), value, 1, $maximum)?;
            }
        };
    }
    set!(safety_one_shot, 64);
    set!(interactive, 1_024);
    set!(telemetry_critical, 1_024);
    set!(telemetry, 4_096);
    set!(background, 1_024);
    Ok(())
}

fn apply_polling(
    settings: &mut ValidatedSettings,
    document: PollingDocument,
) -> Result<(), SettingsError> {
    macro_rules! set {
        ($field:ident) => {
            if let Some(value) = document.$field {
                settings.polling.$field = bounded_u64(stringify!($field), value, 50, 60_000)?;
            }
        };
    }
    set!(telemetry_critical_ms);
    set!(telemetry_ms);
    set!(background_ms);
    Ok(())
}

fn bounded(
    name: &str,
    value: usize,
    minimum: usize,
    maximum: usize,
) -> Result<usize, SettingsError> {
    if (minimum..=maximum).contains(&value) {
        Ok(value)
    } else {
        Err(range_error(name, minimum, maximum))
    }
}

fn bounded_u64(name: &str, value: u64, minimum: u64, maximum: u64) -> Result<u64, SettingsError> {
    if (minimum..=maximum).contains(&value) {
        Ok(value)
    } else {
        Err(range_error(name, minimum, maximum))
    }
}

fn bounded_u8(name: &str, value: u8, minimum: u8, maximum: u8) -> Result<u8, SettingsError> {
    if (minimum..=maximum).contains(&value) {
        Ok(value)
    } else {
        Err(range_error(name, minimum, maximum))
    }
}

fn range_error(
    name: &str,
    minimum: impl fmt::Display,
    maximum: impl fmt::Display,
) -> SettingsError {
    SettingsError::Validation(format!("{name} must be in {minimum}..={maximum}"))
}

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error(transparent)]
    Source(#[from] SettingsSourceError),
    #[error("settings contain {actual} bytes; maximum is {maximum}")]
    TooLarge { actual: usize, maximum: usize },
    #[error("settings TOML is invalid: {message}")]
    Parse { message: String },
    #[error("settings validation failed: {0}")]
    Validation(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MemorySource(Option<Vec<u8>>);

    impl SettingsSourcePort for MemorySource {
        fn load_settings(&self) -> Result<Option<Vec<u8>>, SettingsSourceError> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn precedence_is_defaults_then_config_then_environment_then_cli() {
        let source = MemorySource(Some(
            br#"render_fps = 3
color = "enabled"
suggested_device = "/dev/config"
suggested_slave = 17
"#
            .to_vec(),
        ));
        let settings = SettingsLoader::load(
            &source,
            CliSettingsOverrides {
                device: Some(PathBuf::from("/dev/cli")),
                log_level: Some(LogLevel::Error),
                no_color: true,
                enable_writes: true,
                ..CliSettingsOverrides::default()
            },
            Some("debug"),
        )
        .expect("settings");
        assert_eq!(settings.render_fps, 3);
        assert_eq!(settings.suggested_device, Some(PathBuf::from("/dev/cli")));
        assert_eq!(settings.suggested_slave.map(SlaveId::get), Some(17));
        assert_eq!(settings.log_level, LogLevel::Error);
        assert_eq!(settings.color, ColorMode::Disabled);
        assert!(settings.process_writes_enabled);
    }

    #[test]
    fn dangerous_or_unknown_configuration_is_rejected_wholly() {
        let source = MemorySource(Some(b"enable_writes = true\nrender_fps = 2\n".to_vec()));
        assert!(matches!(
            SettingsLoader::load(&source, CliSettingsOverrides::default(), None),
            Err(SettingsError::Parse { .. })
        ));
    }

    #[test]
    fn all_unbounded_values_are_rejected() {
        for document in [
            "render_fps = 0\n",
            "history_samples = 0\n",
            "memory_limit_mib = 8\n",
            "[queues]\ntelemetry = 0\n",
            "[polling]\ntelemetry_ms = 1\n",
            "suggested_slave = 0\n",
        ] {
            assert!(
                SettingsLoader::load(
                    &MemorySource(Some(document.as_bytes().to_vec())),
                    CliSettingsOverrides::default(),
                    None,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn invalid_environment_log_level_fails_before_application_start() {
        assert!(matches!(
            SettingsLoader::load(
                &MemorySource(None),
                CliSettingsOverrides::default(),
                Some("verbose")
            ),
            Err(SettingsError::Validation(_))
        ));
    }

    #[test]
    fn missing_file_uses_safe_defaults() {
        let settings =
            SettingsLoader::load(&MemorySource(None), CliSettingsOverrides::default(), None)
                .expect("defaults");
        assert!(!settings.process_writes_enabled);
        assert_eq!(settings.render_fps, 5);
        assert_eq!(settings.log_level, LogLevel::Info);
    }
}
