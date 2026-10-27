//! Assistant v2 — plain English → intent → validated plan of typed [`Op`]s.
//!
//! Unlike v1 (`assistant.rs`, still used by the TUI), this plans against the
//! full [`Inventory`]: it knows guest states, templates, switches, the host's
//! memory and the activity log, so it can skip guests that would fail, quote
//! the real stderr of the last failure, and fill defaults from the template.
//!
//! Nothing here executes anything. Plans are run step-by-step by the caller,
//! stopping at the first non-zero exit (see [`Plan::run`]).

use crate::activity::Entry;
use crate::backend::ExecResult;
use crate::inventory::{Hypervisor, Inventory};
use crate::model::{fmt_mib, next_name, stamp, VmState};
use crate::ops::Op;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    pub key: String,
    pub value: String,
    /// True when the value is our default rather than something the user said.
    pub filled: bool,
}

fn chip(k: &str, v: impl Into<String>, filled: bool) -> Chip {
    Chip { key: k.into(), value: v.into(), filled }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub title: String,
    pub op: Op,
    pub effect: String,
}

impl PlanStep {
    fn new(op: Op, effect: impl Into<String>) -> Self {
        Self { title: op.title(), op, effect: effect.into() }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reply {
    pub text: String,
    pub stderr: Option<String>,
    pub chips: Vec<Chip>,
    pub code: Vec<String>,
    pub steps: Vec<PlanStep>,
    pub skipped: Vec<String>,
    pub risk: String,
    pub touches: String,
    pub mem_after: String,
    pub tail: Option<String>,
    pub follow: Vec<String>,
}

impl Reply {
    fn say(t: impl Into<String>) -> Self {
        Self { text: t.into(), ..Default::default() }
    }
    pub fn has_plan(&self) -> bool {
        !self.steps.is_empty()
    }
}

fn has_word(t: &str, words: &[&str]) -> bool {
    t.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| words.contains(&w))
}

fn num_before(t: &str, units: &[&str]) -> Option<u64> {
    let toks: Vec<&str> = t.split(|c: char| c.is_whitespace() || c == ',').filter(|s| !s.is_empty()).collect();
    for (i, tok) in toks.iter().enumerate() {
        let digits: String = tok.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        let rest = &tok[digits.len()..];
        let unit = if rest.is_empty() { toks.get(i + 1).copied().unwrap_or("") } else { rest };
        if units.iter().any(|u| unit == *u) {
            return digits.parse().ok();
        }
    }
    None
}

fn mem_after(inv: &Inventory, delta_mib: i64) -> String {
    let (_, used) = inv.allocated();
    let after = (used as i64 + delta_mib).max(0) as u64;
    format!("{}/{}", fmt_mib(after), fmt_mib(inv.host.mem_mib))
}

pub fn examples() -> Vec<String> {
    vec![
        "make a 4GB ubuntu VM with 2 CPUs and snapshot it before installing".into(),
        "snapshot everything that's running".into(),
        "why did my last command fail?".into(),
        "how do I add a second disk?".into(),
    ]
}

/// Plan a reply for `text` given the current inventory, the selected guest
/// (context), and the activity log (newest first).
pub fn plan(text: &str, inv: &Inventory, selected: Option<&str>, log: &[Entry]) -> Reply {
    let t = text.to_lowercase();
    let t = t.trim();
    let mentioned = inv.guests.iter().filter(|g| t.contains(&g.name.to_lowercase())).max_by_key(|g| g.name.len());
    let running: Vec<_> = inv.guests.iter().filter(|g| g.state.is_running()).collect();
    let locked: Vec<_> = inv.guests.iter().filter(|g| matches!(g.state, VmState::Locked(_))).collect();
    let ctx = mentioned.or_else(|| selected.and_then(|s| inv.guest(s)));
    let ts = crate::model::now();

    if !inv.privilege.can_write() && !t.starts_with("how") && !t.starts_with("why") {
        let mut r = Reply::say("vm(8) needs root and I don't have it, so I can explain things but can't run anything.");
        r.code = vec!["sudo vmctl-desktop".into()];
        r.tail = Some("Relaunch with root (or set up passwordless sudo for vm), then ask again.".into());
        return r;
    }

    // ── create ──
    let wants_create = has_word(t, &["make", "create", "new", "spin", "build", "provision"]) || t.contains("set up");
    let tpl_hit = inv.templates.iter().find(|x| x.id != "default" && t.contains(&x.id));
    if wants_create && (has_word(t, &["vm", "vms", "machine", "box", "guest"]) || tpl_hit.is_some()) {
        let Some(tpl) = tpl_hit.or_else(|| inv.templates.iter().find(|x| x.id != "default")).or(inv.templates.first()) else {
            return Reply::say("This host has no vm-bhyve templates in <datastore>/.templates, so there's nothing to create from yet.");
        };
        let mem_gb = num_before(t, &["g", "gb", "gig", "gigs", "gib"]);
        let cpu = num_before(t, &["cpu", "cpus", "core", "cores", "vcpu", "vcpus"]);
        let disk_gb = num_before(t, &["disk", "g-disk"]).unwrap_or(20) as u32;
        let mem_mib = mem_gb.map(|g| g * 1024).unwrap_or(tpl.mem_mib);
        let cpu_n = cpu.map(|c| c as u32).unwrap_or(tpl.cpu);
        let name = next_name(&tpl.id, &inv.names());
        let ds = inv.datastores.first().map(|d| d.name.clone()).unwrap_or_else(|| "default".into());
        let mut steps = vec![PlanStep::new(
            Op::Create { name: name.clone(), template: tpl.id.clone(), datastore: ds.clone(), disk_gb, cpu: cpu_n, mem_mib },
            format!("New {disk_gb}G disk in {ds}"),
        )];
        let snap_first = t.contains("snapshot");
        if snap_first {
            steps.push(PlanStep::new(Op::Snapshot { name: name.clone(), snap: "pre-install".into(), force: false }, "ZFS snapshot, ~0 bytes"));
        }
        let iso = inv.isos.iter().find(|i| i.name.to_lowercase().contains(&tpl.id)).map(|i| i.name.clone());
        let mut tail = None;
        let wants_install = !has_word(t, &["noinstall"]) && !t.contains("don't install") && !t.contains("without install");
        if wants_install {
            match &iso {
                Some(i) => steps.push(PlanStep::new(Op::Install { name: name.clone(), iso: i.clone() }, "Boots the guest from the ISO")),
                None => tail = Some(format!("I didn't find a {} ISO in the datastore's .iso folder or ~/installers, so I left out the install step.", tpl.id)),
            }
        }
        let mut chips = vec![chip("create", "", false), chip("template", &tpl.id, false), chip("memory", fmt_mib(mem_mib), mem_gb.is_none()), chip("cpu", cpu_n.to_string(), cpu.is_none())];
        if snap_first {
            chips.push(chip("snapshot before install", "", false));
        }
        chips.extend([chip("disk", format!("{disk_gb}G"), true), chip("datastore", &ds, true), chip("name", &name, true)]);
        if let Some(i) = &iso {
            chips.push(chip("iso", i, true));
        }
        return Reply {
            text: "Here's what I understood. Solid values came from you; dashed ones are my defaults from the template.".into(),
            chips,
            steps,
            risk: "Low".into(),
            touches: "Nothing existing".into(),
            mem_after: mem_after(inv, if wants_install && iso.is_some() { mem_mib as i64 } else { 0 }),
            tail,
            ..Default::default()
        };
    }

    // ── snapshot all running ──
    if t.contains("snapshot") && has_word(t, &["running", "everything", "all"]) {
        let snap = format!("pre-{}", stamp(ts));
        let steps: Vec<_> = running
            .iter()
            .map(|g| PlanStep::new(Op::Snapshot { name: g.name.clone(), snap: snap.clone(), force: true }, "Crash-consistent ZFS snapshot"))
            .collect();
        if steps.is_empty() {
            return Reply::say("Nothing is running right now, so there's nothing to snapshot live.");
        }
        return Reply {
            text: format!("{} machine(s) are running. -f snapshots a live disk, which is like a power cut: fine for rollback, not a clean backup.", steps.len()),
            chips: vec![chip("snapshot", "", false), chip("targets", running.iter().map(|g| g.name.as_str()).collect::<Vec<_>>().join(", "), false)],
            skipped: locked.iter().map(|g| format!("{} — locked by another host", g.name)).collect(),
            steps,
            risk: "Low".into(),
            touches: format!("{} running", running.len()),
            mem_after: "no change".into(),
            ..Default::default()
        };
    }

    // ── stop all / start all ──
    let all = has_word(t, &["all", "everything"]);
    if all && (has_word(t, &["stop", "shutdown"]) || t.contains("shut down")) {
        let steps: Vec<_> = running.iter().map(|g| PlanStep::new(Op::Stop(g.name.clone()), "Clean ACPI shutdown")).collect();
        if steps.is_empty() {
            return Reply::say("Nothing is running.");
        }
        let freed: u64 = running.iter().map(|g| g.mem_mib).sum();
        return Reply {
            text: "I'll ask each running guest to shut down cleanly. Nothing is forced.".into(),
            chips: vec![chip("stop", "", false), chip("targets", running.iter().map(|g| g.name.as_str()).collect::<Vec<_>>().join(", "), false)],
            skipped: locked.iter().map(|g| format!("{} — locked by another host", g.name)).collect(),
            steps,
            risk: "Medium".into(),
            touches: format!("{} running", running.len()),
            mem_after: mem_after(inv, -(freed as i64)),
            ..Default::default()
        };
    }
    if all && has_word(t, &["start", "boot"]) {
        let stopped: Vec<_> = inv.guests.iter().filter(|g| matches!(g.state, VmState::Stopped | VmState::Suspended)).collect();
        if stopped.is_empty() {
            return Reply::say("Nothing is stopped right now.");
        }
        let add: u64 = stopped.iter().filter(|g| g.state == VmState::Stopped).map(|g| g.mem_mib).sum();
        return Reply {
            text: format!("{} stopped machine(s) to start.", stopped.len()),
            chips: vec![chip("start", "", false), chip("targets", stopped.iter().map(|g| g.name.as_str()).collect::<Vec<_>>().join(", "), false)],
            steps: stopped.iter().map(|g| PlanStep::new(Op::Start(g.name.clone()), format!("{} vCPU · {}", g.cpu, fmt_mib(g.mem_mib)))).collect(),
            skipped: locked.iter().map(|g| format!("{} — locked by another host", g.name)).collect(),
            risk: "Low".into(),
            touches: "Stopped only".into(),
            mem_after: mem_after(inv, add as i64),
            ..Default::default()
        };
    }

    if t.starts_with("leave it") {
        return Reply::say("Leaving it alone. Nothing was run.");
    }

    // ── stale lock: host is gone ──
    if has_word(t, &["offline", "gone", "dead", "down"]) && !locked.is_empty() {
        let g = mentioned.filter(|g| matches!(g.state, VmState::Locked(_))).unwrap_or(locked[0]);
        let VmState::Locked(h) = &g.state else { unreachable!() };
        return Reply {
            text: format!("If {h} is really gone, the lock is stale. Removing it is only safe in that case — check {h} isn't reachable first, or you'll boot the same disk twice."),
            chips: vec![chip("unlock", "", false), chip("target", &g.name, false), chip("then start", &g.name, true)],
            steps: vec![
                PlanStep::new(Op::Unlock { name: g.name.clone(), lock: g.dir.join("run.lock") }, "Frees the machine for this host"),
                PlanStep::new(Op::Start(g.name.clone()), format!("{} vCPU · {}", g.cpu, fmt_mib(g.mem_mib))),
            ],
            risk: "Medium".into(),
            touches: g.name.clone(),
            mem_after: mem_after(inv, g.mem_mib as i64),
            ..Default::default()
        };
    }

    // ── explain: why / error / lock ──
    if has_word(t, &["why", "fail", "failed", "error", "lock", "locked", "explain"]) {
        if let Some(g) = ctx.filter(|g| matches!(g.state, VmState::Locked(_))).or_else(|| if ctx.is_none() { locked.first().copied() } else { None }) {
            let VmState::Locked(h) = &g.state else { unreachable!() };
            let last = log.iter().find(|e| e.code != 0 && e.cmd.contains(&g.name));
            return Reply {
                text: format!("{} can't start here because another host already has it.", g.name),
                stderr: Some(last.map(|e| e.stderr.clone()).unwrap_or_else(|| format!("{}/run.lock: held by {h}", g.dir.display()))),
                tail: Some(format!("That lock means {h} started it from a shared datastore. If {h} is really gone, removing the lock is safe. I haven't touched anything.")),
                follow: vec![format!("{h} is offline — plan the fix"), "Leave it".into()],
                ..Default::default()
            };
        }
        let last = log.iter().find(|e| e.code != 0 && ctx.map(|g| e.cmd.contains(&g.name)).unwrap_or(true));
        if let Some(e) = last {
            let hint = explain_stderr(&e.stderr);
            return Reply {
                text: format!("`{}` exited {}. The real stderr was:", e.cmd, e.code),
                stderr: Some(e.stderr.clone()),
                tail: Some(hint),
                ..Default::default()
            };
        }
        return Reply::say(match ctx {
            Some(g) => format!("{} looks healthy — no failed commands for it in the activity log.", g.name),
            None => "I don't see any failed commands in the activity log.".into(),
        });
    }

    // ── how-to ──
    if has_word(t, &["how"]) {
        let n = ctx.map(|g| g.name.clone()).unwrap_or_else(|| "<name>".into());
        if t.contains("disk") {
            let mut r = Reply::say("Add a second disk with vm add. Stop the machine first; the disk shows up on next boot.");
            r.code = vec![format!("vm stop {n}"), format!("vm add -d disk -s 20G {n}")];
            r.tail = Some("Want me to plan it? Say \"add a 20G disk to <name>\".".into());
            return r;
        }
        if has_word(t, &["network", "nic", "switch"]) {
            let sw = inv.switches.first().map(|s| s.name.clone()).unwrap_or_else(|| "public".into());
            let mut r = Reply::say("Attach another NIC to a switch with vm add:");
            r.code = vec![format!("vm add -d network -s {sw} {n}")];
            r.tail = Some("It appears as vtnet1 inside the guest after a restart.".into());
            return r;
        }
        if t.contains("console") {
            let mut r = Reply::say("Serial consoles are nmdm devices. Attach from a terminal (detach with ~. ):");
            r.code = vec![format!("vm console {n}")];
            return r;
        }
        if t.contains("iso") {
            let mut r = Reply::say("Drop install ISOs into the datastore's .iso folder (or ~/installers) and they show up in the wizard. vm can also fetch one:");
            r.code = vec!["vm iso https://download.freebsd.org/…/FreeBSD-15.1-RELEASE-amd64-disc1.iso".into()];
            return r;
        }
    }

    // ── add disk (actionable) ──
    if t.contains("disk") && has_word(t, &["add", "attach"]) {
        if let Some(g) = ctx {
            let size = num_before(t, &["g", "gb"]).unwrap_or(20) as u32;
            let mut steps = Vec::new();
            if g.state.is_running() {
                steps.push(PlanStep::new(Op::Stop(g.name.clone()), "Clean ACPI shutdown"));
            }
            steps.push(PlanStep::new(Op::AddDisk { name: g.name.clone(), size_gb: size }, format!("{size}G sparse image")));
            return Reply {
                text: format!("Adding a {size}G disk to {}.", g.name),
                chips: vec![chip("add disk", format!("{size}G"), false), chip("target", &g.name, mentioned.is_none())],
                steps,
                risk: "Low".into(),
                touches: g.name.clone(),
                mem_after: "—".into(),
                ..Default::default()
            };
        }
    }

    // ── single-guest verbs ──
    if let Some(g) = mentioned {
        let single = |op: Op, effect: &str, risk: &str, text: String| Reply {
            text,
            chips: vec![chip(&op.title().split(' ').next().unwrap_or("").to_lowercase(), "", false), chip("target", &g.name, false)],
            steps: vec![PlanStep::new(op, effect)],
            risk: risk.into(),
            touches: g.name.clone(),
            mem_after: "—".into(),
            ..Default::default()
        };
        if has_word(t, &["restart", "reboot"]) {
            return single(Op::Restart(g.name.clone()), "Guest reboots", "Medium", format!("Restarting {}.", g.name));
        }
        if has_word(t, &["start", "boot", "run", "resume"]) {
            return single(Op::Start(g.name.clone()), &format!("{} vCPU · {}", g.cpu, fmt_mib(g.mem_mib)), "Low", format!("Starting {}.", g.name));
        }
        if has_word(t, &["stop", "shutdown"]) || t.contains("shut down") {
            return single(Op::Stop(g.name.clone()), "Clean ACPI shutdown", "Medium", format!("Stopping {} cleanly.", g.name));
        }
        if t.contains("snapshot") {
            return single(
                Op::Snapshot { name: g.name.clone(), snap: format!("manual-{}", stamp(ts)), force: g.state.is_running() },
                if g.state.is_running() { "Crash-consistent ZFS snapshot" } else { "ZFS snapshot" },
                "Low",
                format!("Snapshotting {}.", g.name),
            );
        }
    }

    let mut r = Reply::say("I can create, start, stop, snapshot and explain errors. Try one of these:");
    r.follow = examples();
    r
}

/// Turn common vm-bhyve stderr into a plain-language hint (F-404).
pub fn explain_stderr(e: &str) -> String {
    let l = e.to_lowercase();
    if l.contains("only be managed by root") {
        "vm(8) needs root. Relaunch vmctl with sudo (or doas).".into()
    } else if l.contains("unable to locate iso") || l.contains("unable to locate iso file") {
        "The ISO isn't where vm(8) looks. Put it in the datastore's .iso folder or pass a full path.".into()
    } else if l.contains("powered off first") || l.contains("must be stopped") || l.contains("running; please stop") {
        "The guest has to be stopped first (snapshots can use -f to take a crash-consistent copy).".into()
    } else if l.contains("does not support suspend") {
        "This kernel's bhyve wasn't built with BHYVE_SNAPSHOT, so suspend isn't available. Stop the guest instead.".into()
    } else if l.contains("already be running") || l.contains("already running") {
        "It's already running — nothing to do. Refresh if the list looks stale.".into()
    } else if l.contains("unable to find template") {
        "That template doesn't exist in <datastore>/.templates. The wizard only offers the ones that do.".into()
    } else if l.contains("locked") {
        "Another host holds the run.lock. Only remove it if that host is really gone.".into()
    } else if l.contains("more recent snapshots") {
        "Rolling back past newer snapshots needs -r, which deletes them. vmctl's rollback runs `zfs rollback -r` and asks first.".into()
    } else {
        "That's the real stderr from the command. Ask me to fix it and I'll plan the steps.".into()
    }
}

/// Result of running a plan.
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub results: Vec<ExecResult>,
    pub failed_at: Option<usize>,
}

