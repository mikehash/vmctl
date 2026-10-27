//! `DemoHost` — an in-memory vm-bhyve host.
//!
//! Seeded with the prototype's fleet so the desktop can be reviewed (and UI
//! tests can run) on a machine without vm-bhyve or root. It interprets the same
//! argv that [`crate::ops::Op`] produces and enforces vm-bhyve's guards with
//! vm-bhyve's own error text, so failure paths look exactly like the real host.
//! Launch the desktop with `--demo` to use it.

use std::path::PathBuf;

use crate::backend::ExecResult;
use crate::info::{Disk, Nic, VmInfo};
use crate::inventory::{Datastore, Guest, HostFacts, Hypervisor, Inventory, Privilege, Snap, Switch, Template};
use crate::iso::Iso;
use crate::model::{now, parse_mib, VmState};

pub struct DemoHost {
    pub guests: Vec<Guest>,
    pub switches: Vec<Switch>,
    pub privilege: Privilege,
    next_pid: u32,
}

const DAY: i64 = 86_400;

fn guest(name: &str, ds: &str, loader: &str, cpu: u32, mem: &str, state: VmState, sw: &str, nic: &str, disk_g: u32, snaps: &[(&str, i64, u64)]) -> Guest {
    let t = now();
    let dir = PathBuf::from(format!("/zroot/vm/{name}"));
    let info = VmInfo {
        name: name.into(),
        state: state.clone(),
        fields: [("loader", loader), ("datastore", ds)].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        consoles: if state.is_running() { vec![("com1".into(), format!("/dev/nmdm-{name}.1B"))] } else { vec![] },
        nics: sw
            .split(", ")
            .enumerate()
            .map(|(i, s)| Nic { number: i.to_string(), emulation: nic.into(), switch: s.into(), mac: format!("58:9c:fc:0{i}:4a:1{}", name.len() % 10), device: String::new() })
            .collect(),
        disks: vec![Disk {
            number: "0".into(),
            device_type: "file".into(),
            emulation: "virtio-blk".into(),
            path: format!("/zroot/vm/{name}/disk0.img"),
            size: format!("{disk_g}.000G"),
            used: format!("{:.3}G", disk_g as f32 * 0.31),
        }],
    };
    Guest {
        name: name.into(),
        datastore: ds.into(),
        loader: loader.into(),
        cpu,
        mem_mib: parse_mib(mem).unwrap_or(1024),
        vnc: if loader == "uefi" && nic == "e1000" { "0.0.0.0:5901".into() } else { "-".into() },
        autostart: false,
        state,
        info: Some(info),
        snaps: snaps.iter().map(|(n, ago, used)| Snap { name: n.to_string(), created: t - ago, used: *used, datasets: vec![format!("zroot/vm/{name}")] }).collect(),
        log_tail: vec![format!("{name}: initialising"), format!("{name}:  [loader: {loader}]"), format!("{name}:  [cpu: {cpu}]"), format!("{name}:  [memory: {mem}]")],
        dir,
    }
}

impl DemoHost {
    pub fn new() -> Self {
        const G: u64 = 1 << 30;
        let guests = vec![
            guest("ubuntu-2404", "default", "uefi", 2, "4G", VmState::Running(Some(4182)), "public", "virtio-net", 20, &[("fresh-install", 25 * DAY, 38 * G / 10), ("pre-upgrade", 9 * DAY, 12 * G / 10)]),
            guest("freebsd-build", "fast", "bhyveload", 8, "16G", VmState::Running(Some(3910)), "public", "virtio-net", 80, &[("ports-2026Q4", 16 * DAY, 64 * G / 10)]),
            guest("openbsd-fw", "default", "grub", 1, "1G", VmState::Running(Some(2207)), "public, lan0", "virtio-net", 8, &[]),
            guest("win11-dev", "fast", "uefi", 4, "8G", VmState::Stopped, "nat0", "e1000", 64, &[("activated", 30 * DAY, 11 * G)]),
            guest("debian-ci", "default", "grub", 2, "2G", VmState::Suspended, "nat0", "virtio-net", 16, &[("runner-ok", 8 * DAY, 9 * G / 10)]),
            guest("alpine-test", "default", "grub", 1, "512M", VmState::Locked("nas01".into()), "public", "virtio-net", 4, &[]),
        ];
        let sw = |n: &str, k: &str, i: &str, a: &str, p: &str, ports: &str| Switch { name: n.into(), kind: k.into(), iface: i.into(), address: a.into(), private: p.into(), ports: ports.into() };
        Self {
            guests,
            switches: vec![
                sw("public", "standard", "vm-public", "-", "no", "em0"),
                sw("nat0", "standard", "vm-nat0", "10.10.0.1/24", "no", "-"),
                sw("lan0", "standard", "vm-lan0", "-", "yes", "-"),
            ],
            privilege: Privilege::Root,
            next_pid: 4300,
        }
    }

