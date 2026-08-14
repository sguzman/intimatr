//! Intimatr core library.
//!
//! The project is intentionally split so the in-process UI, debugger, and RPC
//! frontends can all share the same configuration and scanner semantics.

pub mod config;
pub mod logging;
pub mod scanner;
