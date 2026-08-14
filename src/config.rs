use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub target: TargetConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub rpc: RpcConfig,
    #[serde(default)]
    pub scanner: ScannerConfig,
    #[serde(default)]
    pub debugger: DebuggerConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub policy: PolicyConfig,
}

impl AppConfig {
    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(input)?;
        config.validate()?;
        Ok(config)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        debug!(path = %path.display(), "loading Intimatr configuration");
        let contents = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let config = Self::from_toml_str(&contents)?;
        info!(path = %path.display(), target = %config.target.executable, "loaded Intimatr configuration");
        Ok(config)
    }

    pub fn config_path_for_executable(
        config_dir: impl AsRef<Path>,
        executable_path: impl AsRef<Path>,
    ) -> Result<PathBuf, ConfigError> {
        let executable_name = executable_file_name(executable_path.as_ref())?;
        Ok(config_dir.as_ref().join(format!("{executable_name}.toml")))
    }

    pub fn load_for_executable(
        config_dir: impl AsRef<Path>,
        executable_path: impl AsRef<Path>,
    ) -> Result<Self, ConfigError> {
        let executable_path = executable_path.as_ref();
        let config_path = Self::config_path_for_executable(config_dir, executable_path)?;
        let config = Self::load(config_path)?;
        config.validate_for_executable(executable_path)?;
        Ok(config)
    }

    pub fn validate_for_executable(&self, executable_path: &Path) -> Result<(), ConfigError> {
        let actual = executable_file_name(executable_path)?;
        if !self.target.executable.eq_ignore_ascii_case(actual) {
            return Err(ConfigError::ExecutableMismatch {
                configured: self.target.executable.clone(),
                actual: actual.to_owned(),
            });
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.target.executable.trim().is_empty() {
            return Err(ConfigError::InvalidValue(
                "target.executable must not be empty",
            ));
        }
        if self.scanner.chunk_size_bytes == 0 {
            return Err(ConfigError::InvalidValue(
                "scanner.chunk_size_bytes must be greater than zero",
            ));
        }
        if self.scanner.alignment == 0 {
            return Err(ConfigError::InvalidValue(
                "scanner.alignment must be greater than zero",
            ));
        }
        if self.scanner.max_results == 0 {
            return Err(ConfigError::InvalidValue(
                "scanner.max_results must be greater than zero",
            ));
        }
        if !self.scanner.float_epsilon.is_finite() || self.scanner.float_epsilon < 0.0 {
            return Err(ConfigError::InvalidValue(
                "scanner.float_epsilon must be finite and non-negative",
            ));
        }
        if self.rpc.max_clients == 0 {
            return Err(ConfigError::InvalidValue(
                "rpc.max_clients must be greater than zero",
            ));
        }
        if self.rpc.max_request_bytes == 0 {
            return Err(ConfigError::InvalidValue(
                "rpc.max_request_bytes must be greater than zero",
            ));
        }
        Ok(())
    }
}

fn executable_file_name(path: &Path) -> Result<&str, ConfigError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| ConfigError::InvalidExecutablePath(path.to_path_buf()))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetConfig {
    pub executable: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_filter")]
    pub filter: String,
    #[serde(default = "default_log_directory")]
    pub directory: PathBuf,
    #[serde(default = "default_log_file_name")]
    pub file_name: String,
    #[serde(default)]
    pub console: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            filter: default_log_filter(),
            directory: default_log_directory(),
            file_name: default_log_file_name(),
            console: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub transport: RpcTransport,
    #[serde(default = "default_rpc_bind")]
    pub bind: String,
    #[serde(default = "default_max_clients")]
    pub max_clients: usize,
    #[serde(default = "default_max_request_bytes")]
    pub max_request_bytes: usize,
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            transport: RpcTransport::Tcp,
            bind: default_rpc_bind(),
            max_clients: default_max_clients(),
            max_request_bytes: default_max_request_bytes(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcTransport {
    #[default]
    Tcp,
    NamedPipe,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScannerConfig {
    #[serde(default = "default_chunk_size")]
    pub chunk_size_bytes: usize,
    #[serde(default = "default_alignment")]
    pub alignment: usize,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default = "default_float_epsilon")]
    pub float_epsilon: f64,
    #[serde(default = "default_true")]
    pub require_readable: bool,
    #[serde(default)]
    pub require_writable: bool,
    #[serde(default)]
    pub require_executable: bool,
    #[serde(default)]
    pub include_guard_pages: bool,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            chunk_size_bytes: default_chunk_size(),
            alignment: default_alignment(),
            max_results: default_max_results(),
            float_epsilon: default_float_epsilon(),
            require_readable: true,
            require_writable: false,
            require_executable: false,
            include_guard_pages: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebuggerConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub break_on_start: bool,
    #[serde(default = "default_true")]
    pub prefer_hardware_breakpoints: bool,
}

impl Default for DebuggerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            break_on_start: false,
            prefer_hardware_breakpoints: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_toggle_key")]
    pub toggle_key: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            toggle_key: default_toggle_key(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyConfig {
    #[serde(default = "default_true")]
    pub allow_memory_read: bool,
    #[serde(default = "default_true")]
    pub allow_memory_write: bool,
    #[serde(default)]
    pub allow_code_patch: bool,
    #[serde(default = "default_true")]
    pub allow_debugger: bool,
    #[serde(default)]
    pub allow_remote_shutdown: bool,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            allow_memory_read: true,
            allow_memory_write: true,
            allow_code_patch: false,
            allow_debugger: true,
            allow_remote_shutdown: false,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read configuration at {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse TOML configuration: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("invalid executable path: {0}")]
    InvalidExecutablePath(PathBuf),
    #[error("configuration targets {configured}, but the loaded executable is {actual}")]
    ExecutableMismatch { configured: String, actual: String },
    #[error("invalid configuration: {0}")]
    InvalidValue(&'static str),
}

fn default_true() -> bool {
    true
}

fn default_log_filter() -> String {
    "intimatr=trace".to_owned()
}

fn default_log_directory() -> PathBuf {
    PathBuf::from("logs")
}

fn default_log_file_name() -> String {
    "intimatr.log".to_owned()
}

fn default_rpc_bind() -> String {
    "127.0.0.1:31337".to_owned()
}

fn default_max_clients() -> usize {
    4
}

fn default_max_request_bytes() -> usize {
    1024 * 1024
}

fn default_chunk_size() -> usize {
    1024 * 1024
}

fn default_alignment() -> usize {
    1
}

fn default_max_results() -> usize {
    2_000_000
}

fn default_float_epsilon() -> f64 {
    1.0e-6
}

fn default_toggle_key() -> String {
    "Insert".to_owned()
}
