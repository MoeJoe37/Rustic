//! Protocol framing: one JSON object per line in each direction.
//!
//! A command is `{"id": N, "method": "domain.name", "params": {...}}`
//! (`params` optional). Each command gets exactly one response line,
//! `{"id": N, "result": ...}` or `{"id": N, "error": {"code", "message"}}`;
//! the session may also send event lines, `{"event": "...", "params": ...}`.

use crate::authority::{Grants, required_grant};
use crate::json::Value;

/// The protocol name and version reported by `session.capabilities`.
pub const PROTOCOL: &str = "rustic-automation/1";

/// Longest command line a session reads, in bytes.
pub const MAX_COMMAND_BYTES: usize = 1 << 20;

/// Longest output line a session writes, in bytes (a screenshot fits).
pub const MAX_OUTPUT_BYTES: usize = 64 << 20;

/// A parsed, admitted command.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    /// The client's id, echoed in the response.
    pub id: u64,
    /// The method.
    pub method: String,
    /// The parameters (an object; empty when omitted).
    pub params: Value,
}

/// Machine-readable error codes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorCode {
    /// The line is not JSON.
    Parse,
    /// The JSON is not a command.
    InvalidRequest,
    /// No such method.
    UnknownMethod,
    /// The session was not given the method's grant.
    NotGranted,
    /// The parameters are missing or wrong.
    InvalidParams,
    /// The method is not available in the session's current state (for
    /// example, inspection while paused in the debugger).
    WrongState,
    /// The method ran and failed.
    Failed,
}

impl ErrorCode {
    /// The code's protocol name.
    pub fn name(self) -> &'static str {
        match self {
            ErrorCode::Parse => "parse-error",
            ErrorCode::InvalidRequest => "invalid-request",
            ErrorCode::UnknownMethod => "unknown-method",
            ErrorCode::NotGranted => "not-granted",
            ErrorCode::InvalidParams => "invalid-params",
            ErrorCode::WrongState => "wrong-state",
            ErrorCode::Failed => "failed",
        }
    }
}

/// A refused or failed command.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal {
    /// The command's id, when it had a readable one.
    pub id: Option<u64>,
    /// What went wrong.
    pub code: ErrorCode,
    /// A message for people.
    pub message: String,
}

impl Refusal {
    /// A refusal of command `id`.
    pub fn new(id: Option<u64>, code: ErrorCode, message: impl Into<String>) -> Refusal {
        Refusal {
            id,
            code,
            message: message.into(),
        }
    }

    /// The response line.
    pub fn to_line(&self) -> String {
        Value::object([
            (
                "id",
                self.id.map_or(Value::Null, |id| Value::Number(id as f64)),
            ),
            (
                "error",
                Value::object([
                    ("code", Value::string(self.code.name())),
                    ("message", Value::string(self.message.clone())),
                ]),
            ),
        ])
        .to_json()
    }
}

/// Parse a command line and check it against the session's grants.
/// This is the gate the broker applies before a command reaches a
/// renderer, and the session applies again in the renderer.
pub fn admit(line: &str, grants: Grants) -> Result<Command, Refusal> {
    if line.len() > MAX_COMMAND_BYTES {
        return Err(Refusal::new(
            None,
            ErrorCode::InvalidRequest,
            "command line too long",
        ));
    }
    let value = Value::parse(line)
        .map_err(|error| Refusal::new(None, ErrorCode::Parse, error.to_string()))?;
    let Value::Object(members) = &value else {
        return Err(Refusal::new(
            None,
            ErrorCode::InvalidRequest,
            "a command is a JSON object",
        ));
    };
    let id = value.get("id").and_then(Value::as_u64);
    let Some(id) = id else {
        return Err(Refusal::new(
            None,
            ErrorCode::InvalidRequest,
            "a command needs a non-negative integer id",
        ));
    };
    if members
        .iter()
        .any(|(key, _)| !matches!(key.as_str(), "id" | "method" | "params"))
    {
        return Err(Refusal::new(
            Some(id),
            ErrorCode::InvalidRequest,
            "unknown command member",
        ));
    }
    let Some(method) = value.get("method").and_then(Value::as_str) else {
        return Err(Refusal::new(
            Some(id),
            ErrorCode::InvalidRequest,
            "a command needs a method",
        ));
    };
    let params = match value.get("params") {
        None => Value::Object(Vec::new()),
        Some(params @ Value::Object(_)) => params.clone(),
        Some(_) => {
            return Err(Refusal::new(
                Some(id),
                ErrorCode::InvalidParams,
                "params must be an object",
            ));
        }
    };
    match required_grant(method) {
        None => Err(Refusal::new(
            Some(id),
            ErrorCode::UnknownMethod,
            format!("unknown method {method:?}"),
        )),
        Some(Some(grant)) if !grants.contains(grant) => Err(Refusal::new(
            Some(id),
            ErrorCode::NotGranted,
            format!(
                "{method} needs the {:?} grant (--devtools-allow {})",
                grant.name(),
                grant.name()
            ),
        )),
        Some(_) => Ok(Command {
            id,
            method: method.to_owned(),
            params,
        }),
    }
}

/// A success response line.
pub fn response(id: u64, result: Value) -> String {
    Value::object([("id", Value::Number(id as f64)), ("result", result)]).to_json()
}

/// An event line.
pub fn event(name: &str, params: Value) -> String {
    Value::object([("event", Value::string(name)), ("params", params)]).to_json()
}

/// Whether `line` is a well-formed output line: within
/// [`MAX_OUTPUT_BYTES`], on one line, and a JSON object that is a
/// response (`id` with `result` or `error`) or an event. The broker
/// forwards a renderer's lines only when they pass.
pub fn valid_output(line: &str) -> bool {
    if line.len() > MAX_OUTPUT_BYTES || line.contains(['\n', '\r']) {
        return false;
    }
    let Ok(value @ Value::Object(_)) = Value::parse(line) else {
        return false;
    };
    let response = value.get("id").is_some()
        && (value.get("result").is_some() != value.get("error").is_some());
    let event = value.get("event").and_then(Value::as_str).is_some() && value.get("id").is_none();
    response || event
}
