//! Thin, typed wrapper around the `vm(8)` (vm-bhyve) CLI.
//!
//! Everything the TUI does goes through this module so that:
//! - the command surface is in one place (easy to audit / extend),
//! - parsing is testable without a live hypervisor,
//! - we never shell out ad-hoc from UI code.
//!
//! Design note: `vm list`, `vm info`, `vm switch list` etc. need root to read
//! `/dev/vmm`. We surface that as a clear error rather than a parse failure.

use std::process::Command;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VmError {
    #[error("failed to run `vm`: {0}")]
    Io(#[from] std::io::Error),
    #[error("`vm {args}` exited {status}: {stderr}")]
    Command {
        args: String,
        status: i32,
        stderr: String,
    },
    #[error("could not parse `vm {args}` output: {detail}")]
    Parse { args: String, detail: String },
}

/// One row of `vm list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmSummary {
    pub name: String,
    pub datastore: String,
    pub loader: String,
    pub cpu: String,
    pub memory: String,
    pub vnc: String,
    pub autostart: String,
    pub state: String,
}

impl VmSummary {
    /// True when the guest is running (vm-bhyve reports `Running (pid)`).
    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        self.state.starts_with("Running")
    }
}

/// Handle to the `vm` binary. `bin` is overridable for tests.
#[derive(Debug, Clone)]
pub struct Vm {
    bin: String,
}

impl Default for Vm {
    fn default() -> Self {
        Self {
            bin: "vm".to_string(),
        }
    }
}

impl Vm {
    #[allow(dead_code)]
    pub fn new(bin: impl Into<String>) -> Self {
        Self { bin: bin.into() }
    }

    /// Run `vm <args>` and return stdout, erroring on non-zero exit.
    pub fn run(&self, args: &[&str]) -> Result<String, VmError> {
        let out = Command::new(&self.bin).args(args).output()?;
        if !out.status.success() {
            return Err(VmError::Command {
                args: args.join(" "),
                status: out.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// `vm list` — parse the tabular summary of all guests.
    pub fn list(&self) -> Result<Vec<VmSummary>, VmError> {
        let raw = self.run(&["list"])?;
        parse_list(&raw).ok_or_else(|| VmError::Parse {
            args: "list".to_string(),
            detail: "missing or malformed header row".to_string(),
        })
    }
}

/// Parse `vm list` output into rows.
///
/// The header is:
/// `NAME  DATASTORE  LOADER  CPU  MEMORY  VNC  AUTO  STATE`
/// Rows are whitespace-separated; STATE may contain spaces (e.g. `Locked (workstation)`),
/// so we split off the first 7 fields and treat the remainder as STATE.
pub fn parse_list(raw: &str) -> Option<Vec<VmSummary>> {
    let mut lines = raw.lines().filter(|l| !l.trim().is_empty());
    let header = lines.next()?;
    if !header.starts_with("NAME") || !header.contains("STATE") {
        return None;
    }

    let mut rows = Vec::new();
    for line in lines {
        let mut parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 8 {
            continue;
        }
        // AUTO may be two tokens for autostart guests: `Yes [2]` (boot order).
        let mut autostart = parts[6].to_string();
        if parts.len() > 8 && parts[7].starts_with('[') && parts[7].ends_with(']') {
            autostart = format!("{} {}", parts[6], parts[7]);
            parts.remove(7);
        }
        // STATE is everything from the 8th token onward (may contain spaces).
        let state = parts.split_off(7).join(" ");
        rows.push(VmSummary {
            name: parts[0].to_string(),
            datastore: parts[1].to_string(),
            loader: parts[2].to_string(),
            cpu: parts[3].to_string(),
            memory: parts[4].to_string(),
            vnc: parts[5].to_string(),
            autostart,
            state,
        });
    }
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "NAME    DATASTORE  LOADER     CPU  MEMORY  VNC  AUTO  STATE\n\
testvm  default    bhyveload  1    256M    -    No    Locked (workstation)\n\
alpha   default    uefi       2    1G      -    Yes   Running (1234)\n";

    #[test]
    fn parses_rows_and_state_with_spaces() {
        let rows = parse_list(SAMPLE).expect("header present");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "testvm");
        assert_eq!(rows[0].state, "Locked (workstation)");
        assert!(!rows[0].is_running());
        assert_eq!(rows[1].state, "Running (1234)");
        assert!(rows[1].is_running());
    }

    #[test]
    fn autostart_order_is_not_part_of_state() {
        let raw = "NAME             DATASTORE  LOADER     CPU  MEMORY  VNC  AUTO     STATE\n\
qtumvalidator10  default    uefi       2    4096M   -    Yes [2]  Running (90212)\n";
        let rows = parse_list(raw).unwrap();
        assert_eq!(rows[0].autostart, "Yes [2]");
        assert_eq!(rows[0].state, "Running (90212)");
    }

    #[test]
    fn rejects_garbage_header() {
        assert!(parse_list("this is not a table\nfoo bar\n").is_none());
    }

    #[test]
    fn empty_list_is_empty_vec() {
        let rows = parse_list("NAME    DATASTORE  LOADER     CPU  MEMORY  VNC  AUTO  STATE\n")
            .expect("header present");
        assert!(rows.is_empty());
    }
}
