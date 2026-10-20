//! High-level VM actions the TUI exposes, built on `vmbhyve::Vm`.
//!
//! Each action maps to a `vm(8)` subcommand. We keep them as small, testable
//! functions that return a human-readable outcome string so the UI (and the
//! assistant layer) can surface exactly what happened.

use crate::vmbhyve::{Vm, VmError, VmSummary};

/// The set of lifecycle actions we expose in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    Suspend,
    Console,
    Info,
}

impl Action {
    /// All actions, in menu order.
    pub const ALL: [Action; 6] = [
        Action::Start,
        Action::Stop,
        Action::Restart,
        Action::Suspend,
        Action::Console,
        Action::Info,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Action::Start => "Start",
            Action::Stop => "Stop",
            Action::Restart => "Restart",
            Action::Suspend => "Suspend",
            Action::Console => "Console",
            Action::Info => "Info",
        }
    }

    /// The `vm(8)` argv this action runs against a guest name.
    pub fn argv(&self, name: &str) -> Vec<String> {
        let sub = match self {
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Restart => "restart",
            Action::Suspend => "suspend",
            // `console` is interactive; we surface the command rather than
            // exec it from inside the TUI event loop.
            Action::Console => "console",
            Action::Info => "info",
        };
        vec![sub.to_string(), name.to_string()]
    }

    /// True when running this action would block the UI (interactive attach).
    pub fn is_interactive(&self) -> bool {
        matches!(self, Action::Console)
    }
}

/// Run a lifecycle action against a guest. Returns a one-line outcome.
///
/// Interactive actions (`Console`) are not exec'd here — we return the exact
/// command the user should run, so the TUI never swallows the terminal.
pub fn run_action(vm: &Vm, action: Action, name: &str) -> Result<String, VmError> {
    if action.is_interactive() {
        return Ok(format!("run in a shell: vm {} {name}", action.argv(name)[0]));
    }
    let owned = action.argv(name);
    let argv: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
    let out = vm.run(&argv)?;
    let trimmed = out.trim();
    if trimmed.is_empty() {
        Ok(format!("{}: ok", action.label()))
    } else {
        Ok(format!("{}: {trimmed}", action.label()))
    }
}

/// Convenience: pick the most likely action for a guest's current state.
/// Powers the assistant's default suggestion ("it's stopped → Start").
pub fn suggested_action(row: &VmSummary) -> Action {
    if row.is_running() {
        Action::Stop
    } else {
        Action::Start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: &str) -> VmSummary {
        VmSummary {
            name: "testvm".into(),
            datastore: "zroot/vm".into(),
            loader: "bhyveload".into(),
            cpu: "2".into(),
            memory: "256M".into(),
            vnc: "-".into(),
            autostart: "No".into(),
            state: state.into(),
        }
    }

    #[test]
    fn argv_maps_to_vm_subcommands() {
        assert_eq!(Action::Start.argv("x"), vec!["start", "x"]);
        assert_eq!(Action::Stop.argv("x"), vec!["stop", "x"]);
        assert_eq!(Action::Restart.argv("x"), vec!["restart", "x"]);
        assert_eq!(Action::Suspend.argv("x"), vec!["suspend", "x"]);
    }

    #[test]
    fn console_is_interactive_and_not_execd() {
        assert!(Action::Console.is_interactive());
        // run_action must NOT shell out for console — it returns the command.
        let vm = Vm::new("/nonexistent/vm");
        let msg = run_action(&vm, Action::Console, "testvm").expect("no exec");
        assert!(msg.contains("vm console testvm"));
    }

    #[test]
    fn suggested_action_follows_state() {
        assert_eq!(suggested_action(&row("Running (1)")), Action::Stop);
        assert_eq!(suggested_action(&row("Stopped")), Action::Start);
    }
}
