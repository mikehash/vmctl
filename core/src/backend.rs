//! Backend abstraction over `vm(8)`.
//!
//! The assistant engine and the UI both drive VMs through a `Backend`, never
//! through `std::process::Command` directly. That gives us two implementations:
//!
//! - `VmBackend` — the real thing, shelling out to `vm(8)` (via `sudo`).
//! - `MockBackend` — a deterministic in-memory hypervisor for tests and demos.
//!
//! Both return the same `ExecResult` so the plan runner behaves identically
//! against either: a non-zero exit stops the plan and preserves real stderr.

use std::collections::BTreeMap;

use crate::vmbhyve::{Vm, VmError, VmSummary};

/// Result of running one `vm(8)` command. Mirrors a real process exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub cmd: String,
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ExecResult {
    #[allow(dead_code)]
    pub fn ok(cmd: impl Into<String>) -> Self {
        Self { cmd: cmd.into(), code: 0, stdout: String::new(), stderr: String::new() }
    }
    pub fn fail(cmd: impl Into<String>, code: i32, stderr: impl Into<String>) -> Self {
        Self { cmd: cmd.into(), code, stdout: String::new(), stderr: stderr.into() }
    }
    #[allow(dead_code)]
    pub fn success(&self) -> bool {
        self.code == 0
    }
}

/// Anything that can run `vm(8)`-style lifecycle commands and list guests.
pub trait Backend {
    /// Run a raw `vm` subcommand with argv, e.g. `["start", "testvm"]`.
    fn exec(&mut self, argv: &[&str]) -> ExecResult;
    /// Current guest list.
    fn list(&mut self) -> Result<Vec<VmSummary>, VmError>;
}

/// The live backend: shells out to `vm(8)`.
pub struct VmBackend {
    vm: Vm,
    /// When true, prefix commands with `sudo` (vm-bhyve needs root).
    sudo: bool,
}

impl Default for VmBackend {
    fn default() -> Self {
        Self { vm: Vm::default(), sudo: true }
    }
}

impl VmBackend {
    #[allow(dead_code)]
    pub fn new(vm: Vm, sudo: bool) -> Self {
        Self { vm, sudo }
    }
}

impl Backend for VmBackend {
    fn exec(&mut self, argv: &[&str]) -> ExecResult {
        let cmd = format!("vm {}", argv.join(" "));
        // `Vm::run` returns Err on non-zero; we want the exit code either way,
        // so we go through the raw command here.
        let mut full: Vec<&str> = Vec::new();
        if self.sudo {
            full.push("sudo");
        }
        full.push("vm");
        full.extend_from_slice(argv);
        match std::process::Command::new(full[0]).args(&full[1..]).output() {
            Ok(out) => ExecResult {
                cmd,
                code: out.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            },
            Err(e) => ExecResult::fail(cmd, 127, format!("failed to run `vm`: {e}")),
        }
    }

    fn list(&mut self) -> Result<Vec<VmSummary>, VmError> {
        self.vm.list()
    }
}

/// A deterministic in-memory hypervisor for tests and demos.
///
/// State machine mirrors vm-bhyve's guards so the assistant's plans are
/// validated against realistic preconditions (e.g. can't start a running guest).
#[derive(Debug, Clone, Default)]
pub struct MockBackend {
    pub vms: BTreeMap<String, VmSummary>,
    /// Force the next matching command to fail (for testing the stop-at-failure path).
    pub fail_next: Option<(String, i32, String)>,
    /// Every command executed, in order (for asserting the plan ran).
    pub history: Vec<String>,
}

impl MockBackend {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    fn row(name: &str, state: &str) -> VmSummary {
        VmSummary {
            name: name.to_string(),
            datastore: "default".into(),
            loader: "bhyveload".into(),
            cpu: "1".into(),
            memory: "256M".into(),
            vnc: "-".into(),
            autostart: "No".into(),
            state: state.to_string(),
        }
    }

    /// Seed a guest in a given state.
    #[allow(dead_code)]
    pub fn with(mut self, name: &str, state: &str) -> Self {
        self.vms.insert(name.to_string(), Self::row(name, state));
        self
    }

    fn set_state(&mut self, name: &str, state: &str) {
        if let Some(v) = self.vms.get_mut(name) {
            v.state = state.to_string();
        }
    }
}

