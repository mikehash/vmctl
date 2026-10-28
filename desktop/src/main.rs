//! vmctl-desktop — the Slint "Mission Control" front-end over `vmctl-core`.
//!
//! Layout of this crate:
//! - `ui/*.slint`   — the window (pure view; all state in the `App` global)
//! - worker         — vmctl_core::worker, background thread owning the hypervisor (N-103)
//! - `console.rs`   — embedded nmdm serial console
//! - `main.rs`      — the controller: core state → UI models, UI callbacks → ops
//!
//! Every guard, plan and command line comes from vmctl-core, so the desktop
//! and the TUI can't drift in behaviour.

slint::include_modules!();

mod console;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};
use vmctl_core::activity::{self, ActivityLog, Entry};
use vmctl_core::backend::ExecResult;
use vmctl_core::demo::DemoHost;
use vmctl_core::inventory::{Guest, Hypervisor, Inventory, LiveHost, Privilege};
use vmctl_core::model::{fmt_bytes, fmt_local, fmt_mib, next_name, now, stamp, valid_name, VmState};
use vmctl_core::ops::{suggest, Op};
use vmctl_core::planner::{self, Reply};
use vmctl_core::recipes;

use vmctl_core::worker::{Event, Tag, Worker};

const SOURCE: &str = "desktop";

// ───────────────────────── controller state ─────────────────────────

struct Msg {
    id: i32,
    user: bool,
    reply: Reply,
    text: String,
    /// idle | running | done | failed | cancelled | none
    phase: String,
    results: Vec<ExecResult>,
}

struct State {
    inv: Option<Inventory>,
    log: ActivityLog,
    selected: String,
    filter: String,
    search: String,
    view: String,
    msgs: Vec<Msg>,
    next_id: i32,
    active_plan: Option<i32>,
    toast_serial: u64,
    console: Option<(String, console::Session, console::Screen)>,
    demo: bool,
    sudo: bool,
}

type Shared = Rc<RefCell<State>>;

// ───────────────────────── helpers ─────────────────────────

