//! Intimatr core library.
//!
//! The project is intentionally split so the in-process UI, debugger, and RPC
//! frontends can all share the same configuration and scanner semantics.

pub mod analysis;
pub mod backpressure;
pub mod bottles;
pub mod command;
pub mod config;
pub mod debugger;
pub mod lifecycle;
pub mod logging;
pub mod memory;
pub mod platform;
pub mod rpc;
pub mod runtime;
pub mod scanner;
pub mod speed;
pub mod speed_config;
pub mod speed_controls;

#[cfg(windows)]
pub mod debugger_ui;

#[cfg(windows)]
pub mod ui;

#[cfg(windows)]
mod dll;