impl Backend for MockBackend {
    fn exec(&mut self, argv: &[&str]) -> ExecResult {
        let cmd = format!("vm {}", argv.join(" "));
        self.history.push(cmd.clone());

        if let Some((needle, code, stderr)) = self.fail_next.clone() {
            if cmd.contains(&needle) {
                self.fail_next = None;
                return ExecResult::fail(cmd, code, stderr);
            }
        }

        let (op, name) = match argv {
            [op, name, ..] => (*op, *name),
            [op] => (*op, ""),
            [] => return ExecResult::fail(cmd, 1, "vm: no arguments"),
        };

        // `create` and `list` don't require an existing guest.
        if op == "list" {
            return ExecResult::ok(cmd);
        }
        if op == "create" {
            if self.vms.contains_key(name) {
                return ExecResult::fail(cmd, 1, format!("vm: {name}: already exists"));
            }
            self.vms.insert(name.to_string(), Self::row(name, "Stopped"));
            return ExecResult::ok(cmd);
        }

        let state = self.vms.get(name).map(|v| v.state.clone());
        let state = match state {
            Some(s) => s,
            None => return ExecResult::fail(cmd, 1, format!("vm: {name}: unable to locate virtual machine")),
        };

        match op {
            "start" | "install" => {
                if state.starts_with("Running") {
                    return ExecResult::fail(cmd, 1, format!("vm: {name}: virtual machine is already running"));
                }
                self.set_state(name, "Running (4242)");
                ExecResult::ok(cmd)
            }
            "stop" | "poweroff" => {
                if !state.starts_with("Running") {
                    return ExecResult::fail(cmd, 1, format!("vm: {name}: virtual machine is not running"));
                }
                self.set_state(name, "Stopped");
                ExecResult::ok(cmd)
            }
            "restart" => {
                if !state.starts_with("Running") {
                    return ExecResult::fail(cmd, 1, format!("vm: {name}: virtual machine is not running"));
                }
                ExecResult::ok(cmd)
            }
            "suspend" => {
                if !state.starts_with("Running") {
                    return ExecResult::fail(cmd, 1, format!("vm: {name}: virtual machine is not running"));
                }
                self.set_state(name, "Suspended");
                ExecResult::ok(cmd)
            }
            "resume" => {
                if state != "Suspended" {
                    return ExecResult::fail(cmd, 1, format!("vm: {name}: virtual machine is not suspended"));
                }
                self.set_state(name, "Running (4242)");
                ExecResult::ok(cmd)
            }
            "console" | "info" => {
                if op == "console" && !state.starts_with("Running") {
                    return ExecResult::fail(cmd, 1, format!("vm: {name}: virtual machine is not running"));
                }
                ExecResult::ok(cmd)
            }
            "snapshot" | "rollback" | "destroy" => ExecResult::ok(cmd),
            _ => ExecResult::ok(cmd),
        }
    }

    fn list(&mut self) -> Result<Vec<VmSummary>, VmError> {
        Ok(self.vms.values().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_enforces_running_guards() {
        let mut b = MockBackend::new().with("a", "Stopped");
        assert!(b.exec(&["start", "a"]).success());
        // starting again must fail
        assert!(!b.exec(&["start", "a"]).success());
        // stop works now that it runs
        assert!(b.exec(&["stop", "a"]).success());
    }

    #[test]
    fn mock_fails_on_missing_guest() {
        let mut b = MockBackend::new();
        let r = b.exec(&["start", "ghost"]);
        assert!(!r.success());
        assert!(r.stderr.contains("unable to locate"));
    }

    #[test]
    fn mock_fail_next_injects_failure() {
        let mut b = MockBackend::new().with("a", "Stopped").with("b", "Stopped");
        b.fail_next = Some(("start a".into(), 1, "boom".into()));
        let r = b.exec(&["start", "a"]);
        assert_eq!(r.code, 1);
        assert_eq!(r.stderr, "boom");
        // consumed
        assert!(b.exec(&["start", "a"]).success());
    }

    #[test]
    fn mock_create_then_list() {
        let mut b = MockBackend::new();
        assert!(b.exec(&["create", "new"]).success());
        assert_eq!(b.list().unwrap().len(), 1);
    }
}
