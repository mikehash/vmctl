//! Assistant engine — plain English → intent → validated plan → sequential run.
//!
//! This mirrors the prototype's `planFor` + `useRunner`:
//! - `plan_for` turns a request into a `Plan` of `Step`s (each with a title, the
//!   `vm(8)` argv it runs, an undo hint, and a human effect description).
//! - `run_plan` executes the steps in order against a `Backend`, **stopping at
//!   the first non-zero exit and preserving the real stderr**. Nothing after a
//!   failure runs.
//!
//! The plan is validated before it runs: every step's target must be a guest the
//! backend knows about (except `create`, which introduces one). This is the
//! "honest by construction" guarantee — we never run a command we can't justify.

use crate::backend::{Backend, ExecResult};

/// One validated step in a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub title: String,
    /// argv for `vm(8)`, e.g. `["start", "testvm"]`.
    pub argv: Vec<String>,
    /// How to undo this step (advisory, shown to the user).
    pub undo: String,
    /// Human description of the effect.
    pub effect: String,
}

impl Step {
    /// The full command string, e.g. `vm start testvm`.
    pub fn cmd(&self) -> String {
        format!("vm {}", self.argv.join(" "))
    }
}

/// A validated, ordered plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub summary: String,
    pub steps: Vec<Step>,
    pub risk: &'static str,
    pub touches: String,
}

impl Plan {
    fn empty(summary: impl Into<String>) -> Self {
        Self { summary: summary.into(), steps: Vec::new(), risk: "Low", touches: "Nothing".into() }
    }
}

/// Outcome of running a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    pub results: Vec<ExecResult>,
    /// Index of the step that failed, if any. Everything after it did NOT run.
    pub failed_at: Option<usize>,
}

impl RunReport {
    #[allow(dead_code)]
    pub fn success(&self) -> bool {
        self.failed_at.is_none()
    }
}

/// Parse a size/memory token like "4gb", "4G", "512M" into a `vm create -s`/`-m` value.
fn size_token(s: &str) -> Option<String> {
    let t = s.trim().to_lowercase();
    let num: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if num.is_empty() {
        return None;
    }
    let unit: String = t.chars().skip(num.len()).collect();
    let suffix = match unit.as_str() {
        "" | "g" | "gb" | "gig" | "gigs" | "gigabyte" | "gigabytes" => "G",
        "m" | "mb" => "M",
        _ => return None,
    };
    Some(format!("{num}{suffix}"))
}

