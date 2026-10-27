//! Typed host operations — the single command surface both front-ends use.
//!
//! An [`Op`] knows the exact argv it runs, how to describe itself, how to undo
//! itself (advisory), and a *predictive* guard that mirrors vm-bhyve's own
//! preconditions. The guard is used to grey out buttons and to explain why;
//! it never replaces the real exit status — every op is still executed for
//! real and its real stderr is what the user sees.

use std::path::PathBuf;

use crate::inventory::{Guest, Inventory};
use crate::model::{fmt_mib, valid_name, VmState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Start(String),
    Stop(String),
    Restart(String),
    Suspend(String),
    Poweroff(String),
    Reset(String),
    Snapshot { name: String, snap: String, force: bool },
    /// `zfs rollback -r <dataset>@snap` — one per dataset of the guest.
    /// Newer snapshots are destroyed with it.
    ///
    /// Not `vm rollback`: vm-bhyve 1.7.5 ends its rollback loop with
    /// `[ $? -ne 0 ] && exit $?`, so the loop's status is that test's (1) and
    /// `vm rollback` exits 1 after every *successful* rollback. Calling zfs
    /// directly keeps the exit status honest (N-108). Guards still run in
    /// `Op::check` (stopped, not locked, snapshot exists).
    Rollback { name: String, snap: String, dataset: String },
    /// `zfs destroy -r <dataset>@snap` where `dataset` is the guest's own
    /// dataset (e.g. `zroot/vm/web`); `-r` takes the zvol children with it.
    SnapDelete { name: String, snap: String, dataset: String },
    Clone { name: String, snap: String, to: String },
    Destroy(String),
    Create { name: String, template: String, datastore: String, disk_gb: u32, cpu: u32, mem_mib: u64 },
    SetSwitch { name: String, conf: PathBuf, switch: String },
    Install { name: String, iso: String },
    Autostart { name: String, on: bool },
    Unlock { name: String, lock: PathBuf },
    AddDisk { name: String, size_gb: u32 },
    AddNic { name: String, switch: String },
    SwitchCreate { name: String, private: bool },
    SwitchDestroy(String),
}

fn s(v: &str) -> String {
    v.to_string()
}