fn ss(s: impl AsRef<str>) -> SharedString {
    SharedString::from(s.as_ref())
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

fn os_of(g: &Guest, inv: &Inventory) -> String {
    // vm list doesn't carry the OS; infer a friendly label from the name, then loader.
    let n = g.name.to_lowercase();
    for t in &inv.templates {
        if t.id != "default" && n.contains(&t.id) {
            return title_of(&t.id);
        }
    }
    for (needle, label) in [("win", "Windows"), ("bsd", "FreeBSD"), ("ubu", "Ubuntu"), ("deb", "Debian")] {
        if n.contains(needle) {
            return label.into();
        }
    }
    match g.loader.as_str() {
        "bhyveload" => "FreeBSD".into(),
        "uefi" => "UEFI guest".into(),
        "grub" => "GRUB guest".into(),
        l => l.to_string(),
    }
}

fn title_of(id: &str) -> String {
    match id {
        "freebsd" => "FreeBSD".into(),
        "openbsd" => "OpenBSD".into(),
        "netbsd" => "NetBSD".into(),
        "default" => "Generic".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

fn tpl_visual(id: &str) -> (&'static str, Color) {
    let hex = |v: u32| Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8);
    match id {
        "ubuntu" => ("UB", hex(0xFF7A3D)),
        "freebsd" => ("FB", hex(0xFF6B7D)),
        "debian" => ("DB", hex(0xFF6B7D)),
        "windows" => ("WN", hex(0x7FB2FF)),
        "openbsd" => ("OB", hex(0xF5B53D)),
        "alpine" => ("AL", hex(0x7FD1E0)),
        "default" => ("··", hex(0x9AA1AC)),
        _ => ("??", hex(0xB4BAC4)),
    }
}

fn kind_matches(filter: &str, g: &Guest) -> bool {
    match filter {
        "running" => g.state.kind() == "Running",
        "stopped" => g.state.kind() == "Stopped",
        "other" => !matches!(g.state.kind(), "Running" | "Stopped"),
        _ => true,
    }
}

fn search_matches(q: &str, g: &Guest) -> bool {
    if q.is_empty() {
        return true;
    }
    let q = q.to_lowercase();
    g.name.to_lowercase().contains(&q)
        || g.snaps.iter().any(|s| s.name.to_lowercase().contains(&q))
        || g.switches().iter().any(|s| s.to_lowercase().contains(&q))
        || g.loader.contains(&q)
}

fn log_row(e: &Entry, with_date: bool) -> LogRow {
    let today = fmt_local(now(), true)[..10].to_string();
    let full = fmt_local(e.ts, true);
    let time = if with_date || !full.starts_with(&today) { full } else { fmt_local(e.ts, false) };
    LogRow { time: ss(time), code: e.code, cmd: ss(&e.cmd), stderr: ss(&e.stderr), source: ss(&e.source) }
}

// ───────────────────────── pushing state to the UI ─────────────────────────

fn push_all(ui: &MainWindow, st: &mut State) {
    let app = ui.global::<App>();
    app.set_demo(st.demo);
    app.set_view(ss(&st.view));
    app.set_filter(ss(&st.filter));
    let Some(inv) = st.inv.as_ref() else {
        app.set_status_left(ss("vm list · loading…"));
        return;
    };

    // keep selection valid
    if inv.guest(&st.selected).is_none() {
        st.selected = inv.guests.first().map(|g| g.name.clone()).unwrap_or_default();
    }

    app.set_host_name(ss(&inv.host.hostname));
    app.set_host_os(ss(&inv.host.os));
    app.set_vm_version(ss(&inv.host.vm_version));
    app.set_user(ss(&inv.host.user));
    app.set_privilege(ss(match inv.privilege {
        Privilege::Root => "root",
        Privilege::Sudo => "sudo",
        Privilege::None => "none",
    }));
    app.set_threads(inv.host.threads as i32);
    app.set_mem_g((inv.host.mem_mib as f32 / 1024.0).max(1.0));
    let (ac, am) = inv.allocated();
    app.set_alloc_cpu(ac as i32);
    app.set_alloc_mem_g(am as f32 / 1024.0);
    app.set_list_error(ss(inv.error.clone().unwrap_or_default()));

    // list
    let row = |g: &Guest| VmRow {
        name: ss(&g.name),
        os: ss(os_of(g, inv)),
        kind: ss(g.state.kind()),
        label: ss(g.state.label()),
        res: ss(format!("{} vCPU · {}", g.cpu, fmt_mib(g.mem_mib))),
        count: g.snaps.len() as i32,
    };
    let shown: Vec<VmRow> = inv.guests.iter().filter(|g| kind_matches(&st.filter, g) && search_matches(&st.search, g)).map(row).collect();
    app.set_vms(model(shown));
    app.set_snap_vms(model(inv.guests.iter().map(row).collect()));
    app.set_count_all(inv.guests.len() as i32);
    app.set_count_running(inv.guests.iter().filter(|g| g.state.kind() == "Running").count() as i32);
    app.set_count_stopped(inv.guests.iter().filter(|g| g.state.kind() == "Stopped").count() as i32);
    app.set_count_other(inv.guests.iter().filter(|g| !matches!(g.state.kind(), "Running" | "Stopped")).count() as i32);
    app.set_snap_total(inv.guests.iter().map(|g| g.snaps.len()).sum::<usize>() as i32);
    app.set_log_total(st.log.entries.len() as i32);
    app.set_selected(ss(&st.selected));

    // detail
    match inv.guest(&st.selected) {
        Some(g) => {
            app.set_has_detail(true);
            app.set_detail(detail_for(g, inv, &st.log));
        }
        None => app.set_has_detail(false),
    }

    // switches
    let switches = inv
        .switches
        .iter()
        .map(|w| SwitchRow {
            name: ss(&w.name),
            kind: ss(if w.private == "yes" { format!("{} · private", w.kind) } else { w.kind.clone() }),
            iface: ss(if w.ports != "-" && !w.ports.is_empty() { format!("{} → {}", w.iface, w.ports) } else if w.address != "-" { format!("{} {}", w.iface, w.address) } else { w.iface.clone() }),
            desc: ss(if w.ports != "-" && !w.ports.is_empty() {
                format!("Bridged to {}. Guests get their own address on that network.", w.ports)
            } else if w.address != "-" {
                format!("Host-side address {} — guests sit on a private subnet behind the host.", w.address)
            } else if w.private == "yes" {
                "Isolated: guests on this switch can't talk to each other or the outside.".into()
            } else {
                "No uplink. Guests on this switch only see each other.".into()
            }),
            members: model(inv.guests.iter().filter(|g| g.switches().contains(&w.name)).map(row).collect()),
        })
        .collect();
    app.set_switches(model(switches));

    // datastores
    app.set_datastores(model(
        inv.datastores
            .iter()
            .map(|d| DsRow {
                name: ss(&d.name),
                kind: ss(if d.dataset != "-" { format!("ZFS · {}", d.dataset) } else { d.kind.clone() }),
                path: ss(&d.path),
                free: ss(if d.avail > 0 { fmt_bytes(d.avail) } else { "?".into() }),
                pct: d.used_pct(),
                machines: inv.guests.iter().filter(|g| g.datastore == d.name).count() as i32,
            })
            .collect(),
    ));
    app.set_isos(model(inv.isos.iter().map(|i| IsoRow { name: ss(&i.name), size: ss(i.size_h()), dir: ss(i.source) }).collect()));
    app.set_templates(model(
        inv.templates
            .iter()
            .map(|t| {
                let (mono, accent) = tpl_visual(&t.id);
                TplRow {
                    id: ss(&t.id),
                    title: ss(title_of(&t.id)),
                    mono: ss(mono),
                    accent,
                    loader: ss(&t.loader),
                    cpu: t.cpu as i32,
                    mem_g: t.mem_mib.div_ceil(1024).max(1) as i32,
                    spec: ss(format!("{} · {} vCPU · {} · {}", t.loader, t.cpu, fmt_mib(t.mem_mib), t.nic)),
                }
            })
            .collect(),
    ));

    // activity
    app.set_activity(model(st.log.entries.iter().take(300).map(|e| log_row(e, true)).collect()));

    // status bar
    app.set_status_left(ss(match &inv.error {
        Some(_) => "vm list failed".to_string(),
        None => format!("vm list · {} ms · refreshed {}", inv.list_ms, fmt_local(inv.taken_at, false)),
    }));
    app.set_status_ds(ss(format!("datastores {}", inv.datastores.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", "))));
    app.set_status_sw(ss(format!("switches {}", if inv.switches.is_empty() { "none".into() } else { inv.switches.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ") })));
    app.set_status_last(ss(st.log.entries.first().map(|e| format!("last: {} → exit {}", e.cmd, e.code)).unwrap_or_else(|| "last: —".into())));

    push_msgs(ui, st);
}

fn detail_for(g: &Guest, inv: &Inventory, log: &ActivityLog) -> Detail {
    let sug = suggest(g);
    let can = |op: Op| op.guard(inv).is_ok();
    let n = g.name.clone();
    let snaps_newest: Vec<SnapRow> = g
        .snaps
        .iter()
        .rev()
        .enumerate()
        .map(|(i, s)| SnapRow {
            name: ss(&s.name),
            when: ss(fmt_local(s.created, true)),
            size: ss(fmt_bytes(s.used)),
            safety: s.name.starts_with("pre-"),
            latest: i == 0,
        })
        .collect();

    let mut config = vec![KV { k: ss("loader"), v: ss(&g.loader) }, KV { k: ss("cpu"), v: ss(g.cpu.to_string()) }, KV { k: ss("memory"), v: ss(fmt_mib(g.mem_mib)) }];
    if let Some(info) = &g.info {
        for d in &info.disks {
            let v = if d.emulation.contains("cd") {
                format!("{} · {}", d.emulation, d.path.rsplit('/').next().unwrap_or(&d.path))
            } else {
                format!("{} · {} · {}{}", d.path.rsplit('/').next().unwrap_or(&d.path), d.emulation, d.size.replace(".000", ""), if d.used.is_empty() { String::new() } else { format!(" ({} used)", d.used.replace(".000", "")) })
            };
            config.push(KV { k: ss(format!("disk{}", d.number)), v: ss(v) });
        }
        for nic in &info.nics {
            let dev = if nic.device.is_empty() { String::new() } else { format!(" · {}", nic.device) };
            config.push(KV { k: ss(format!("network{}", nic.number)), v: ss(format!("{} → {}{}", nic.emulation, nic.switch, dev)) });
        }
        if let Some(u) = info.fields.get("memory-resident") {
            config.push(KV { k: ss("resident"), v: ss(u.split('(').nth(1).map(|x| x.trim_end_matches(')')).unwrap_or(u)) });
        }
    }
    config.push(KV { k: ss("console"), v: ss(g.console()) });
    let ds_path = inv.datastore(&g.datastore).map(|d| d.path.clone()).unwrap_or_default();
    config.push(KV { k: ss("datastore"), v: ss(format!("{} ({})", g.datastore, ds_path)) });

    let (console_sub, console_lines): (String, Vec<String>) = match &g.state {
        VmState::Running(_) | VmState::Bootloader(_) => (
            "vm-bhyve.log — open the console for the live serial port".into(),
            g.log_tail.iter().rev().take(8).rev().cloned().collect(),
        ),
        VmState::Stopped => ("Console unavailable — machine is stopped. Last log lines:".into(), g.log_tail.iter().rev().take(7).rev().cloned().collect()),
        VmState::Suspended => ("Guest is suspended. Start it to restore and attach.".into(), g.log_tail.iter().rev().take(7).rev().cloned().collect()),
        VmState::Locked(h) => (format!("run.lock held by {h}. The console belongs to that host."), Vec::new()),
        VmState::Other(s) => (format!("State unknown ({s})."), g.log_tail.iter().rev().take(7).rev().cloned().collect()),
    };

    let why = match &g.state {
        VmState::Locked(_) | VmState::Other(_) => Op::Start(n.clone()).guard(inv).err().unwrap_or_default(),
        _ if !inv.privilege.can_write() => "Read-only: vm(8) needs root.".into(),
        s if s.is_running() && !inv.host.suspend_supported => "Suspend is unavailable: this kernel's bhyve has no BHYVE_SNAPSHOT support.".into(),
        _ => String::new(),
    };
    let latest = g.snaps.last().map(|s| s.name.clone()).unwrap_or_default();

    Detail {
        name: ss(&n),
        kind: ss(g.state.kind()),
        label: ss(g.state.label()),
        subtitle: ss(format!(
            "{} · datastore {} · {}{}",
            os_of(g, inv),
            g.datastore,
            g.loader,
            if g.autostart { " · starts at host boot" } else { "" }
        )),
        sugg_kind: ss(sug.kind),
        sugg_label: ss(sug.label),
        sugg_text: ss(sug.text),
        sugg_cmd: ss(sug.cmd),
        console_title: ss(g.console()),
        console_sub: ss(console_sub),
        console_lines: model(console_lines.into_iter().map(ss).collect()),
        config: model(config),
        snaps: model(snaps_newest),
        log: model(log.entries.iter().filter(|e| e.cmd.split_whitespace().any(|w| w == n || w.starts_with(&format!("{n}@")) || w.ends_with(&format!("/{n}")) || w.contains(&format!("/{n}/")) || w.ends_with(&format!("={n}")))).take(5).map(|e| log_row(e, false)).collect()),
        can_start: can(Op::Start(n.clone())),
        start_label: ss(if g.state == VmState::Suspended { "Resume" } else { "Start" }),
        can_stop: can(Op::Stop(n.clone())),
        can_restart: can(Op::Restart(n.clone())),
        can_suspend: can(Op::Suspend(n.clone())),
        can_console: g.state.is_running() && inv.privilege.can_write(),
        can_snapshot: can(Op::Snapshot { name: n.clone(), snap: format!("manual-{}", stamp(now())), force: true }),
        can_clone: !latest.is_empty() && can(Op::Clone { name: n.clone(), snap: latest.clone(), to: next_name(&format!("{n}-clone"), &inv.names()) }),
        can_poweroff: can(Op::Poweroff(n.clone())),
        can_destroy: can(Op::Destroy(n.clone())),
        can_rollback: g.state == VmState::Stopped && inv.privilege.can_write(),
        autostart: g.autostart,
        why_disabled: ss(why),
    }
}

fn push_msgs(ui: &MainWindow, st: &State) {
    let app = ui.global::<App>();
    let rows: Vec<MsgRow> = st
        .msgs
        .iter()
        .map(|m| {
            let r = &m.reply;
            let steps: Vec<StepRow> = r
                .steps
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let res = m.results.get(i);
                    let status = match res {
                        Some(x) if x.code == 0 => "ok",
                        Some(_) => "fail",
                        None if m.phase == "running" && i == m.results.len() => "run",
                        None if m.phase == "failed" => "skip",
                        None => "wait",
                    };
                    StepRow {
                        title: ss(&s.title),
                        cmd: ss(s.op.cmd()),
                        effect: ss(&s.effect),
                        undo: ss(s.op.undo()),
                        status: ss(status),
                        code: res.map(|x| x.code).unwrap_or(0),
                        stderr: ss(res.map(|x| x.stderr.clone()).unwrap_or_default()),
                    }
                })
                .collect();
            MsgRow {
                id: m.id,
                user: m.user,
                text: ss(if m.user { &m.text } else { &r.text }),
                stderr: ss(r.stderr.clone().unwrap_or_default()),
                chips: model(r.chips.iter().map(|c| ChipRow { k: ss(&c.key), v: ss(&c.value), filled: c.filled }).collect()),
                code: model(r.code.iter().map(ss).collect()),
                steps: model(steps),
                skipped: ss(r.skipped.join(" · ")),
                risk: ss(&r.risk),
                touches: ss(&r.touches),
                mem_after: ss(&r.mem_after),
                tail: ss(r.tail.clone().unwrap_or_default()),
                follow: model(r.follow.iter().map(ss).collect()),
                phase: ss(&m.phase),
                active: st.active_plan == Some(m.id),
            }
        })
        .collect();
    app.set_msgs(model(rows));
    app.set_runner_busy(st.msgs.iter().any(|m| m.phase == "running"));
}

fn show_toast(ui: &MainWindow, st: &mut State, r: &ExecResult) {
    let app = ui.global::<App>();
    app.set_toast(ToastData { shown: true, code: r.code, cmd: ss(&r.cmd), stderr: ss(&r.stderr) });
    st.toast_serial += 1;
    let serial = st.toast_serial;
    TOAST_SERIAL.with(|t| t.set(serial));
    let weak = ui.as_weak();
    let ms = if r.code == 0 { 3800 } else { 9000 };
    slint::Timer::single_shot(Duration::from_millis(ms), move || {
        if TOAST_SERIAL.with(|t| t.get()) != serial {
            return; // a newer toast replaced this one
        }
        if let Some(ui) = weak.upgrade() {
            let app = ui.global::<App>();
            let mut t = app.get_toast();
            t.shown = false;
            app.set_toast(t);
        }
    });
}

thread_local! {
    static TOAST_SERIAL: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn record(st: &mut State, r: &ExecResult) {
    if let Err(e) = st.log.push(Entry::from_result(SOURCE, r)) {
        eprintln!("vmctl-desktop: could not write activity log: {e}");
    }
}

/// Run a single op from a button: guard first (honest, with the reason), then queue it.
fn run_ops(ui: &MainWindow, st: &mut State, w: &Worker, ops: Vec<Op>, tag: Tag) {
    if let (Some(inv), Some(first)) = (st.inv.as_ref(), ops.first()) {
        if let Err(why) = first.guard(inv) {
            // Not run. Say so plainly; this is a prediction, so nothing is logged as executed.
            let r = ExecResult::fail(format!("{} (not run)", first.cmd()), 1, why);
            show_toast(ui, st, &r);
            return;
        }
    }
    w.run(ops, tag);
}

// ───────────────────────── worker events ─────────────────────────

fn on_event(ui: &MainWindow, st: &Shared, ev: Event) {
    let mut s = st.borrow_mut();
    match ev {
        Event::Busy(b) => ui.global::<App>().set_busy(b),
        Event::Inventory(inv) => {
            s.log.reload();
            s.inv = Some(*inv);
            push_all(ui, &mut s);
        }
        Event::Step { tag, result, .. } => {
            record(&mut s, &result);
            if let Tag::Plan(id) = tag {
                if let Some(m) = s.msgs.iter_mut().find(|m| m.id == id) {
                    m.results.push(result);
                }
                push_msgs(ui, &s);
            } else {
                // For multi-step actions, toast the failure immediately or the final success.
                if result.code != 0 {
                    show_toast(ui, &mut s, &result);
                } else {
                    s.toast_pending(result);
                }
            }
        }
        Event::RunDone { tag, failed_at } => {
            match tag {
                Tag::Plan(id) => {
                    if let Some(m) = s.msgs.iter_mut().find(|m| m.id == id) {
                        m.phase = if failed_at.is_some() { "failed".into() } else { "done".into() };
                    }
                    push_msgs(ui, &s);
                }
                Tag::Create(name) | Tag::Clone(name) => {
                    if failed_at.is_none() {
                        s.selected = name;
                        s.view = "machines".into();
                    }
                    s.flush_toast(ui);
                }
                Tag::Action => s.flush_toast(ui),
            }
        }
    }
}

thread_local! {
    static PENDING_OK: RefCell<Option<ExecResult>> = const { RefCell::new(None) };
}

impl State {
    fn toast_pending(&mut self, r: ExecResult) {
        PENDING_OK.with(|p| *p.borrow_mut() = Some(r));
    }
    fn flush_toast(&mut self, ui: &MainWindow) {
        if let Some(r) = PENDING_OK.with(|p| p.borrow_mut().take()) {
            show_toast(ui, self, &r);
        }
    }
}

// ───────────────────────── main ─────────────────────────

struct Args {
    demo: bool,
    screenshot: Option<String>,
    shot_view: Option<String>,
}

fn parse_args() -> Args {
    let mut a = Args { demo: false, screenshot: None, shot_view: None };
    let mut it = std::env::args().skip(1);
    while let Some(x) = it.next() {
        match x.as_str() {
            "--demo" => a.demo = true,
            "--screenshot" => a.screenshot = it.next(),
            "--view" => a.shot_view = it.next(),
            "-h" | "--help" => {
                println!(
                    "vmctl-desktop — Mission Control for vm-bhyve\n\n\
                     USAGE: vmctl-desktop [--demo]\n\n\
                     \x20 --demo        use a built-in demo host (no vm-bhyve or root needed)\n\n\
                     Privilege: runs vm(8) directly as root, or via `sudo -n` when passwordless\n\
                     sudo is set up; otherwise the app is read-only and says so.\n\n\
                     Keys: Ctrl+K assistant · Ctrl+N new machine · Ctrl+R refresh · / search · Alt+1..4 views"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("vmctl-desktop: unknown argument '{other}' (try --help)");
                std::process::exit(2);
            }
        }
    }
    a
}

fn main() -> Result<(), slint::PlatformError> {
    let args = parse_args();
    let ui = MainWindow::new()?;

    let hv: Box<dyn Hypervisor> = if args.demo { Box::new(DemoHost::new()) } else { Box::new(LiveHost::new()) };
    let sudo = !args.demo && LiveHost::new().privilege() == Privilege::Sudo;
    let log = if args.demo { ActivityLog::memory() } else { ActivityLog::open(activity::default_path()) };

    let st: Shared = Rc::new(RefCell::new(State {
        inv: None,
        log,
        selected: String::new(),
        filter: "all".into(),
        search: String::new(),
        view: args.shot_view.clone().filter(|v| ["machines", "snapshots", "network", "activity"].contains(&v.as_str())).unwrap_or_else(|| "machines".into()),
        msgs: Vec::new(),
        next_id: 1,
        active_plan: None,
        toast_serial: 0,
        console: None,
        demo: args.demo,
        sudo,
    }));

    let weak = ui.as_weak();
    STATE.with(|s| *s.borrow_mut() = Some(st.clone()));
    let worker = Worker::spawn(hv, move |ev| {
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let (Some(ui), Some(st)) = (weak.upgrade(), STATE.with(|s| s.borrow().clone())) {
                on_event(&ui, &st, ev);
            }
        });
    });

    wire_callbacks(&ui, &st, &worker);

    {
        let app = ui.global::<App>();
        app.set_examples(model(planner::examples().into_iter().map(ss).collect()));
        push_all(&ui, &mut st.borrow_mut());
    }
    worker.refresh();

    // Periodic refresh: live state changes outside the app (TUI, shell, guests shutting down).
    let w2 = worker.clone();
    let refresh_timer = slint::Timer::default();
    refresh_timer.start(slint::TimerMode::Repeated, Duration::from_secs(if args.demo { 30 } else { 5 }), move || w2.refresh());

    // Console pump.
    let console_timer = slint::Timer::default();
    {
        let weak = ui.as_weak();
        let st = st.clone();
        console_timer.start(slint::TimerMode::Repeated, Duration::from_millis(40), move || {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            let Some((_, sess, screen)) = s.console.as_mut() else { return };
            match sess.poll() {
                Some(b) if b.is_empty() => {}
                Some(b) => {
                    screen.feed(&b);
                    ui.global::<App>().set_console_text(ss(screen.text()));
                }
                None => {
                    let app = ui.global::<App>();
                    app.set_console_alive(false);
                    app.set_console_status(ss("disconnected"));
                }
            }
        });
    }

    if let Some(path) = args.screenshot {
        screenshot_flow(&ui, &st, &worker, path, args.shot_view);
    }

    ui.run()
}

thread_local! {
    static STATE: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

fn wire_callbacks(ui: &MainWindow, st: &Shared, w: &Worker) {
    let app = ui.global::<App>();

    macro_rules! with {
        ($ui:ident, $s:ident, $body:block) => {{
            let weak = ui.as_weak();
            let st = st.clone();
            move || {
                let Some($ui) = weak.upgrade() else { return };
                let mut $s = st.borrow_mut();
                $body
            }
        }};
    }

    {
        let weak = ui.as_weak();
        let st = st.clone();
        app.on_select(move |name| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            s.selected = name.to_string();
            push_all(&ui, &mut s);
        });
    }
    {
        let weak = ui.as_weak();
        let st = st.clone();
        app.on_set_filter(move |f| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            s.filter = f.to_string();
            push_all(&ui, &mut s);
        });
    }
    {
        let weak = ui.as_weak();
        let st = st.clone();
        app.on_search_changed(move |q| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            s.search = q.to_string();
            // jump to the first match so the detail follows the search
            if let Some(inv) = &s.inv {
                if let Some(g) = inv.guests.iter().find(|g| search_matches(&s.search, g)) {
                    if !search_matches(&s.search, inv.guest(&s.selected).unwrap_or(g)) {
                        s.selected = g.name.clone();
                    }
                }
            }
            if !s.search.is_empty() && s.view != "machines" {
                s.view = "machines".into();
            }
            push_all(&ui, &mut s);
        });
    }
    {
        let weak = ui.as_weak();
        let st = st.clone();
        app.on_set_view(move |v| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            s.view = v.to_string();
            push_all(&ui, &mut s);
        });
    }
    app.on_refresh({
        let w = w.clone();
        move || w.refresh()
    });
    app.on_reprobe({
        let w = w.clone();
        move || w.reprobe()
    });
    app.on_dismiss_toast(with!(ui, s, {
        let _ = &mut s;
        let mut t = ui.global::<App>().get_toast();
        t.shown = false;
        ui.global::<App>().set_toast(t);
    }));

    // one-click actions
    {
        let weak = ui.as_weak();
        let st = st.clone();
        let w = w.clone();
        app.on_act(move |op, name| {
            let Some(ui) = weak.upgrade() else { return };
            if op == "console" {
                open_console(&ui, &st, &name);
                return;
            }
            let mut s = st.borrow_mut();
            let Some(inv) = s.inv.clone() else { return };
            let g = inv.guest(&name);
            if let Some(o) = recipes::op_for(&op, &name, g) {
                run_ops(&ui, &mut s, &w, vec![o], Tag::Action);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let st = st.clone();
        let w = w.clone();
        app.on_snapshot_now(move |name| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            let Some(inv) = s.inv.clone() else { return };
            if let Some(o) = recipes::op_for("snapshot", &name, inv.guest(&name)) {
                run_ops(&ui, &mut s, &w, vec![o], Tag::Action);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let st = st.clone();
        let w = w.clone();
        app.on_toggle_autostart(move |name| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            let Some(inv) = s.inv.clone() else { return };
            if let Some(o) = recipes::op_for("autostart", &name, inv.guest(&name)) {
                run_ops(&ui, &mut s, &w, vec![o], Tag::Action);
            }
        });
    }
    let clone = {
        let weak = ui.as_weak();
        let st = st.clone();
        let w = w.clone();
        move |name: SharedString, snap: Option<String>| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            let Some(inv) = s.inv.clone() else { return };
            let Some(g) = inv.guest(&name) else { return };
            let Some(snap) = snap.or_else(|| g.snaps.last().map(|x| x.name.clone())) else { return };
            let to = next_name(&format!("{name}-clone"), &inv.names());
            run_ops(&ui, &mut s, &w, vec![Op::Clone { name: name.to_string(), snap, to: to.clone() }], Tag::Clone(to));
        }
    };
    {
        let c = clone.clone();
        app.on_clone_latest(move |n| c(n, None));
    }
    app.on_clone_snap(move |n, snap| clone(n, Some(snap.to_string())));

    // primary (suggestion strip)
    {
        let weak = ui.as_weak();
        let st = st.clone();
        let w = w.clone();
        app.on_primary(move |name| {
            let Some(ui) = weak.upgrade() else { return };
            let kind = {
                let s = st.borrow();
                s.inv.as_ref().and_then(|i| i.guest(&name)).map(|g| suggest(g).kind)
            };
            match kind {
                Some("console") => open_console(&ui, &st, &name),
                Some("start") => {
                    let mut s = st.borrow_mut();
                    run_ops(&ui, &mut s, &w, vec![Op::Start(name.to_string())], Tag::Action);
                }
                Some("explain") => {
                    ui.global::<App>().set_drawer(true);
                    ask(&ui, &st, &format!("why is {name} locked?"));
                }
                _ => w.refresh(),
            }
        });
    }

    // assistant
    {
        let weak = ui.as_weak();
        let st = st.clone();
        app.on_ask(move |text| {
            let Some(ui) = weak.upgrade() else { return };
            ask(&ui, &st, &text);
        });
    }
    {
        let weak = ui.as_weak();
        let st = st.clone();
        let w = w.clone();
        app.on_run_plan(move |id| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            if s.msgs.iter().any(|m| m.phase == "running") {
                return;
            }
            let Some(m) = s.msgs.iter_mut().find(|m| m.id == id) else { return };
            if m.phase != "idle" {
                return;
            }
            m.phase = "running".into();
            m.results.clear();
            let ops: Vec<Op> = m.reply.steps.iter().map(|x| x.op.clone()).collect();
            w.run(ops, Tag::Plan(id));
            push_msgs(&ui, &s);
        });
    }
    {
        let weak = ui.as_weak();
        let st = st.clone();
        app.on_cancel_plan(move |id| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            if let Some(m) = s.msgs.iter_mut().find(|m| m.id == id) {
                if m.phase == "idle" {
                    m.phase = "cancelled".into();
                }
            }
            if s.active_plan == Some(id) {
                s.active_plan = None;
            }
            push_msgs(&ui, &s);
        });
    }

    // confirm modal
    {
        let st = st.clone();
        app.on_confirm_preview(move |kind, name, snap, safety| {
            let s = st.borrow();
            let Some(inv) = s.inv.as_ref() else { return model(vec![]) };
            model(recipes::confirm_ops(inv, &kind, &name, &snap, safety).iter().map(|o| ss(o.cmd())).collect())
        });
    }
    {
        let weak = ui.as_weak();
        let st = st.clone();
        let w = w.clone();
        app.on_confirm_run(move |kind, name, snap, safety| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            let Some(inv) = s.inv.clone() else { return };
            let ops = recipes::confirm_ops(&inv, &kind, &name, &snap, safety);
            run_ops(&ui, &mut s, &w, ops, Tag::Action);
        });
    }

    // wizard
    {
        let st = st.clone();
        app.on_suggest_name(move |tpl| {
            let s = st.borrow();
            ss(next_name(&tpl, &s.inv.as_ref().map(|i| i.names()).unwrap_or_default()))
        });
    }
    app.on_template_title(|id| ss(title_of(&id)));
    {
        let st = st.clone();
        app.on_name_error(move |n| {
            let s = st.borrow();
            ss(if n.is_empty() {
                "Give it a name.".to_string()
            } else if !valid_name(&n) {
                "Use letters, digits, - and _ only.".into()
            } else if s.inv.as_ref().and_then(|i| i.guest(&n)).is_some() {
                "A machine with this name already exists.".into()
            } else {
                String::new()
            })
        });
    }
    {
        let st = st.clone();
        app.on_wizard_cmds(move |spec| {
            let s = st.borrow();
            let Some(inv) = s.inv.as_ref() else { return model(vec![]) };
            model(recipes::wizard_ops(inv, &core_spec(&spec)).iter().map(|o| ss(o.cmd())).collect())
        });
    }
    {
        let weak = ui.as_weak();
        let st = st.clone();
        let w = w.clone();
        app.on_wizard_create(move |spec| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            let Some(inv) = s.inv.clone() else { return };
            let ops = recipes::wizard_ops(&inv, &core_spec(&spec));
            run_ops(&ui, &mut s, &w, ops, Tag::Create(spec.name.to_string()));
        });
    }

    // network
    {
        let st = st.clone();
        app.on_switch_name_error(move |n| {
            let s = st.borrow();
            ss(if !valid_name(&n) {
                "Use letters, digits, - and _ only.".to_string()
            } else if s.inv.as_ref().map(|i| i.switches.iter().any(|w| w.name == n.as_str())).unwrap_or(false) {
                "A switch with this name already exists.".into()
            } else {
                String::new()
            })
        });
    }
    {
        let weak = ui.as_weak();
        let st = st.clone();
        let w = w.clone();
        app.on_switch_create(move |n, private| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            run_ops(&ui, &mut s, &w, vec![Op::SwitchCreate { name: n.to_string(), private }], Tag::Action);
        });
    }

    // console
    {
        let st = st.clone();
        let weak = ui.as_weak();
        app.on_console_key(move |text, ctrl| {
            let Some(ui) = weak.upgrade() else { return };
            let mut s = st.borrow_mut();
            let Some((_, sess, screen)) = s.console.as_mut() else { return };
            let bytes = console::key_bytes(&text, ctrl);
            if bytes.is_empty() {
                return;
            }
            let echo = sess.send(&bytes);
            if !echo.is_empty() {
                screen.feed(&echo);
                ui.global::<App>().set_console_text(ss(screen.text()));
            }
        });
    }
    {
        let st = st.clone();
        let weak = ui.as_weak();
        app.on_console_close(move || {
            st.borrow_mut().console = None; // Drop closes the helper
            if let Some(ui) = weak.upgrade() {
                ui.global::<App>().set_console_open(false);
            }
        });
    }
    {
        let st = st.clone();
        let weak = ui.as_weak();
        app.on_console_external(move |name| {
            let Some(ui) = weak.upgrade() else { return };
            let s = st.borrow();
            let mut argv = if s.demo { vec!["sh".to_string(), "-c".into(), format!("echo demo: vm console {name}; sleep 3")] } else { vec![] };
            if argv.is_empty() {
                if s.sudo {
                    argv.push("sudo".into());
                }
                argv.extend(["vm".into(), "console".into(), name.to_string()]);
            }
            drop(s);
            // Free the nmdm device first: only one reader can own it.
            st.borrow_mut().console = None;
            ui.global::<App>().set_console_open(false);
            let term = std::env::var("TERMINAL").ok().filter(|t| !t.is_empty());
            let candidates: Vec<String> = term.into_iter().chain(["kitty", "alacritty", "foot", "konsole", "xterm"].iter().map(|s| s.to_string())).collect();
            for t in candidates {
                let mut c = std::process::Command::new(&t);
                if t == "konsole" || t == "xterm" || t == "alacritty" || t == "foot" {
                    c.arg("-e");
                }
                if c.args(&argv).spawn().is_ok() {
                    return;
                }
            }
            let r = ExecResult::fail(argv.join(" "), 127, "no terminal emulator found (set $TERMINAL)");
            let mut s = st.borrow_mut();
            show_toast(&ui, &mut s, &r);
        });
    }
}