    fn find(&mut self, n: &str) -> Option<&mut Guest> {
        self.guests.iter_mut().find(|g| g.name == n)
    }

    fn templates() -> Vec<Template> {
        let t = |id: &str, loader: &str, cpu: u32, mem: u64, nic: &str, gfx: bool| Template { id: id.into(), loader: loader.into(), cpu, mem_mib: mem, nic: nic.into(), switch: "public".into(), graphics: gfx };
        vec![
            t("ubuntu", "uefi", 2, 4096, "virtio-net", false),
            t("freebsd", "bhyveload", 2, 2048, "virtio-net", false),
            t("debian", "grub", 2, 2048, "virtio-net", false),
            t("windows", "uefi", 4, 8192, "e1000", true),
            t("openbsd", "grub", 1, 1024, "virtio-net", false),
            t("alpine", "grub", 1, 1024, "virtio-net", false),
            t("default", "bhyveload", 1, 256, "virtio-net", false),
        ]
    }

    /// Interpret `vm …` argv with vm-bhyve's guards and messages.
    fn vm(&mut self, a: &[String]) -> Result<String, String> {
        let a: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        let positional: Vec<&str> = a.iter().skip(1).filter(|x| !x.starts_with('-')).copied().collect();
        let force = a.contains(&"-f");
        let missing = |n: &str| format!("{n} does not appear to be an existing virtual machine");
        let lockchk = |g: &Guest| -> Result<(), String> {
            if let VmState::Locked(h) = &g.state {
                Err(format!("{} appears to be running on {h} (locked)", g.name))
            } else {
                Ok(())
            }
        };
        let pid = {
            self.next_pid += 37;
            self.next_pid
        };
        match a.first().copied().unwrap_or("") {
            "start" | "install" => {
                let n = positional.first().copied().unwrap_or("");
                let g = self.find(n).ok_or_else(|| missing(n))?;
                lockchk(g)?;
                if g.state.is_running() {
                    return Err(format!("{n} appears to already be running"));
                }
                g.state = VmState::Running(Some(pid));
                Ok(format!("Starting {n}\n  * found guest in /zroot/vm/{n}\n  * booting..."))
            }
            "stop" | "restart" | "poweroff" | "reset" => {
                let op = a[0];
                let n = positional.first().copied().unwrap_or("");
                let g = self.find(n).ok_or_else(|| missing(n))?;
                lockchk(g)?;
                if !g.state.is_running() {
                    return Err(format!("{n} doesn't appear to be a running virtual machine"));
                }
                match op {
                    "stop" | "poweroff" => g.state = VmState::Stopped,
                    _ => g.state = VmState::Running(Some(pid)),
                }
                Ok(format!("Sending ACPI shutdown to {n}"))
            }
            "suspend" => Err("bhyve on this host does not support suspend".into()),
            "snapshot" => {
                let spec = positional.first().copied().unwrap_or("");
                let (n, snap) = spec.split_once('@').unwrap_or((spec, "manual"));
                let g = self.find(n).ok_or_else(|| missing(n))?;
                if g.state.is_running() && !force {
                    return Err(format!("{n} must be powered off first (use -f to override)"));
                }
                if g.snaps.iter().any(|s| s.name == snap) {
                    return Err(format!("cannot create snapshot 'zroot/vm/{n}@{snap}': dataset already exists"));
                }
                g.snaps.push(Snap { name: snap.into(), created: now(), used: 0, datasets: vec![format!("zroot/vm/{n}")] });
                Ok(String::new())
            }
            "rollback" => {
                let spec = positional.first().copied().unwrap_or("");
                let (n, snap) = spec.split_once('@').ok_or("a snapshot name must be provided in guest@snapshot format")?;
                let g = self.find(n).ok_or_else(|| missing(n))?;
                if g.state != VmState::Stopped {
                    return Err(format!("{n} is running; please stop it first"));
                }
                let i = g.snaps.iter().position(|s| s.name == snap).ok_or_else(|| format!("cannot open 'zroot/vm/{n}@{snap}': dataset does not exist"))?;
                if i + 1 < g.snaps.len() && !a.contains(&"-r") {
                    return Err(format!("cannot rollback to 'zroot/vm/{n}@{snap}': more recent snapshots exist\nuse '-r' to force deletion of the following snapshots"));
                }
                g.snaps.truncate(i + 1);
                Ok(String::new())
            }
            "clone" => {
                let spec = positional.first().copied().unwrap_or("");
                let to = positional.get(1).copied().unwrap_or("").to_string();
                let (n, snap) = spec.split_once('@').unwrap_or((spec, ""));
                if self.guests.iter().any(|g| g.name == to) {
                    return Err(format!("new guest already exists in /zroot/vm/{to}"));
                }
                let g = self.guests.iter().find(|g| g.name == n).ok_or_else(|| missing(n))?;
                if !snap.is_empty() && !g.snaps.iter().any(|s| s.name == snap) {
                    return Err(format!("{n}@{snap}: snapshot not found"));
                }
                let mut c = g.clone();
                c.name = to.clone();
                c.state = VmState::Stopped;
                c.snaps.clear();
                c.dir = PathBuf::from(format!("/zroot/vm/{to}"));
                if let Some(i) = c.info.as_mut() {
                    i.name = to.clone();
                    i.consoles.clear();
                }
                self.guests.push(c);
                Ok(String::new())
            }
            "destroy" => {
                let n = positional.first().copied().unwrap_or("");
                let g = self.find(n).ok_or_else(|| missing(n))?;
                if g.state != VmState::Stopped {
                    return Err(format!("{n} appears to be running or locked"));
                }
                self.guests.retain(|g| g.name != n);
                Ok(String::new())
            }
            "create" => {
                let opt = |k: &str| a.iter().position(|x| *x == k).and_then(|i| a.get(i + 1)).copied();
                let n = a.last().copied().unwrap_or("");
                if self.guests.iter().any(|g| g.name == n) {
                    return Err(format!("directory /zroot/vm/{n} already exists"));
                }
                let tpl = opt("-t").unwrap_or("default");
                let t = Self::templates().into_iter().find(|t| t.id == tpl).ok_or_else(|| format!("unable to find template /zroot/vm/.templates/{tpl}.conf"))?;
                let disk: u32 = opt("-s").and_then(|s| s.trim_end_matches('G').parse().ok()).unwrap_or(20);
                let mut g = guest(n, opt("-d").unwrap_or("default"), &t.loader, opt("-c").and_then(|c| c.parse().ok()).unwrap_or(t.cpu), opt("-m").unwrap_or("1G"), VmState::Stopped, "public", &t.nic, disk, &[]);
                g.info.as_mut().map(|i| i.consoles.clear());
                self.guests.push(g);
                Ok(String::new())
            }
            "add" => {
                let n = a.last().copied().unwrap_or("");
                let g = self.find(n).ok_or_else(|| missing(n))?;
                if g.state != VmState::Stopped {
                    return Err(format!("{n} must be stopped to add devices"));
                }
                Ok(String::new())
            }
            "switch" => match a.get(1).copied() {
                Some("create") => {
                    let n = a.last().copied().unwrap_or("").to_string();
                    if self.switches.iter().any(|s| s.name == n) {
                        return Err(format!("switch {n} already exists"));
                    }
                    self.switches.push(Switch { name: n.clone(), kind: "standard".into(), iface: format!("vm-{n}"), address: "-".into(), private: if a.contains(&"-p") { "yes" } else { "no" }.into(), ports: "-".into() });
                    Ok(String::new())
                }
                Some("destroy") => {
                    let n = a.last().copied().unwrap_or("");
                    let before = self.switches.len();
                    self.switches.retain(|s| s.name != n);
                    if before == self.switches.len() {
                        Err(format!("unable to locate virtual switch {n}"))
                    } else {
                        Ok(String::new())
                    }
                }
                _ => Ok(String::new()),
            },
            "" => Err("no command specified".into()),
            other => Err(format!("unknown command '{other}'")),
        }
    }
}

