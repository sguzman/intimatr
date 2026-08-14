//! Intimatr core library.
//!
//! The project is intentionally split so the in-process UI, debugger, and RPC
//! frontends can all share the same configuration and scanner semantics.

pub mod command;
pub mod config;
pub mod lifecycle;
pub mod logging;
pub mod memory;
pub mod platform;
pub mod runtime;
pub mod scanner;

#[cfg(windows)]
mod dll;