fn ask(ui: &MainWindow, st: &Shared, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let mut s = st.borrow_mut();
    let Some(inv) = s.inv.clone() else { return };
    // An unrun plan becomes "cancelled" when a new question supersedes it.
    if let Some(active) = s.active_plan {
        if let Some(m) = s.msgs.iter_mut().find(|m| m.id == active && m.phase == "idle") {
            m.phase = "cancelled".into();
        }
    }
    let selected = s.selected.clone();
    let reply = planner::plan(text, &inv, Some(&selected), &s.log.entries);
    let uid = s.next_id;
    let aid = uid + 1;
    s.next_id += 2;
    s.msgs.push(Msg { id: uid, user: true, reply: Reply::default(), text: text.into(), phase: "none".into(), results: vec![] });
    let has_plan = reply.has_plan();
    s.msgs.push(Msg { id: aid, user: false, reply, text: String::new(), phase: if has_plan { "idle".into() } else { "none".into() }, results: vec![] });
    s.active_plan = has_plan.then_some(aid);
    if s.msgs.len() > 60 {
        let drop_n = s.msgs.len() - 60;
        s.msgs.drain(..drop_n);
    }
    push_msgs(ui, &s);
    let app = ui.global::<App>();
    app.set_msgs_serial(app.get_msgs_serial() + 1);
}