impl Op {
    pub fn program(&self) -> &'static str {
        match self {
            Op::SetSwitch { .. } | Op::Autostart { .. } => "sysrc",
            Op::Unlock { .. } => "rm",
            Op::SnapDelete { .. } | Op::Rollback { .. } => "zfs",
            _ => "vm",
        }
    }

    pub fn argv(&self) -> Vec<String> {
        match self {
            Op::Start(n) => vec![s("start"), n.clone()],
            Op::Stop(n) => vec![s("stop"), n.clone()],
            Op::Restart(n) => vec![s("restart"), n.clone()],
            Op::Suspend(n) => vec![s("suspend"), n.clone()],
            Op::Poweroff(n) => vec![s("poweroff"), s("-f"), n.clone()],
            Op::Reset(n) => vec![s("reset"), s("-f"), n.clone()],
            Op::Snapshot { name, snap, force } => {
                let mut v = vec![s("snapshot")];
                if *force {
                    v.push(s("-f"));
                }
                v.push(format!("{name}@{snap}"));
                v
            }
            Op::Rollback { snap, dataset, .. } => vec![s("rollback"), s("-r"), format!("{dataset}@{snap}")],
            Op::SnapDelete { snap, dataset, .. } => vec![s("destroy"), s("-r"), format!("{dataset}@{snap}")],
            Op::Clone { name, snap, to } => vec![s("clone"), format!("{name}@{snap}"), to.clone()],
            Op::Destroy(n) => vec![s("destroy"), s("-f"), n.clone()],
            Op::Create { name, template, datastore, disk_gb, cpu, mem_mib } => vec![
                s("create"),
                s("-t"),
                template.clone(),
                s("-d"),
                datastore.clone(),
                s("-s"),
                format!("{disk_gb}G"),
                s("-m"),
                fmt_mib(*mem_mib),
                s("-c"),
                cpu.to_string(),
                name.clone(),
            ],
            Op::SetSwitch { conf, switch, .. } => vec![s("-f"), conf.display().to_string(), format!("network0_switch={switch}")],
            Op::Install { name, iso } => vec![s("install"), name.clone(), iso.clone()],
            Op::Autostart { name, on } => vec![format!("vm_list{}={name}", if *on { "+" } else { "-" })],
            Op::Unlock { lock, .. } => vec![s("-f"), lock.display().to_string()],
            Op::AddDisk { name, size_gb } => vec![s("add"), s("-d"), s("disk"), s("-s"), format!("{size_gb}G"), name.clone()],
            Op::AddNic { name, switch } => vec![s("add"), s("-d"), s("network"), s("-s"), switch.clone(), name.clone()],
            Op::SwitchCreate { name, private } => {
                let mut v = vec![s("switch"), s("create")];
                if *private {
                    v.push(s("-p"));
                }
                v.push(name.clone());
                v
            }
            Op::SwitchDestroy(n) => vec![s("switch"), s("destroy"), n.clone()],
        }
    }

    /// The exact command line, e.g. `vm start testvm`.
    pub fn cmd(&self) -> String {
        std::iter::once(self.program().to_string()).chain(self.argv()).collect::<Vec<_>>().join(" ")
    }

    /// The guest this op acts on (None for host-level ops).
    pub fn target(&self) -> Option<&str> {
        match self {
            Op::Start(n) | Op::Stop(n) | Op::Restart(n) | Op::Suspend(n) | Op::Poweroff(n) | Op::Reset(n) | Op::Destroy(n) => Some(n),
            Op::Snapshot { name, .. }
            | Op::Rollback { name, .. }
            | Op::SnapDelete { name, .. }
            | Op::Clone { name, .. }
            | Op::Create { name, .. }
            | Op::SetSwitch { name, .. }
            | Op::Install { name, .. }
            | Op::Autostart { name, .. }
            | Op::Unlock { name, .. }
            | Op::AddDisk { name, .. }
            | Op::AddNic { name, .. } => Some(name),
            Op::SwitchCreate { .. } | Op::SwitchDestroy(_) => None,
        }
    }

    /// Short verb for titles/toasts.
    pub fn title(&self) -> String {
        match self {
            Op::Start(n) => format!("Start {n}"),
            Op::Stop(n) => format!("Stop {n}"),
            Op::Restart(n) => format!("Restart {n}"),
            Op::Suspend(n) => format!("Suspend {n}"),
            Op::Poweroff(n) => format!("Power off {n}"),
            Op::Reset(n) => format!("Reset {n}"),
            Op::Snapshot { name, .. } => format!("Snapshot {name}"),
            Op::Rollback { name, snap, .. } => format!("Roll back {name} to @{snap}"),
            Op::SnapDelete { name, snap, .. } => format!("Delete {name}@{snap}"),
            Op::Clone { to, .. } => format!("Clone to {to}"),
            Op::Destroy(n) => format!("Destroy {n}"),
            Op::Create { name, .. } => format!("Create {name}"),
            Op::SetSwitch { switch, .. } => format!("Attach network0 to {switch}"),
            Op::Install { .. } => "Boot the installer".into(),
            Op::Autostart { on, .. } => if *on { "Start at host boot".into() } else { "Don't start at host boot".into() },
            Op::Unlock { .. } => "Remove the stale lock".into(),
            Op::AddDisk { size_gb, .. } => format!("Add a {size_gb}G disk"),
            Op::AddNic { switch, .. } => format!("Add a NIC on {switch}"),
            Op::SwitchCreate { name, .. } => format!("Create switch {name}"),
            Op::SwitchDestroy(n) => format!("Destroy switch {n}"),
        }
    }

    /// Advisory undo, shown next to plan steps.
    pub fn undo(&self) -> String {
        match self {
            Op::Start(n) => format!("vm stop {n}"),
            Op::Stop(n) | Op::Poweroff(n) => format!("vm start {n}"),
            Op::Restart(_) | Op::Reset(_) => "—".into(),
            Op::Suspend(n) => format!("vm start {n}"),
            Op::Snapshot { name, snap, .. } => format!("zfs destroy -r …/{name}@{snap}"),
            Op::Rollback { .. } | Op::SnapDelete { .. } | Op::Destroy(_) => "not reversible".into(),
            Op::Clone { to, .. } => format!("vm destroy {to}"),
            Op::Create { name, .. } => format!("vm destroy {name}"),
            Op::SetSwitch { .. } => "sysrc -f … network0_switch=public".into(),
            Op::Install { name, .. } => format!("vm poweroff -f {name}"),
            Op::Autostart { name, on } => format!("sysrc vm_list{}={name}", if *on { "-" } else { "+" }),
            Op::Unlock { name, .. } => format!("echo <host> > …/{name}/run.lock"),
            Op::AddDisk { .. } | Op::AddNic { .. } => "edit the guest config".into(),
            Op::SwitchCreate { name, .. } => format!("vm switch destroy {name}"),
            Op::SwitchDestroy(n) => format!("vm switch create {n}"),
        }
    }

    pub fn is_destructive(&self) -> bool {
        matches!(self, Op::Poweroff(_) | Op::Reset(_) | Op::Rollback { .. } | Op::SnapDelete { .. } | Op::Destroy(_) | Op::Unlock { .. } | Op::SwitchDestroy(_))
    }

    /// Predict whether vm-bhyve will accept this op, mirroring its own error
    /// text where it has one. `Err` is a plain-language reason.
    pub fn guard(&self, inv: &Inventory) -> Result<(), String> {
        if !inv.privilege.can_write() {
            return Err("virtual machines can only be managed by root".into());
        }
        let need = |n: &str| -> Result<&Guest, String> {
            inv.guest(n).ok_or_else(|| format!("{n} does not appear to be an existing virtual machine"))
        };
        let snap_exists = |g: &Guest, snap: &str| g.snaps.iter().any(|x| x.name == snap);
        let zfs = |g: &Guest| inv.datastore(&g.datastore).map(|d| d.dataset != "-").unwrap_or(false);

        if let Some(n) = self.target() {
            if !matches!(self, Op::Create { .. }) {
                let g = need(n)?;
                if let VmState::Locked(h) = &g.state {
                    if !matches!(self, Op::Unlock { .. }) {
                        return Err(format!("{n} is locked by {h} — it's probably running there from a shared datastore"));
                    }
                }
                if let VmState::Other(st) = &g.state {
                    return Err(format!("{n} is in an unknown state ({st})"));
                }
            }
        }
        let running = |n: &str| need(n).map(|g| g.state.is_running()).unwrap_or(false);
        let stopped = |n: &str| need(n).map(|g| g.state == VmState::Stopped).unwrap_or(false);

        match self {
            Op::Start(n) => {
                if running(n) {
                    return Err(format!("{n} is already running"));
                }
            }
            Op::Stop(n) | Op::Restart(n) | Op::Reset(n) => {
                if !running(n) {
                    return Err(format!("{n} doesn't seem to be running"));
                }
            }
            Op::Poweroff(n) => {
                if !running(n) {
                    return Err(format!("{n} doesn't seem to be running"));
                }
            }
            Op::Suspend(n) => {
                if !running(n) {
                    return Err(format!("{n} doesn't seem to be running"));
                }
                if !inv.host.suspend_supported {
                    return Err("bhyve on this host does not support suspend (needs a BHYVE_SNAPSHOT kernel and suspend=yes)".into());
                }
            }
            Op::Snapshot { name, snap, force } => {
                let g = need(name)?;
                if !zfs(g) {
                    return Err("cannot snapshot guests on non-zfs datastores".into());
                }
                if g.state.is_running() && !force {
                    return Err(format!("{name} must be powered off first (use -f to override)"));
                }
                if snap_exists(g, snap) {
                    return Err(format!("{name}@{snap} already exists"));
                }
            }
            Op::Rollback { name, snap, .. } => {
                let g = need(name)?;
                if !zfs(g) {
                    return Err("cannot rollback guests on non-zfs datastores".into());
                }
                if g.state != VmState::Stopped {
                    return Err(format!("{name} must be stopped to roll back"));
                }
                if !snap_exists(g, snap) {
                    return Err(format!("{name}@{snap}: snapshot not found"));
                }
            }
            Op::SnapDelete { name, snap, .. } => {
                if !snap_exists(need(name)?, snap) {
                    return Err(format!("{name}@{snap}: snapshot not found"));
                }
            }
            Op::Clone { name, snap, to } => {
                let g = need(name)?;
                if !zfs(g) {
                    return Err("cannot clone guests on non-zfs datastores".into());
                }
                if !snap_exists(g, snap) {
                    return Err(format!("{name}@{snap}: snapshot not found"));
                }
                if inv.guest(to).is_some() {
                    return Err(format!("new guest already exists: {to}"));
                }
                if !valid_name(to) {
                    return Err(format!("{to}: use letters, digits, - and _ only"));
                }
            }
            Op::Destroy(n) | Op::Install { name: n, .. } => {
                if !stopped(n) {
                    return Err(format!("{n} must be stopped first"));
                }
            }
            Op::Create { name, template, datastore, .. } => {
                if !valid_name(name) {
                    return Err(format!("{name}: use letters, digits, - and _ only"));
                }
                if inv.guest(name).is_some() {
                    return Err(format!("{name} already exists"));
                }
                if !inv.templates.iter().any(|t| &t.id == template) {
                    return Err(format!("unable to find template {template}"));
                }
                if inv.datastore(datastore).is_none() {
                    return Err(format!("unable to locate datastore {datastore}"));
                }
            }
            Op::Unlock { name, .. } => {
                if !matches!(need(name)?.state, VmState::Locked(_)) {
                    return Err(format!("{name} has no run.lock to remove"));
                }
            }
            Op::AddDisk { name, .. } | Op::AddNic { name, .. } => {
                if !stopped(name) {
                    return Err(format!("{name} must be stopped to add devices"));
                }
            }
            Op::SwitchCreate { name, .. } => {
                if inv.switches.iter().any(|w| &w.name == name) {
                    return Err(format!("switch {name} already exists"));
                }
                if !valid_name(name) {
                    return Err(format!("{name}: use letters, digits, - and _ only"));
                }
            }
            Op::SwitchDestroy(n) => {
                if !inv.switches.iter().any(|w| &w.name == n) {
                    return Err(format!("switch {n} doesn't exist"));
                }
            }
            Op::SetSwitch { .. } | Op::Autostart { .. } => {}
        }
        Ok(())
    }
}