impl Default for DemoHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Hypervisor for DemoHost {
    fn run(&mut self, program: &str, argv: &[String]) -> ExecResult {
        let cmd = std::iter::once(program.to_string()).chain(argv.iter().cloned()).collect::<Vec<_>>().join(" ");
        if !self.privilege.can_write() && program == "vm" {
            return ExecResult::fail(cmd, 1, "/usr/local/sbin/vm: ERROR: virtual machines can only be managed by root");
        }
        // Small delay so progress states are visible in the demo.
        std::thread::sleep(std::time::Duration::from_millis(350));
        let res = match program {
            "vm" => self.vm(argv),
            "sysrc" => {
                if let Some(spec) = argv.iter().find(|a| a.starts_with("vm_list")) {
                    let on = spec.contains("+=");
                    let n = spec.split('=').nth(1).unwrap_or("").to_string();
                    if let Some(g) = self.find(&n) {
                        g.autostart = on;
                    }
                    Ok(format!("vm_list: -> {n}"))
                } else if let Some(spec) = argv.iter().find(|a| a.starts_with("network0_switch=")) {
                    let sw = spec.split('=').nth(1).unwrap_or("").to_string();
                    let conf = argv.get(1).cloned().unwrap_or_default();
                    let n = conf.rsplit('/').nth(1).unwrap_or("").to_string();
                    if let Some(i) = self.find(&n).and_then(|g| g.info.as_mut()) {
                        if let Some(nic) = i.nics.first_mut() {
                            nic.switch = sw;
                        }
                    }
                    Ok(String::new())
                } else {
                    Ok(String::new())
                }
            }
            "rm" => {
                let path = argv.last().cloned().unwrap_or_default();
                let n = path.rsplit('/').nth(1).unwrap_or("").to_string();
                match self.find(&n) {
                    Some(g) if matches!(g.state, VmState::Locked(_)) => {
                        g.state = VmState::Stopped;
                        Ok(String::new())
                    }
                    _ => Err(format!("rm: {path}: No such file or directory")),
                }
            }
            "zfs" => {
                let spec = argv.last().cloned().unwrap_or_default();
                let (ds, snap) = spec.split_once('@').unwrap_or((&spec, ""));
                let n = ds.rsplit('/').next().unwrap_or("").to_string();
                let sub = argv.first().map(String::as_str).unwrap_or("");
                let recursive = argv.iter().any(|x| x == "-r");
                match (sub, self.find(&n)) {
                    ("rollback", Some(g)) => {
                        let Some(i) = g.snaps.iter().position(|s| s.name == snap) else {
                            return ExecResult::fail(cmd, 1, format!("cannot open '{spec}': dataset does not exist"));
                        };
                        if i + 1 < g.snaps.len() && !recursive {
                            return ExecResult::fail(cmd, 1, format!("cannot rollback to '{spec}': more recent snapshots exist\nuse '-r' to force deletion of the following snapshots"));
                        }
                        g.snaps.truncate(i + 1);
                        Ok(String::new())
                    }
                    ("destroy", Some(g)) if g.snaps.iter().any(|s| s.name == snap) => {
                        g.snaps.retain(|s| s.name != snap);
                        Ok(String::new())
                    }
                    _ => Err("could not find any snapshots to destroy; check snapshot names.".to_string()),
                }
            }
            _ => Err(format!("{program}: not available in demo mode")),
        };
        match res {
            Ok(out) => ExecResult { cmd, code: 0, stdout: out, stderr: String::new() },
            Err(e) => ExecResult::fail(cmd, 1, format!("/usr/local/sbin/vm: ERROR: {e}").replace("/usr/local/sbin/vm: ERROR: rm:", "rm:")),
        }
    }

