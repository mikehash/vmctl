//! Host inventory: everything a front-end renders, gathered in one pass.
//!
//! A [`Hypervisor`] owns two things: running commands (honest exit status) and
//! producing an [`Inventory`] snapshot. `LiveHost` talks to the real `vm(8)` /
//! `zfs(8)`; `DemoHost` (see `demo.rs`) is an in-memory host for screenshots,
//! UI tests and running the app on a machine without vm-bhyve.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::backend::ExecResult;
use crate::info::{parse_info, VmInfo};
use crate::iso::{self, Iso};
use crate::model::{parse_mib, VmState};
use crate::vmbhyve::parse_list;

/// How (and whether) we can run privileged `vm(8)` commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privilege {
    Root,
    /// Not root, but `sudo -n` works without a password.
    Sudo,
    /// Read-only: neither root nor passwordless sudo.
    None,
}

impl Privilege {
    pub fn can_write(self) -> bool {
        !matches!(self, Privilege::None)
    }
}

#[derive(Debug, Clone, Default)]
pub struct HostFacts {
    pub hostname: String,
    pub os: String,
    pub vm_version: String,
    pub threads: u32,
    pub mem_mib: u64,
    pub user: String,
    pub suspend_supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snap {
    pub name: String,
    pub created: i64,
    pub used: u64,
    /// Every dataset (guest fs + zvols) carrying `@name`, in `zfs list` order.
    pub datasets: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Guest {
    pub name: String,
    pub datastore: String,
    pub loader: String,
    pub cpu: u32,
    pub mem_mib: u64,
    pub vnc: String,
    pub autostart: bool,
    pub state: VmState,
    pub info: Option<VmInfo>,
    /// Oldest first.
    pub snaps: Vec<Snap>,
    /// Guest directory on disk (`<datastore path>/<name>`).
    pub dir: PathBuf,
    /// Tail of `vm-bhyve.log` (what vm-bhyve did with this guest).
    pub log_tail: Vec<String>,
}

impl Guest {
    pub fn switches(&self) -> Vec<String> {
        self.info.as_ref().map(|i| i.nics.iter().map(|n| n.switch.clone()).collect()).unwrap_or_default()
    }
    pub fn console(&self) -> String {
        if self.vnc != "-" && !self.vnc.is_empty() {
            return format!("vnc {}", self.vnc);
        }
        if let Some(i) = &self.info {
            if let Some((port, dev)) = i.consoles.first() {
                return format!("{port} {dev}");
            }
        }
        "com1 (nmdm, on start)".into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Switch {
    pub name: String,
    pub kind: String,
    pub iface: String,
    pub address: String,
    pub private: String,
    pub ports: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datastore {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub dataset: String,
    pub used: u64,
    pub avail: u64,
}

impl Datastore {
    pub fn used_pct(&self) -> f32 {
        let t = self.used + self.avail;
        if t == 0 { 0.0 } else { self.used as f32 / t as f32 * 100.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub id: String,
    pub loader: String,
    pub cpu: u32,
    pub mem_mib: u64,
    pub nic: String,
    /// `network0_switch` from the template (what `vm create` attaches by default).
    pub switch: String,
    pub graphics: bool,
}

#[derive(Debug, Clone)]
pub struct Inventory {
    pub host: HostFacts,
    pub privilege: Privilege,
    pub guests: Vec<Guest>,
    pub switches: Vec<Switch>,
    pub datastores: Vec<Datastore>,
    pub isos: Vec<Iso>,
    pub templates: Vec<Template>,
    /// Wall time of `vm list`, for the status bar.
    pub list_ms: u128,
    /// Set when the inventory is partial (e.g. `vm list` failed). Real stderr.
    pub error: Option<String>,
    pub taken_at: i64,
}

impl Inventory {
    pub fn guest(&self, name: &str) -> Option<&Guest> {
        self.guests.iter().find(|g| g.name == name)
    }
    pub fn names(&self) -> Vec<String> {
        self.guests.iter().map(|g| g.name.clone()).collect()
    }
    /// (vCPUs, MiB) configured for guests that hold resources.
    pub fn allocated(&self) -> (u32, u64) {
        self.guests
            .iter()
            .filter(|g| g.state.holds_resources())
            .fold((0, 0), |(c, m), g| (c + g.cpu, m + g.mem_mib))
    }
    pub fn datastore(&self, name: &str) -> Option<&Datastore> {
        self.datastores.iter().find(|d| d.name == name)
    }
}

/// Anything that can run host commands and describe the host.
pub trait Hypervisor: Send {
    /// Run `program argv…` with whatever privilege is needed. Never panics;
    /// spawn failures come back as exit 127 with the OS error as stderr.
    fn run(&mut self, program: &str, argv: &[String]) -> ExecResult;
    fn inventory(&mut self) -> Inventory;
    /// Command line that attaches an interactive console (for a terminal).
    fn console_argv(&self, name: &str) -> Vec<String>;
    /// Re-detect privilege (the user may have fixed sudo since launch).
    fn reprobe(&mut self) {}
}

// ───────────────────────── parsers (pure) ─────────────────────────

fn table_rows(raw: &str, first: &str) -> Vec<Vec<String>> {
    let mut lines = raw.lines().filter(|l| !l.trim().is_empty());
    match lines.next() {
        Some(h) if h.trim_start().starts_with(first) => {}
        _ => return Vec::new(),
    }
    lines.map(|l| l.split_whitespace().map(str::to_string).collect()).collect()
}

/// `vm switch list`: NAME TYPE IFACE ADDRESS PRIVATE MTU VLAN PORTS
pub fn parse_switches(raw: &str) -> Vec<Switch> {
    table_rows(raw, "NAME")
        .into_iter()
        .filter(|r| r.len() >= 5)
        .map(|r| Switch {
            name: r[0].clone(),
            kind: r[1].clone(),
            iface: r[2].clone(),
            address: r[3].clone(),
            private: r[4].clone(),
            ports: if r.len() > 7 { r[7..].join(" ") } else { "-".into() },
        })
        .collect()
}

/// `vm datastore list`: NAME TYPE PATH ZFS DATASET
pub fn parse_datastores(raw: &str) -> Vec<Datastore> {
    table_rows(raw, "NAME")
        .into_iter()
        .filter(|r| r.len() >= 3)
        .map(|r| Datastore {
            name: r[0].clone(),
            kind: r[1].clone(),
            path: r[2].clone(),
            dataset: r.get(3).cloned().unwrap_or_else(|| "-".into()),
            used: 0,
            avail: 0,
        })
        .collect()
}

/// `zfs list -H -p -t snapshot -o name,used,creation -r <dataset>` → per-guest snapshots.
///
/// `vm snapshot` is recursive, so a guest with zvols has one snapshot per
/// dataset; we fold them into one entry per `guest@snap` and sum `used`.
pub fn parse_zfs_snapshots(raw: &str, dataset: &str) -> BTreeMap<String, Vec<Snap>> {
    let mut map: BTreeMap<String, Vec<Snap>> = BTreeMap::new();
    let prefix = format!("{dataset}/");
    for line in raw.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 {
            continue;
        }
        let Some((ds, snap)) = cols[0].split_once('@') else { continue };
        let Some(rest) = ds.strip_prefix(&prefix) else { continue };
        let guest = rest.split('/').next().unwrap_or(rest).to_string();
        let used: u64 = cols[1].trim().parse().unwrap_or(0);
        let created: i64 = cols[2].trim().parse().unwrap_or(0);
        let list = map.entry(guest).or_default();
        if let Some(s) = list.iter_mut().find(|s| s.name == snap) {
            s.used += used;
            s.datasets.push(ds.to_string());
        } else {
            list.push(Snap { name: snap.to_string(), created, used, datasets: vec![ds.to_string()] });
        }
    }
    for v in map.values_mut() {
        v.sort_by_key(|s| s.created);
    }
    map
}

/// Parse a vm-bhyve `key="value"` config file.
pub fn parse_conf(raw: &str) -> BTreeMap<String, String> {
    raw.lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_string(), v.trim().trim_matches('"').to_string()))
        })
        .collect()
}

pub fn template_from_conf(id: &str, raw: &str) -> Template {
    let c = parse_conf(raw);
    Template {
        id: id.to_string(),
        loader: c.get("loader").cloned().unwrap_or_else(|| "bhyveload".into()),
        cpu: c.get("cpu").and_then(|v| v.parse().ok()).unwrap_or(1),
        mem_mib: c.get("memory").and_then(|v| parse_mib(v)).unwrap_or(512),
        nic: c.get("network0_type").cloned().unwrap_or_else(|| "virtio-net".into()),
        switch: c.get("network0_switch").cloned().unwrap_or_default(),
        graphics: c.get("graphics").map(|v| v == "yes").unwrap_or(false),
    }
}

fn tail(path: &Path, n: usize) -> Vec<String> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let lines: Vec<String> = s.lines().map(str::to_string).collect();
            lines[lines.len().saturating_sub(n)..].to_vec()
        }
        Err(_) => Vec::new(),
    }
}