/// The suggested next step for a guest (F-403). Both front-ends show this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// `console`, `start`, `explain`, `none`
    pub kind: &'static str,
    pub label: String,
    pub text: String,
    pub cmd: String,
}

pub fn suggest(g: &Guest) -> Suggestion {
    let n = &g.name;
    match &g.state {
        VmState::Running(_) | VmState::Bootloader(_) => Suggestion {
            kind: "console",
            label: "Open console".into(),
            text: format!("{n} is up. Next step: open its serial console."),
            cmd: format!("vm console {n}"),
        },
        VmState::Stopped => Suggestion {
            kind: "start",
            label: "Start".into(),
            text: format!("{n} is stopped. Start it and it boots with {} vCPU and {} memory.", g.cpu, fmt_mib(g.mem_mib)),
            cmd: format!("vm start {n}"),
        },
        VmState::Suspended => Suggestion {
            kind: "start",
            label: "Resume".into(),
            text: format!("{n} is suspended with its memory saved. Starting it restores exactly where it paused."),
            cmd: format!("vm start {n}"),
        },
        VmState::Locked(h) => Suggestion {
            kind: "explain",
            label: "Explain lock".into(),
            text: format!("run.lock is held by {h}. It's probably running there from a shared datastore. I won't touch it unless you confirm {h} is gone."),
            cmd: format!("cat {}/run.lock", g.dir.display()),
        },
        VmState::Other(st) => Suggestion {
            kind: "none",
            label: "Refresh".into(),
            text: format!("{n} is in a state I can't act on ({st})."),
            cmd: "vm list".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::DemoHost;
    use crate::inventory::Hypervisor;

    #[test]
    fn argv_shapes() {
        assert_eq!(Op::Start("a".into()).cmd(), "vm start a");
        assert_eq!(Op::Poweroff("a".into()).cmd(), "vm poweroff -f a");
        assert_eq!(Op::Snapshot { name: "a".into(), snap: "s".into(), force: true }.cmd(), "vm snapshot -f a@s");
        assert_eq!(Op::Rollback { name: "a".into(), snap: "s".into(), dataset: "zroot/vm/a".into() }.cmd(), "zfs rollback -r zroot/vm/a@s");
        assert_eq!(Op::Clone { name: "a".into(), snap: "s".into(), to: "b".into() }.cmd(), "vm clone a@s b");
        assert_eq!(Op::SnapDelete { name: "a".into(), snap: "s".into(), dataset: "zroot/vm/a".into() }.cmd(), "zfs destroy -r zroot/vm/a@s");
        assert_eq!(
            Op::Create { name: "u".into(), template: "ubuntu".into(), datastore: "default".into(), disk_gb: 20, cpu: 2, mem_mib: 4096 }.cmd(),
            "vm create -t ubuntu -d default -s 20G -m 4G -c 2 u"
        );
        assert_eq!(Op::Autostart { name: "a".into(), on: true }.cmd(), "sysrc vm_list+=a");
        assert_eq!(Op::SwitchCreate { name: "lan0".into(), private: true }.cmd(), "vm switch create -p lan0");
    }

    #[test]
    fn guards_mirror_vm_bhyve() {
        let inv = DemoHost::new().inventory();
        assert!(Op::Start("win11-dev".into()).guard(&inv).is_ok());
        assert!(Op::Start("ubuntu-2404".into()).guard(&inv).unwrap_err().contains("already running"));
        assert!(Op::Snapshot { name: "ubuntu-2404".into(), snap: "x".into(), force: false }.guard(&inv).unwrap_err().contains("-f"));
        assert!(Op::Snapshot { name: "ubuntu-2404".into(), snap: "x".into(), force: true }.guard(&inv).is_ok());
        assert!(Op::Start("alpine-test".into()).guard(&inv).unwrap_err().contains("locked"));
        assert!(Op::Destroy("ubuntu-2404".into()).guard(&inv).is_err());
        assert!(Op::Start("ghost".into()).guard(&inv).is_err());
    }

    #[test]
    fn suggestion_follows_state() {
        let inv = DemoHost::new().inventory();
        assert_eq!(suggest(inv.guest("ubuntu-2404").unwrap()).kind, "console");
        assert_eq!(suggest(inv.guest("win11-dev").unwrap()).kind, "start");
        assert_eq!(suggest(inv.guest("debian-ci").unwrap()).label, "Resume");
        assert_eq!(suggest(inv.guest("alpine-test").unwrap()).kind, "explain");
    }
}