/// Slint's generated `WizSpec` → the core recipe input (negative ints clamp to 0; core clamps to ≥1).
fn core_spec(s: &WizSpec) -> recipes::WizSpec {
    let n = |v: i32| v.max(0) as u32;
    recipes::WizSpec {
        tpl: s.tpl.to_string(),
        cpu: n(s.cpu),
        mem_gib: n(s.mem_g),
        disk_gib: n(s.disk_g),
        ds: s.ds.to_string(),
        sw: s.sw.to_string(),
        iso: s.iso.to_string(),
        name: s.name.to_string(),
        install: s.install,
        autostart: s.autostart,
    }
}

fn open_console(ui: &MainWindow, st: &Shared, name: &str) {
    let mut s = st.borrow_mut();
    let Some(inv) = s.inv.clone() else { return };
    let Some(g) = inv.guest(name) else { return };
    let app = ui.global::<App>();
    if !g.state.is_running() {
        let r = ExecResult::fail(format!("vm console {name} (not run)"), 1, format!("{name} isn't running"));
        show_toast(ui, &mut s, &r);
        return;
    }
    let dev = g.info.as_ref().and_then(|i| i.consoles.first().map(|(_, d)| d.clone())).unwrap_or_else(|| format!("/dev/nmdm-{name}.1B"));
    let sess = if s.demo { Ok(console::Session::demo(name)) } else { console::Session::open(&dev, s.sudo) };
    match sess {
        Ok(sess) => {
            let mut screen = console::Screen::default();
            screen.feed(sess.greeting().as_bytes());
            let first = if s.demo { String::new() } else { "\r".into() };
            app.set_console_name(ss(name));
            app.set_console_dev(ss(format!("{dev} · 115200 8N1")));
            app.set_console_text(ss(screen.text()));
            app.set_console_status(ss("attached"));
            app.set_console_alive(true);
            app.set_console_open(true);
            s.console = Some((name.to_string(), sess, screen));
            if !first.is_empty() {
                // nudge getty so a login prompt appears
                if let Some((_, sess, _)) = s.console.as_mut() {
                    sess.send(first.as_bytes());
                }
            }
            let r = ExecResult { cmd: format!("console attach {dev}"), code: 0, stdout: String::new(), stderr: String::new() };
            record(&mut s, &r);
        }
        Err(e) => {
            let r = ExecResult::fail(format!("console attach {dev}"), 127, e.to_string());
            record(&mut s, &r);
            show_toast(ui, &mut s, &r);
        }
    }
}

