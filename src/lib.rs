//! Intimatr core library.
//!
//! The project is intentionally split so the in-process UI, debugger, and RPC
//! frontends can all share the same configuration and scanner semantics.

pub mod analysis;
pub mod backpressure;
pub mod bottles;
#[cfg(target_os = "linux")]
pub mod bottles_attachment_readiness;
pub mod bottles_live_matches;
#[cfg(target_os = "linux")]
pub mod bottles_prefix;
pub mod bottles_preflight;
#[cfg(target_os = "linux")]
pub mod bottles_processes;
#[cfg(target_os = "linux")]
pub mod bottles_runner;
pub mod bottles_selection;
#[cfg(target_os = "linux")]
pub mod bottles_speed_cli;
pub mod command;
pub mod config;
pub mod debugger;
#[cfg(windows)]
pub mod experimental_timer_probe;
pub mod lifecycle;
pub mod logging;
pub mod memory;
pub mod pe_image;
pub mod pe_timer_imports;
pub mod platform;
pub mod rpc;
pub mod runtime;
pub mod scanner;
pub mod speed;
pub mod speed_commands;
pub mod speed_config;
pub mod speed_controls;
pub mod speed_coordinator;
#[cfg(windows)]
pub mod speed_hook_lifecycle;
#[cfg(windows)]
pub mod speed_hotkey_worker;
pub mod speed_hotkeys;
pub mod speed_input;
#[cfg(windows)]
pub mod speed_input_windows;
pub mod speed_policy;
pub mod speed_profile_fingerprint;
pub mod speed_profile_load;
pub mod speed_session;
pub mod speed_timers;
pub mod speed_window;
#[cfg(windows)]
pub mod speed_window_windows;
#[cfg(windows)]
pub mod windows_process_identity;
#[cfg(windows)]
pub mod windows_process_machine;

#[cfg(windows)]
pub mod debugger_ui;

#[cfg(windows)]
pub mod ui;

#[cfg(windows)]
mod dll;