    fn inventory(&mut self) -> Inventory {
        const G: u64 = 1 << 30;
        // Keep VmInfo state in sync with the guest row.
        for g in &mut self.guests {
            let running = g.state.is_running();
            let n = g.name.clone();
            if let Some(i) = g.info.as_mut() {
                i.state = g.state.clone();
                i.consoles = if running { vec![("com1".into(), format!("/dev/nmdm-{n}.1B"))] } else { vec![] };
            }
        }
        Inventory {
            host: HostFacts {
                hostname: "zeus-host".into(),
                os: "FreeBSD 14.3-RELEASE".into(),
                vm_version: "vm-bhyve 1.7.5".into(),
                threads: 24,
                mem_mib: 64 * 1024,
                user: "mike".into(),
                suspend_supported: false,
            },
            privilege: self.privilege,
            guests: self.guests.clone(),
            switches: self.switches.clone(),
            datastores: vec![
                Datastore { name: "default".into(), kind: "zfs".into(), path: "/zroot/vm".into(), dataset: "zroot/vm".into(), used: 569 * G, avail: 412 * G },
                Datastore { name: "fast".into(), kind: "zfs".into(), path: "/nvme/vm".into(), dataset: "nvme/vm".into(), used: 656 * G, avail: 268 * G },
            ],
            isos: [
                ("ubuntu-24.04.3-live-server-amd64.iso", 31u64),
                ("FreeBSD-14.3-RELEASE-amd64-disc1.iso", 12),
                ("debian-13.1.0-amd64-netinst.iso", 7),
                ("Win11_24H2_English_x64.iso", 54),
                ("alpine-virt-3.22.1-x86_64.iso", 1),
            ]
            .iter()
            .map(|(n, s)| Iso { name: n.to_string(), path: PathBuf::from(format!("/zroot/vm/.iso/{n}")), size: s * G / 10, source: "datastore" })
            .collect(),
            templates: Self::templates(),
            list_ms: 142,
            error: None,
            taken_at: now(),
        }
    }

