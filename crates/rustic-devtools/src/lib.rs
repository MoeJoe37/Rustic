//! Rustic's automation and DevTools protocol (ADR 0069).
//!
//! `rustic automate` serves one page over standard input and output: the
//! client writes one JSON command per line and reads one JSON response per
//! command, plus events. This crate holds the parts that do not depend on
//! the engine, so the broker can check commands before any renderer sees
//! them and the fuzzer can reach them:
//!
//! - [`json`]: a bounded JSON reader and a one-line writer;
//! - [`protocol`]: framing, the command gate and output validation;
//! - [`authority`]: the grants and the method table;
//! - [`redact`]: credential redaction for network diagnostics;
//! - [`base64`]: screenshot encoding.
//!
//! There is no network listener: the only transport is the standard
//! streams of the process the user started, so remote automation is off
//! by construction.

#![warn(missing_docs)]

pub mod authority;
pub mod base64;
pub mod json;
pub mod protocol;
pub mod redact;

pub use authority::{Grant, Grants};
pub use json::Value;
pub use protocol::{Command, ErrorCode, Refusal};