/// Run steps in order against a hypervisor, calling `each` after every step.
/// Stops at the first non-zero exit; later steps do not run.
pub fn run_steps(hv: &mut dyn Hypervisor, steps: &[Op], mut each: impl FnMut(usize, &ExecResult)) -> RunOutcome {
    let mut out = RunOutcome::default();
    for (i, op) in steps.iter().enumerate() {
        let r = hv.run(op.program(), &op.argv());
        each(i, &r);
        let failed = r.code != 0;
        out.results.push(r);
        if failed {
            out.failed_at = Some(i);
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::DemoHost;

    #[test]
    fn create_fills_defaults_from_template() {
        let inv = DemoHost::new().inventory();
        let r = plan("make a 4GB Ubuntu VM with 2 CPUs and snapshot it before installing", &inv, None, &[]);
        assert_eq!(r.steps.len(), 3);
        match &r.steps[0].op {
            Op::Create { template, mem_mib, cpu, name, .. } => {
                assert_eq!(template, "ubuntu");
                assert_eq!(*mem_mib, 4096);
                assert_eq!(*cpu, 2);
                assert_eq!(name, "ubuntu-01");
            }
            o => panic!("{o:?}"),
        }
        assert!(matches!(r.steps[1].op, Op::Snapshot { .. }));
        assert!(matches!(r.steps[2].op, Op::Install { .. }));
        assert!(r.chips.iter().any(|c| c.key == "memory" && !c.filled));
        assert!(r.chips.iter().any(|c| c.key == "disk" && c.filled));
    }

    #[test]
    fn snapshot_running_skips_locked() {
        let inv = DemoHost::new().inventory();
        let r = plan("snapshot everything that's running", &inv, None, &[]);
        assert_eq!(r.steps.len(), 3);
        assert_eq!(r.skipped.len(), 1);
    }

    #[test]
    fn explains_lock_and_offers_fix() {
        let inv = DemoHost::new().inventory();
        let r = plan("why is alpine-test locked?", &inv, None, &[]);
        assert!(r.stderr.is_some());
        assert!(r.steps.is_empty());
        let fix = plan(&r.follow[0], &inv, None, &[]);
        assert_eq!(fix.steps.len(), 2);
        assert!(matches!(fix.steps[0].op, Op::Unlock { .. }));
    }

    #[test]
    fn quotes_real_stderr_of_last_failure() {
        let inv = DemoHost::new().inventory();
        let log = vec![Entry { ts: 1, source: "desktop".into(), cmd: "vm start win11-dev".into(), code: 1, stderr: "unable to locate iso file - 'x.iso'".into() }];
        let r = plan("why did it fail", &inv, Some("win11-dev"), &log);
        assert_eq!(r.stderr.as_deref(), Some("unable to locate iso file - 'x.iso'"));
        assert!(r.tail.unwrap().contains(".iso folder"));
    }

    #[test]
    fn run_stops_at_first_failure() {
        let mut h = DemoHost::new();
        let steps = vec![Op::Start("win11-dev".into()), Op::Start("win11-dev".into()), Op::Stop("win11-dev".into())];
        let mut seen = 0;
        let out = run_steps(&mut h, &steps, |_, _| seen += 1);
        assert_eq!(out.failed_at, Some(1));
        assert_eq!(seen, 2);
        assert!(!out.results[1].stderr.is_empty());
    }

    #[test]
    fn unknown_offers_examples() {
        let inv = DemoHost::new().inventory();
        assert!(!plan("asdf", &inv, None, &[]).follow.is_empty());
    }
}