/// Turn a plain-English request into a validated plan.
///
/// `known` is the set of guest names the backend currently knows about; used to
/// resolve which machine a request targets and to validate targets.
pub fn plan_for(text: &str, known: &[String]) -> Plan {
    let t = text.to_lowercase();

    // "create/make/new ... <os> ... N cpu ... M memory"
    let wants_create = ["create", "make", "new", "spin up", "build", "provision"]
        .iter()
        .any(|w| t.contains(w));
    let is_vm = ["vm", "machine", "box", "guest"].iter().any(|w| t.contains(w));
    if wants_create && is_vm {
        let os = ["ubuntu", "freebsd", "debian", "windows", "openbsd", "alpine"]
            .iter()
            .find(|o| t.contains(**o))
            .copied()
            .unwrap_or("freebsd");
        let cpu = regex_first_num(&t, &["cpu", "cpus", "core", "cores", "vcpu"]).unwrap_or(1);
        let mem = regex_first_size(&t).unwrap_or_else(|| "1G".to_string());
        let name = unique_name(os, known);
        let mut steps = vec![Step {
            title: format!("Create {name}"),
            argv: vec!["create".into(), "-t".into(), os.into(), "-c".into(), cpu.to_string(), "-m".into(), mem.clone(), name.clone()],
            undo: format!("vm destroy {name}"),
            effect: format!("{os} guest, {cpu} vCPU, {mem} memory"),
        }];
        // If the request mentions install/boot from an ISO, add an install step.
        if ["install", "iso", "boot"].iter().any(|w| t.contains(w)) {
            steps.push(Step {
                title: format!("Install {name} from ISO"),
                argv: vec!["install".into(), name.clone()],
                undo: format!("vm destroy {name}"),
                effect: "Boot the installer".into(),
            });
        }
        return Plan {
            summary: format!("Create a new {os} machine named {name}"),
            steps,
            risk: "Low",
            touches: name,
        };
    }

    // Resolve a target guest by name (first known name mentioned in the text).
    let target = known.iter().find(|n| t.contains(n.as_str())).cloned();

    // "start <name>" / "boot <name>"
    if ["start", "boot", "power on", "turn on"].iter().any(|w| t.contains(w)) {
        if let Some(name) = target {
            return Plan {
                summary: format!("Start {name}"),
                steps: vec![Step {
                    title: format!("Start {name}"),
                    argv: vec!["start".into(), name.clone()],
                    undo: format!("vm stop {name}"),
                    effect: "Guest boots".into(),
                }],
                risk: "Low",
                touches: name,
            };
        }
    }

    // "stop <name>" / "shut down <name>"
    if ["stop", "shut down", "shutdown", "power off", "turn off"].iter().any(|w| t.contains(w)) {
        if let Some(name) = target {
            return Plan {
                summary: format!("Stop {name}"),
                steps: vec![Step {
                    title: format!("Stop {name}"),
                    argv: vec!["stop".into(), name.clone()],
                    undo: format!("vm start {name}"),
                    effect: "Clean ACPI shutdown".into(),
                }],
                risk: "Medium",
                touches: name,
            };
        }
    }

    // "restart <name>"
    if ["restart", "reboot"].iter().any(|w| t.contains(w)) {
        if let Some(name) = target {
            return Plan {
                summary: format!("Restart {name}"),
                steps: vec![Step {
                    title: format!("Restart {name}"),
                    argv: vec!["restart".into(), name.clone()],
                    undo: format!("vm stop {name} && vm start {name}"),
                    effect: "Guest reboots".into(),
                }],
                risk: "Medium",
                touches: name,
            };
        }
    }

    // "snapshot <name>"
    if t.contains("snapshot") {
        if let Some(name) = target {
            return Plan {
                summary: format!("Snapshot {name}"),
                steps: vec![Step {
                    title: format!("Snapshot {name}"),
                    argv: vec!["snapshot".into(), name.clone()],
                    undo: format!("vm snapshot -d {name}@<snap>"),
                    effect: "ZFS snapshot".into(),
                }],
                risk: "Low",
                touches: name,
            };
        }
    }

    // "stop all running"
    if ["stop", "shut down", "shutdown"].iter().any(|w| t.contains(w))
        && ["all", "everything", "running"].iter().any(|w| t.contains(w))
    {
        let running: Vec<String> = known.to_vec();
        if !running.is_empty() {
            let steps = running
                .iter()
                .map(|n| Step {
                    title: format!("Stop {n}"),
                    argv: vec!["stop".into(), n.clone()],
                    undo: format!("vm start {n}"),
                    effect: "Clean ACPI shutdown".into(),
                })
                .collect();
            return Plan {
                summary: format!("Stop {} running machine(s)", running.len()),
                steps,
                risk: "Medium",
                touches: running.join(", "),
            };
        }
    }

    Plan::empty("I couldn't map that to a vm(8) action. Try: create, start, stop, restart, or snapshot a machine.")
}

/// Execute a plan in order. Stops at the first non-zero exit; later steps do not run.
pub fn run_plan(backend: &mut dyn Backend, plan: &Plan) -> RunReport {
    let mut results = Vec::new();
    for (i, step) in plan.steps.iter().enumerate() {
        let argv: Vec<&str> = step.argv.iter().map(|s| s.as_str()).collect();
        let r = backend.exec(&argv);
        let failed = !r.success();
        results.push(r);
        if failed {
            return RunReport { results, failed_at: Some(i) };
        }
    }
    RunReport { results, failed_at: None }
}

// ── tiny regex-free helpers (keep the crate dependency-light) ──

fn regex_first_num(t: &str, keys: &[&str]) -> Option<u32> {
    // find "<num> <key>" occurrences
    for key in keys {
        let mut idx = 0usize;
        while let Some(pos) = t[idx..].find(key) {
            let abs = idx + pos;
            // walk backwards to collect digits
            let before = &t[..abs];
            let num: String = before
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit() || *c == ' ')
                .collect::<String>()
                .chars()
                .rev()
                .filter(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = num.parse::<u32>() {
                return Some(n);
            }
            idx = abs + key.len();
        }
    }
    None
}