// ───────────────────────── live host ─────────────────────────

pub struct LiveHost {
    privilege: Privilege,
    facts: Option<HostFacts>,
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

fn plain(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

impl LiveHost {
    pub fn new() -> Self {
        let privilege = if euid() == 0 {
            Privilege::Root
        } else if Command::new("sudo")
            .args(["-n", "true"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            Privilege::Sudo
        } else {
            Privilege::None
        };
        Self { privilege, facts: None }
    }

    pub fn privilege(&self) -> Privilege {
        self.privilege
    }

    fn facts(&mut self) -> HostFacts {
        if let Some(f) = &self.facts {
            return f.clone();
        }
        let vmv = self.run("vm", &["version".into()]);
        let vm_version = vmv
            .stdout
            .split_whitespace()
            .find(|w| w.starts_with('v') && w[1..].starts_with(|c: char| c.is_ascii_digit()))
            .map(|v| format!("vm-bhyve {}", &v[1..]))
            .unwrap_or_else(|| "vm-bhyve".into());
        let user = std::env::var("SUDO_USER")
            .or_else(|_| std::env::var("DOAS_USER"))
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "root".into());
        let bhyvectl = Command::new("bhyvectl").output().map(|o| {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s
        });
        let f = HostFacts {
            hostname: plain("hostname", &["-s"]).unwrap_or_else(|| "localhost".into()),
            os: plain("uname", &["-sr"]).unwrap_or_default(),
            vm_version,
            threads: plain("sysctl", &["-n", "hw.ncpu"]).and_then(|s| s.parse().ok()).unwrap_or(1),
            mem_mib: plain("sysctl", &["-n", "hw.physmem"])
                .and_then(|s| s.parse::<u64>().ok())
                .map(|b| b / 1024 / 1024)
                .unwrap_or(0),
            user,
            suspend_supported: bhyvectl.map(|s| s.contains("--suspend=")).unwrap_or(false),
        };
        self.facts = Some(f.clone());
        f
    }

    /// When `vm list` is unavailable (no privilege), show what's configured on
    /// disk so the user can still browse. State is honestly "unknown".
    fn guests_from_disk(vm_dir: &Path) -> Vec<Guest> {
        let Ok(rd) = std::fs::read_dir(vm_dir) else { return Vec::new() };
        let mut out: Vec<Guest> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    return None;
                }
                let conf = std::fs::read_to_string(e.path().join(format!("{name}.conf"))).ok()?;
                let c = parse_conf(&conf);
                Some(Guest {
                    datastore: "default".into(),
                    loader: c.get("loader").cloned().unwrap_or_default(),
                    cpu: c.get("cpu").and_then(|v| v.parse().ok()).unwrap_or(1),
                    mem_mib: c.get("memory").and_then(|v| parse_mib(v)).unwrap_or(0),
                    vnc: "-".into(),
                    autostart: false,
                    state: VmState::Other("Unknown (needs root)".into()),
                    info: None,
                    snaps: Vec::new(),
                    log_tail: tail(&e.path().join("vm-bhyve.log"), 12),
                    dir: e.path(),
                    name,
                })
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

impl Default for LiveHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Hypervisor for LiveHost {
    fn run(&mut self, program: &str, argv: &[String]) -> ExecResult {
        let cmd = std::iter::once(program.to_string()).chain(argv.iter().cloned()).collect::<Vec<_>>().join(" ");
        let mut c = if self.privilege == Privilege::Sudo {
            let mut c = Command::new("sudo");
            c.arg("-n").arg(program);
            c
        } else {
            Command::new(program)
        };
        c.args(argv);
        match c.output() {
            Ok(out) => ExecResult {
                cmd,
                code: out.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            },
            Err(e) => ExecResult::fail(cmd, 127, format!("failed to run `{program}`: {e}")),
        }
    }

    fn reprobe(&mut self) {
        *self = LiveHost::new();
    }

    fn console_argv(&self, name: &str) -> Vec<String> {
        let mut v = Vec::new();
        if self.privilege == Privilege::Sudo {
            v.push("sudo".into());
        }
        v.extend(["vm".to_string(), "console".into(), name.to_string()]);
        v
    }

    fn inventory(&mut self) -> Inventory {
        let host = self.facts();
        let mut error = None;

        // Datastores first: they tell us where guests and ISOs live.
        let ds_res = self.run("vm", &["datastore".into(), "list".into()]);
        let mut datastores = if ds_res.code == 0 { parse_datastores(&ds_res.stdout) } else { Vec::new() };
        if datastores.is_empty() {
            let dir = plain("sysrc", &["-n", "vm_dir"]).unwrap_or_default();
            let (path, dataset) = match dir.strip_prefix("zfs:") {
                Some(ds) => (
                    plain("zfs", &["get", "-H", "-o", "value", "mountpoint", ds]).unwrap_or_else(|| format!("/{ds}")),
                    ds.to_string(),
                ),
                None => (dir.clone(), "-".into()),
            };
            if !path.is_empty() {
                datastores.push(Datastore { name: "default".into(), kind: if dataset == "-" { "directory".into() } else { "zfs".into() }, path, dataset, used: 0, avail: 0 });
            }
        }
        for d in datastores.iter_mut().filter(|d| d.dataset != "-") {
            if let Some(s) = plain("zfs", &["list", "-H", "-p", "-o", "used,avail", &d.dataset]) {
                let mut it = s.split_whitespace();
                d.used = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                d.avail = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            }
        }
        let ds_path = |name: &str| -> PathBuf {
            datastores.iter().find(|d| d.name == name).map(|d| PathBuf::from(&d.path)).unwrap_or_else(|| PathBuf::from("/zroot/vm"))
        };
        let default_path = ds_path("default");

        let t0 = Instant::now();
        let list = self.run("vm", &["list".into()]);
        let list_ms = t0.elapsed().as_millis();

        let mut guests: Vec<Guest> = Vec::new();
        if list.code == 0 {
            let infos: BTreeMap<String, VmInfo> = {
                let r = self.run("vm", &["info".into()]);
                parse_info(&r.stdout).into_iter().map(|i| (i.name.clone(), i)).collect()
            };
            let mut snaps: BTreeMap<String, Vec<Snap>> = BTreeMap::new();
            for d in datastores.iter().filter(|d| d.dataset != "-") {
                if let Some(raw) = plain("zfs", &["list", "-H", "-p", "-t", "snapshot", "-o", "name,used,creation", "-r", &d.dataset]) {
                    snaps.extend(parse_zfs_snapshots(&raw, &d.dataset));
                }
            }
            for row in parse_list(&list.stdout).unwrap_or_default() {
                let dir = ds_path(&row.datastore).join(&row.name);
                guests.push(Guest {
                    cpu: row.cpu.parse().unwrap_or(0),
                    mem_mib: parse_mib(&row.memory).unwrap_or(0),
                    autostart: row.autostart.starts_with("Yes"),
                    state: VmState::parse(&row.state),
                    info: infos.get(&row.name).cloned(),
                    snaps: snaps.remove(&row.name).unwrap_or_default(),
                    log_tail: tail(&dir.join("vm-bhyve.log"), 12),
                    dir,
                    name: row.name,
                    datastore: row.datastore,
                    loader: row.loader,
                    vnc: row.vnc,
                });
            }
        } else {
            error = Some(if list.stderr.is_empty() { format!("vm list exited {}", list.code) } else { list.stderr.clone() });
            guests = Self::guests_from_disk(&default_path);
        }

        let sw = self.run("vm", &["switch".into(), "list".into()]);
        let switches = if sw.code == 0 { parse_switches(&sw.stdout) } else { Vec::new() };

        let tdir = default_path.join(".templates");
        let mut templates: Vec<Template> = crate::templates::discover_in(&tdir)
            .into_iter()
            .map(|id| {
                let raw = std::fs::read_to_string(tdir.join(format!("{id}.conf"))).unwrap_or_default();
                template_from_conf(&id, &raw)
            })
            .collect();
        templates.sort_by_key(|t| (template_rank(&t.id), t.id.clone()));

        Inventory {
            host,
            privilege: self.privilege,
            guests,
            switches,
            datastores,
            isos: iso::discover(&default_path),
            templates,
            list_ms,
            error,
            taken_at: crate::model::now(),
        }
    }
}

/// Order templates like the prototype: common OSes first, `default` last.
pub fn template_rank(id: &str) -> usize {
    crate::templates::KNOWN.iter().position(|k| *k == id).unwrap_or(if id == "default" { 99 } else { 50 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_parse() {
        let raw = "NAME    TYPE      IFACE      ADDRESS  PRIVATE  MTU  VLAN  PORTS\npublic  standard  vm-public  -        no       -    -     em0\n";
        let s = parse_switches(raw);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "public");
        assert_eq!(s[0].ports, "em0");
    }

    #[test]
    fn datastores_parse() {
        let raw = "NAME            TYPE        PATH                      ZFS DATASET\ndefault         zfs         /zroot/vm                 zroot/vm\n";
        let d = parse_datastores(raw);
        assert_eq!(d[0].path, "/zroot/vm");
        assert_eq!(d[0].dataset, "zroot/vm");
    }

    #[test]
    fn snapshots_fold_recursive() {
        let raw = "zroot/vm/a@one\t100\t10\nzroot/vm/a/disk0@one\t50\t10\nzroot/vm/a@two\t5\t20\nzroot/vm/b@x\t1\t5\nzroot/other@z\t1\t1\n";
        let m = parse_zfs_snapshots(raw, "zroot/vm");
        assert_eq!(m["a"].len(), 2);
        assert_eq!(m["a"][0].name, "one");
        assert_eq!(m["a"][0].used, 150);
        assert_eq!(m["a"][0].datasets, vec!["zroot/vm/a", "zroot/vm/a/disk0"]);
        assert_eq!(m["b"][0].name, "x");
        assert!(!m.contains_key("other"));
    }

    #[test]
    fn template_conf() {
        let t = template_from_conf("windows", "loader=\"uefi\"\ngraphics=\"yes\"\ncpu=4\nmemory=8G\nnetwork0_type=\"e1000\"\n");
        assert_eq!(t.loader, "uefi");
        assert_eq!(t.cpu, 4);
        assert_eq!(t.mem_mib, 8192);
        assert!(t.graphics);
        assert_eq!(t.nic, "e1000");
    }
}