// ───────────────────────── screenshots (docs / review) ─────────────────────────

/// `--screenshot out.png [--view machines|snapshots|network|activity|assistant|wizard|confirm|console]`
/// Renders the window once data has loaded, saves it, and exits. Uses the
/// renderer's own snapshot (works on X11 and Xvfb).
fn screenshot_flow(ui: &MainWindow, st: &Shared, w: &Worker, path: String, view: Option<String>) {
    let weak = ui.as_weak();
    let st = st.clone();
    let _ = w;
    let view = view.unwrap_or_else(|| "machines".into());
    slint::Timer::single_shot(Duration::from_millis(1500), move || {
        let Some(ui) = weak.upgrade() else { return };
        let app = ui.global::<App>();
        match view.as_str() {
            "assistant" => {
                app.set_view(ss("machines"));
                app.set_drawer(true);
                ask(&ui, &st, "make a 4GB ubuntu VM with 2 CPUs and snapshot it before installing");
            }
            "explain" => {
                app.set_drawer(true);
                let locked = st.borrow().inv.as_ref().and_then(|i| i.guests.iter().find(|g| matches!(g.state, VmState::Locked(_))).map(|g| g.name.clone()));
                if let Some(n) = locked {
                    st.borrow_mut().selected = n.clone();
                    push_all(&ui, &mut st.borrow_mut());
                    ask(&ui, &st, &format!("why is {n} locked?"));
                }
            }
            "wizard" | "wizard-summary" => app.set_wizard_open(true),
            "confirm" => {
                let n = st.borrow().selected.clone();
                app.set_confirm_name(ss(&n));
                app.set_confirm_snap(ss(""));
                app.set_confirm_kind(ss("poweroff"));
            }
            "destroy" => {
                let n = st.borrow().inv.as_ref().and_then(|i| i.guests.iter().find(|g| g.state == VmState::Stopped).map(|g| g.name.clone())).unwrap_or_default();
                app.set_confirm_name(ss(&n));
                app.set_confirm_kind(ss("destroy"));
            }
            "toast" => {
                let r = ExecResult::fail("vm start alpine-test", 1, "/usr/local/sbin/vm: ERROR: alpine-test appears to be running on nas01 (locked)");
                show_toast(&ui, &mut st.borrow_mut(), &r);
            }
            "console" => {
                let n = st.borrow().inv.as_ref().and_then(|i| i.guests.iter().find(|g| g.state.is_running()).map(|g| g.name.clone()));
                if let Some(n) = n {
                    open_console(&ui, &st, &n);
                }
            }
            _ => {}
        }
        let weak = ui.as_weak();
        let path = path.clone();
        slint::Timer::single_shot(Duration::from_millis(900), move || {
            let Some(ui) = weak.upgrade() else { return };
            match ui.window().take_snapshot() {
                Ok(buf) => {
                    let (w, h) = (buf.width(), buf.height());
                    let bytes: Vec<u8> = buf.as_slice().iter().flat_map(|p| [p.r, p.g, p.b, p.a]).collect();
                    if let Err(e) = write_png(&path, w, h, &bytes) {
                        eprintln!("screenshot: {e}");
                    } else {
                        println!("wrote {path} ({w}x{h})");
                    }
                }
                Err(e) => eprintln!("screenshot: {e}"),
            }
            let _ = slint::quit_event_loop();
        });
    });
}

fn write_png(path: &str, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    let f = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(std::io::Error::other)?;
    wr.write_image_data(rgba).map_err(std::io::Error::other)
}