    fn console_argv(&self, name: &str) -> Vec<String> {
        vec!["sh".into(), "-c".into(), format!("echo 'demo mode: would run vm console {name}'; sleep 5")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Op;

    fn run(h: &mut DemoHost, op: Op) -> ExecResult {
        h.run(op.program(), &op.argv())
    }

    #[test]
    fn lifecycle_round_trip() {
        let mut h = DemoHost::new();
        assert_eq!(run(&mut h, Op::Start("win11-dev".into())).code, 0);
        let r = run(&mut h, Op::Start("win11-dev".into()));
        assert_eq!(r.code, 1);
        assert!(r.stderr.contains("already be running"));
        assert_eq!(run(&mut h, Op::Stop("win11-dev".into())).code, 0);
        assert_eq!(h.inventory().guest("win11-dev").unwrap().state, VmState::Stopped);
    }

    #[test]
    fn locked_guest_refuses_then_unlocks() {
        let mut h = DemoHost::new();
        assert!(run(&mut h, Op::Start("alpine-test".into())).stderr.contains("nas01"));
        assert_eq!(run(&mut h, Op::Unlock { name: "alpine-test".into(), lock: "/zroot/vm/alpine-test/run.lock".into() }).code, 0);
        assert_eq!(run(&mut h, Op::Start("alpine-test".into())).code, 0);
    }

    #[test]
    fn snapshot_clone_destroy() {
        let mut h = DemoHost::new();
        assert_eq!(run(&mut h, Op::Snapshot { name: "ubuntu-2404".into(), snap: "s1".into(), force: false }).code, 1);
        assert_eq!(run(&mut h, Op::Snapshot { name: "ubuntu-2404".into(), snap: "s1".into(), force: true }).code, 0);
        assert_eq!(run(&mut h, Op::Clone { name: "ubuntu-2404".into(), snap: "s1".into(), to: "u2".into() }).code, 0);
        assert_eq!(run(&mut h, Op::Destroy("u2".into())).code, 0);
        assert!(h.inventory().guest("u2").is_none());
    }

    #[test]
    fn rollback_truncates() {
        let mut h = DemoHost::new();
        let mut inv = h.inventory();
        assert_eq!(inv.guest("win11-dev").unwrap().snaps.len(), 1);
        run(&mut h, Op::Snapshot { name: "win11-dev".into(), snap: "b".into(), force: false });
        assert_eq!(run(&mut h, Op::Rollback { name: "win11-dev".into(), snap: "activated".into(), dataset: "zroot/vm/win11-dev".into() }).code, 0);
        inv = h.inventory();
        assert_eq!(inv.guest("win11-dev").unwrap().snaps.len(), 1);
    }

    #[test]
    fn create_uses_template() {
        let mut h = DemoHost::new();
        let op = Op::Create { name: "ubuntu-01".into(), template: "ubuntu".into(), datastore: "default".into(), disk_gb: 20, cpu: 2, mem_mib: 4096 };
        assert!(op.guard(&h.inventory()).is_ok());
        assert_eq!(run(&mut h, op).code, 0);
        let g = h.inventory().guest("ubuntu-01").unwrap().clone();
        assert_eq!(g.mem_mib, 4096);
        assert_eq!(g.loader, "uefi");
    }
}