fn regex_first_size(t: &str) -> Option<String> {
    let mut idx = 0usize;
    let bytes = t.as_bytes();
    while idx < bytes.len() {
        if bytes[idx].is_ascii_digit() {
            let start = idx;
            while idx < bytes.len() && bytes[idx].is_ascii_digit() {
                idx += 1;
            }
            let unit_start = idx;
            while idx < bytes.len() && bytes[idx].is_ascii_alphabetic() {
                idx += 1;
            }
            let num = &t[start..unit_start];
            let unit = &t[unit_start..idx];
            if let Some(s) = size_token(&format!("{num}{unit}")) {
                // only treat as memory if the unit looks like a size
                if matches!(unit, "g" | "gb" | "gig" | "gigs" | "m" | "mb" | "") {
                    return Some(s);
                }
            }
        } else {
            idx += 1;
        }
    }
    None
}

fn unique_name(base: &str, known: &[String]) -> String {
    let mut n = 1u32;
    loop {
        let cand = format!("{base}-{n:02}");
        if !known.contains(&cand) {
            return cand;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MockBackend;

    #[test]
    fn plans_create_with_cpu_and_mem() {
        let p = plan_for("create a 4gb ubuntu vm with 2 cpus", &[]);
        assert_eq!(p.steps.len(), 1);
        let argv = p.steps[0].argv.join(" ");
        assert!(argv.contains("create"));
        assert!(argv.contains("ubuntu"));
        assert!(argv.contains("2")); // cpu
        assert!(argv.contains("4G")); // mem
    }

    #[test]
    fn plans_start_target() {
        let known = vec!["testvm".to_string()];
        let p = plan_for("start testvm", &known);
        assert_eq!(p.steps.len(), 1);
        assert_eq!(p.steps[0].argv, vec!["start", "testvm"]);
    }

    #[test]
    fn run_plan_stops_at_first_failure_and_keeps_stderr() {
        let mut b = MockBackend::new().with("a", "Stopped").with("b", "Stopped");
        b.fail_next = Some(("start a".into(), 1, "a is locked".into()));
        let known = vec!["a".to_string(), "b".to_string()];
        let p = plan_for("stop all running", &known); // 2 stop steps
        // make it a start-all style plan manually to test ordering
        let p = Plan {
            summary: "start both".into(),
            steps: vec![
                Step { title: "Start a".into(), argv: vec!["start".into(), "a".into()], undo: String::new(), effect: String::new() },
                Step { title: "Start b".into(), argv: vec!["start".into(), "b".into()], undo: String::new(), effect: String::new() },
            ],
            risk: "Low",
            touches: "a, b".into(),
        };
        let _ = p;
        let plan = Plan {
            summary: "start both".into(),
            steps: vec![
                Step { title: "Start a".into(), argv: vec!["start".into(), "a".into()], undo: String::new(), effect: String::new() },
                Step { title: "Start b".into(), argv: vec!["start".into(), "b".into()], undo: String::new(), effect: String::new() },
            ],
            risk: "Low",
            touches: "a, b".into(),
        };
        let report = run_plan(&mut b, &plan);
        assert_eq!(report.failed_at, Some(0));
        assert_eq!(report.results.len(), 1); // second step never ran
        assert_eq!(report.results[0].stderr, "a is locked");
    }

    #[test]
    fn run_plan_all_success() {
        let mut b = MockBackend::new().with("a", "Stopped");
        let plan = Plan {
            summary: "start a".into(),
            steps: vec![Step { title: "Start a".into(), argv: vec!["start".into(), "a".into()], undo: String::new(), effect: String::new() }],
            risk: "Low",
            touches: "a".into(),
        };
        let report = run_plan(&mut b, &plan);
        assert!(report.success());
        assert_eq!(report.results.len(), 1);
    }

    #[test]
    fn size_parsing() {
        assert_eq!(size_token("4gb"), Some("4G".into()));
        assert_eq!(size_token("512M"), Some("512M".into()));
        assert_eq!(size_token("2g"), Some("2G".into()));
    }
}
