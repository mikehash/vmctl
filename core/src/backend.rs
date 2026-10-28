//! The result of running one host command.
//!
//! Every front-end action ends as an `ExecResult`: the exact command line, its
//! real exit code, and its real stdout/stderr. Nothing is synthesised except
//! spawn failures (exit 127, OS error as stderr) and predicted guard refusals,
//! which are labelled "(not run)" by the caller.

/// Result of running one `vm(8)` command. Mirrors a real process exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub cmd: String,
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ExecResult {
    pub fn ok(cmd: impl Into<String>) -> Self {
        Self { cmd: cmd.into(), code: 0, stdout: String::new(), stderr: String::new() }
    }
    pub fn fail(cmd: impl Into<String>, code: i32, stderr: impl Into<String>) -> Self {
        Self { cmd: cmd.into(), code, stdout: String::new(), stderr: stderr.into() }
    }
    pub fn success(&self) -> bool {
        self.code == 0
    }
}
