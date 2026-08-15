use std::{
    fs,
    net::SocketAddr,
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
        if self.rpc.max_response_bytes == 0 {
            return Err(ConfigError::InvalidValue(
                "rpc.max_response_bytes must be greater than zero",
            ));
        }
        if self.rpc.max_memory_transfer_bytes == 0 {
            return Err(ConfigError::InvalidValue(
                "rpc.max_memory_transfer_bytes must be greater than zero",
            ));
        }
        if self.rpc.max_scan_results_per_page == 0 {
            return Err(ConfigError::InvalidValue(
                "rpc.max_scan_results_per_page must be greater than zero",
            ));
        }
        match self.rpc.transport {
            RpcTransport::Tcp => {
                let address: SocketAddr = self.rpc.bind.parse().map_err(|_| {
                    ConfigError::InvalidValue("rpc.bind must be a valid socket address")
                })?;
                if !address.ip().is_loopback() {
                    return Err(ConfigError::InvalidValue(
                        "rpc.bind must use a loopback IP address",
                    ));
                }
            }
            RpcTransport::NamedPipe => {
                if self.rpc.pipe_name.trim().is_empty() {
                    return Err(ConfigError::InvalidValue(
                        "rpc.pipe_name must not be empty for named-pipe transport",
                    ));
                }
                if self.rpc.max_clients > 254 {
                    return Err(ConfigError::InvalidValue(
                        "rpc.max_clients must not exceed 254 for named-pipe transport",
                    ));
                }
            }
        }

        if self.debugger.max_disassembly_bytes == 0 {
            return Err(ConfigError::InvalidValue(
                "debugger.max_disassembly_bytes must be greater than zero",
            ));
        }
        if self.debugger.max_disassembly_instructions == 0 {
            return Err(ConfigError::InvalidValue(
                "debugger.max_disassembly_instructions must be greater than zero",
            ));
        }
        if self.debugger.max_hardware_breakpoints == 0
            || self.debugger.max_hardware_breakpoints > 64
        {
            return Err(ConfigError::InvalidValue(
                "debugger.max_hardware_breakpoints must be between 1 and 64",
            ));
        }
        if self.debugger.max_events_per_poll == 0 || self.debugger.max_events_per_poll > 512 {
            return Err(ConfigError::InvalidValue(
                "debugger.max_events_per_poll must be between 1 and 512",
            ));
        }
        if self.debugger.ui_toggle_key.trim().is_empty() {
            return Err(ConfigError::InvalidValue(
                "debugger.ui_toggle_key must not be empty",
            ));
        }
        if !self.debugger.ui_width.is_finite() || self.debugger.ui_width <= 0.0 {
            return Err(ConfigError::InvalidValue(
                "debugger.ui_width must be finite and greater than zero",
            ));
        }
        if !self.debugger.ui_height.is_finite() || self.debugger.ui_height <= 0.0 {
            return Err(ConfigError::InvalidValue(
                "debugger.ui_height must be finite and greater than zero",
            ));
        }
        if self.debugger.event_poll_ms == 0 {
            return Err(ConfigError::InvalidValue(
                "debugger.event_poll_ms must be greater than zero",
            ));
        }
        if self.debugger.disassembly_default_bytes == 0
            || self.debugger.disassembly_default_bytes > self.debugger.max_disassembly_bytes
        {
            return Err(ConfigError::InvalidValue(
                "debugger.disassembly_default_bytes must be between 1 and debugger.max_disassembly_bytes",
            ));
        }
        if self.debugger.disassembly_default_instructions == 0
            || self.debugger.disassembly_default_instructions
                > self.debugger.max_disassembly_instructions
        {
            return Err(ConfigError::InvalidValue(
                "debugger.disassembly_default_instructions must be between 1 and debugger.max_disassembly_instructions",
            ));
        }

        if self.ui.toggle_key.trim().is_empty() {
            return Err(ConfigError::InvalidValue("ui.toggle_key must not be empty"));
        }
        if !self.ui.width.is_finite() || self.ui.width <= 0.0 {
            return Err(ConfigError::InvalidValue(
                "ui.width must be finite and greater than zero",
            ));
        }
        if !self.ui.height.is_finite() || self.ui.height <= 0.0 {
            return Err(ConfigError::InvalidValue(
                "ui.height must be finite and greater than zero",
            ));
        }
        if self.ui.watch_refresh_ms == 0 {
            return Err(ConfigError::InvalidValue(
                "ui.watch_refresh_ms must be greater than zero",
            ));
        }
        if self.ui.scan_page_size == 0 {
            return Err(ConfigError::InvalidValue(
                "ui.scan_page_size must be greater than zero",
            ));
        }
        if self.ui.scan_page_size > self.rpc.max_scan_results_per_page {
            return Err(ConfigError::InvalidValue(
                "ui.scan_page_size must not exceed rpc.max_scan_results_per_page",
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
    #[serde(default = "default_pipe_name")]
    pub pipe_name: String,
    #[serde(default = "default_max_clients")]
    pub max_clients: usize,
    #[serde(default = "default_max_request_bytes")]
    pub max_request_bytes: usize,
    #[serde(default = "default_max_response_bytes")]
    pub max_response_bytes: usize,
    #[serde(default = "default_max_memory_transfer_bytes")]
    pub max_memory_transfer_bytes: usize,
    #[serde(default = "default_max_scan_results_per_page")]
    pub max_scan_results_per_page: usize,
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            transport: RpcTransport::Tcp,
            bind: default_rpc_bind(),
            pipe_name: default_pipe_name(),
            max_clients: default_max_clients(),
            max_request_bytes: default_max_request_bytes(),
            max_response_bytes: default_max_response_bytes(),
            max_memory_transfer_bytes: default_max_memory_transfer_bytes(),
            max_scan_results_per_page: default_max_scan_results_per_page(),
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
    #[serde(default = "default_max_disassembly_bytes")]
    pub max_disassembly_bytes: usize,
    #[serde(default = "default_max_disassembly_instructions")]
    pub max_disassembly_instructions: usize,
    #[serde(default = "default_max_hardware_breakpoints")]
    pub max_hardware_breakpoints: usize,
    #[serde(default = "default_max_debugger_events_per_poll")]
    pub max_events_per_poll: usize,
    #[serde(default = "default_true")]
    pub ui_enabled: bool,
    #[serde(default)]
    pub ui_initially_visible: bool,
    #[serde(default)]
    pub ui_always_on_top: bool,
    #[serde(default = "default_debugger_toggle_key")]
    pub ui_toggle_key: String,
    #[serde(default = "default_debugger_ui_width")]
    pub ui_width: f32,
    #[serde(default = "default_debugger_ui_height")]
    pub ui_height: f32,
    #[serde(default = "default_debugger_event_poll_ms")]
    pub event_poll_ms: u64,
    #[serde(default = "default_disassembly_bytes")]
    pub disassembly_default_bytes: usize,
    #[serde(default = "default_disassembly_instructions")]
    pub disassembly_default_instructions: usize,
}

impl Default for DebuggerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            break_on_start: false,
            prefer_hardware_breakpoints: true,
            max_disassembly_bytes: default_max_disassembly_bytes(),
            max_disassembly_instructions: default_max_disassembly_instructions(),
            max_hardware_breakpoints: default_max_hardware_breakpoints(),
            max_events_per_poll: default_max_debugger_events_per_poll(),
            ui_enabled: true,
            ui_initially_visible: false,
            ui_always_on_top: false,
            ui_toggle_key: default_debugger_toggle_key(),
            ui_width: default_debugger_ui_width(),
            ui_height: default_debugger_ui_height(),
            event_poll_ms: default_debugger_event_poll_ms(),
            disassembly_default_bytes: default_disassembly_bytes(),
            disassembly_default_instructions: default_disassembly_instructions(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub initially_visible: bool,
    #[serde(default)]
    pub always_on_top: bool,
    #[serde(default = "default_toggle_key")]
    pub toggle_key: String,
    #[serde(default = "default_ui_width")]
    pub width: f32,
    #[serde(default = "default_ui_height")]
    pub height: f32,
    #[serde(default = "default_watch_refresh_ms")]
    pub watch_refresh_ms: u64,
    #[serde(default = "default_ui_scan_page_size")]
    pub scan_page_size: usize,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            initially_visible: true,
            always_on_top: false,
            toggle_key: default_toggle_key(),
            width: default_ui_width(),
            height: default_ui_height(),
            watch_refresh_ms: default_watch_refresh_ms(),
            scan_page_size: default_ui_scan_page_size(),
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
fn default_pipe_name() -> String {
    "intimatr".to_owned()
}
fn default_max_clients() -> usize {
    4
}
fn default_max_request_bytes() -> usize {
    1024 * 1024
}
fn default_max_response_bytes() -> usize {
    4 * 1024 * 1024
}
fn default_max_memory_transfer_bytes() -> usize {
    256 * 1024
}
fn default_max_scan_results_per_page() -> usize {
    4096
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
fn default_max_disassembly_bytes() -> usize {
    64 * 1024
}
fn default_max_disassembly_instructions() -> usize {
    512
}
fn default_max_hardware_breakpoints() -> usize {
    32
}
fn default_max_debugger_events_per_poll() -> usize {
    256
}
fn default_debugger_toggle_key() -> String {
    "F10".to_owned()
}
fn default_debugger_ui_width() -> f32 {
    1160.0
}
fn default_debugger_ui_height() -> f32 {
    760.0
}
fn default_debugger_event_poll_ms() -> u64 {
    100
}
fn default_disassembly_bytes() -> usize {
    256
}
fn default_disassembly_instructions() -> usize {
    64
}
fn default_toggle_key() -> String {
    "Insert".to_owned()
}
fn default_ui_width() -> f32 {
    1180.0
}
fn default_ui_height() -> f32 {
    760.0
}
fn default_watch_refresh_ms() -> u64 {
    250
}
fn default_ui_scan_page_size() -> usize {
    256
}
