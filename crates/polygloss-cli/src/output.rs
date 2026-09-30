//! Human text vs JSON output and the error shape (design §14, T4.3).
//!
//! JSON is the default when stdout is not a terminal (`--json` forces it).
//! A command prints one JSON object on stdout, or in human mode a few lines of
//! text. Errors exit 1: in JSON mode `{"error": {"code", "message"}}` on
//! stdout (codes as MCP's, design §15.1), in human mode `polygloss: <message>`
//! on stderr.

use std::fmt;
use std::io::{IsTerminal as _, Write as _};

use polygloss_core::review::CoreError;
use polygloss_mcp::ApiError;
use serde_json::{Value, json};

/// How results and errors are printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Human,
    Json,
}

impl Mode {
    /// JSON when asked for, or when stdout is not a terminal.
    pub fn detect(json_flag: bool) -> Mode {
        Mode::choose(json_flag, std::io::stdout().is_terminal())
    }

    pub fn choose(json_flag: bool, stdout_is_tty: bool) -> Mode {
        if json_flag || !stdout_is_tty {
            Mode::Json
        } else {
            Mode::Human
        }
    }
}

/// A command's result: the JSON object and its human rendering.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub json: Value,
    pub human: String,
}

/// A failed command: an agent-facing code (`not_found`, `repo_not_found`,
/// `objects_missing`, `app_unavailable`, `conflict`, `internal`, …) and a
/// message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub code: String,
    pub message: String,
}

impl CliError {
    pub fn new(code: &str, message: impl Into<String>) -> CliError {
        CliError {
            code: code.to_owned(),
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> CliError {
        CliError::new("internal", message)
    }

    /// The JSON-mode error object.
    pub fn to_json(&self) -> Value {
        json!({ "error": { "code": self.code, "message": self.message } })
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

impl From<CoreError> for CliError {
    fn from(e: CoreError) -> CliError {
        CliError::new(e.code(), e.to_string())
    }
}

impl From<ApiError> for CliError {
    fn from(e: ApiError) -> CliError {
        CliError::new(e.code.as_str(), e.message)
    }
}

impl From<anyhow::Error> for CliError {
    fn from(e: anyhow::Error) -> CliError {
        CliError::internal(format!("{e:#}"))
    }
}

/// Prints `report` in `mode` on stdout.
pub fn print_report(mode: Mode, report: &Report) {
    match mode {
        Mode::Json => print_json(&report.json),
        Mode::Human => {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(report.human.as_bytes());
            if !report.human.ends_with('\n') {
                let _ = out.write_all(b"\n");
            }
        }
    }
}

/// Prints one JSON object and a newline on stdout.
pub fn print_json(value: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = serde_json::to_writer(&mut out, value);
    let _ = out.write_all(b"\n");
}

/// Prints `err` in `mode`: JSON on stdout, or a message on stderr.
pub fn print_error(mode: Mode, err: &CliError) {
    match mode {
        Mode::Json => print_json(&err.to_json()),
        Mode::Human => eprintln!("polygloss: {}", err.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_unless_a_terminal_without_the_flag() {
        assert_eq!(Mode::choose(false, true), Mode::Human);
        assert_eq!(Mode::choose(true, true), Mode::Json);
        assert_eq!(Mode::choose(false, false), Mode::Json);
        assert_eq!(Mode::choose(true, false), Mode::Json);
    }

    #[test]
    fn errors_carry_core_codes() {
        let e: CliError = CoreError::RepoNotFound("abc".into()).into();
        assert_eq!(e.code, "repo_not_found");
        assert_eq!(
            e.to_json(),
            json!({ "error": { "code": "repo_not_found", "message": e.message } })
        );
        let api: CliError = ApiError::internal("boom").into();
        assert_eq!(
            (api.code.as_str(), api.message.as_str()),
            ("internal", "boom")
        );
    }
}
