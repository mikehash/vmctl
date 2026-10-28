//! TUI state machine over the v2 core.
//!
//! `App` never touches the terminal or the hypervisor. Keys and worker events
//! go in; [`Effect`]s come out (run these ops, open this console, quit). That
//! keeps every behaviour unit-testable against `DemoHost` with no TTY.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vmctl_core::activity::{ActivityLog, Entry};
use vmctl_core::inventory::{Guest, Inventory, Privilege, Snap};
use vmctl_core::model::{next_name, valid_name};
use vmctl_core::ops::Op;
use vmctl_core::planner::{self, Reply};
use vmctl_core::recipes::{self, WizSpec};
use vmctl_core::worker::{Event, Tag};

/// Activity-log source tag for commands this front-end ran.
pub const SOURCE: &str = "tui";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Machines,
    Snapshots,
    Network,
    Storage,
    Activity,
    Assistant,
}

impl View {
    pub const ALL: [View; 6] = [View::Machines, View::Snapshots, View::Network, View::Storage, View::Activity, View::Assistant];

    pub fn title(self) -> &'static str {
        match self {
            View::Machines => "Machines",
            View::Snapshots => "Snapshots",
            View::Network => "Network",
            View::Storage => "Storage",
            View::Activity => "Activity",
            View::Assistant => "Ask vmctl",
        }
    }

    pub fn index(self) -> usize {
        View::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }

    fn step(self, d: isize) -> View {
        let n = View::ALL.len() as isize;
        View::ALL[((self.index() as isize + d).rem_euclid(n)) as usize]
    }
}

/// What the event loop must do on the App's behalf.
#[derive(Debug)]
pub enum Effect {
    Quit,
    Run { ops: Vec<Op>, tag: Tag },
    Refresh,
    Reprobe,
    /// Suspend the TUI, exec this argv on the real terminal, then restore.
    Console(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub code: i32,
    pub cmd: String,
    pub stderr: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StepState {
    Pending,
    Ok,
    Failed,
    Skipped,
}

/// One assistant exchange: the user's words and the planner's reply, plus
/// live per-step status once the plan runs.
pub struct Msg {
    pub id: i32,
    pub user: String,
    pub reply: Reply,
    pub states: Vec<StepState>,
    pub stderr: Vec<String>,
    next: usize,
    pub ran: bool,
}

pub struct Confirm {
    pub kind: String,
    pub name: String,
    pub snap: String,
    /// Snapshot first (poweroff only).
    pub safety: bool,
    pub ops: Vec<Op>,
}

pub enum InputPurpose {
    Clone { name: String, snap: String },
    SwitchCreate,
}

pub struct Input {
    pub purpose: InputPurpose,
    pub label: String,
    pub value: String,
}

/// Create wizard. Choice fields index into the inventory lists.
pub struct Wizard {
    pub field: usize,
    pub spec: WizSpec,
    pub tpl: usize,
    pub ds: usize,
    pub sw: usize,
    /// 0 = no ISO; otherwise isos[iso-1].
    pub iso: usize,
}

pub const WIZ_FIELDS: [&str; 10] = ["Template", "Name", "CPUs", "Memory", "Disk", "Datastore", "Switch", "Install ISO", "Boot installer", "Start at boot"];

pub enum Modal {
    None,
    Confirm(Confirm),
    Wizard(Wizard),
    Input(Input),
    Help,
}

/// Machine actions, in action-bar order: (label, recipe key).
pub const ACTIONS: [(&str, &str); 11] = [
    ("Start", "start"),
    ("Stop", "stop"),
    ("Restart", "restart"),
    ("Suspend", "suspend"),
    ("Power off", "poweroff"),
    ("Snapshot", "snapshot"),
    ("Autostart", "autostart"),
    ("Console", "console"),
    ("Clone", "clone"),
    ("Destroy", "destroy"),
    ("Unlock", "unlock"),
];

const CONFIRMED: [&str; 3] = ["poweroff", "destroy", "unlock"];

pub struct App {
    pub inv: Option<Inventory>,
    pub log: ActivityLog,
    pub demo: bool,
    pub view: View,
    pub sel: usize,
    pub action: usize,
    pub snap_sel: usize,
    pub sw_sel: usize,
    pub iso_sel: usize,
    pub log_sel: usize,
    pub modal: Modal,
    pub toast: Option<Toast>,
    pub busy: bool,
    pub msgs: Vec<Msg>,
    pub input: String,
    next_id: i32,
    pending_select: Option<String>,
}

/// Console command line for a guest. Mirrors `Hypervisor::console_argv`, but
/// computed UI-side because the hypervisor lives on the worker thread.
pub fn console_argv(demo: bool, privilege: Privilege, name: &str) -> Vec<String> {
    if demo {
        return vec!["sh".into(), "-c".into(), format!("echo 'demo mode: would run vm console {name}'; sleep 2")];
    }
    let mut v = Vec::new();
    if privilege == Privilege::Sudo {
        v.push("sudo".into());
    }
    v.extend(["vm".to_string(), "console".into(), name.to_string()]);
    v
}

impl App {
    pub fn new(log: ActivityLog, demo: bool) -> Self {
        Self {
            inv: None,
            log,
            demo,
            view: View::Machines,
            sel: 0,
            action: 0,
            snap_sel: 0,
            sw_sel: 0,
            iso_sel: 0,
            log_sel: 0,
            modal: Modal::None,
            toast: None,
            busy: false,
            msgs: Vec::new(),
            input: String::new(),
            next_id: 1,
            pending_select: None,
        }
    }

    // ───────────────────────── queries ─────────────────────────

    pub fn guest(&self) -> Option<&Guest> {
        self.inv.as_ref()?.guests.get(self.sel)
    }

    pub fn privilege(&self) -> Privilege {
        self.inv.as_ref().map(|i| i.privilege).unwrap_or(Privilege::None)
    }

    /// Every (guest, snapshot) pair, guest order then oldest-first.
    pub fn snap_rows(&self) -> Vec<(&Guest, &Snap)> {
        self.inv.as_ref().map(|i| i.guests.iter().flat_map(|g| g.snaps.iter().map(move |s| (g, s))).collect()).unwrap_or_default()
    }

    /// Is a machine action available for the selected guest? `Err` = why not.
    pub fn action_state(&self, key: &str) -> Result<(), String> {
        let inv = self.inv.as_ref().ok_or("still loading")?;
        let g = self.guest().ok_or("no machine selected")?;
        match key {
            "console" => {
                if !inv.privilege.can_write() {
                    Err("the console needs root (or passwordless sudo)".into())
                } else if !g.state.is_running() {
                    Err(format!("{} isn't running", g.name))
                } else {
                    Ok(())
                }
            }
            "clone" => match g.snaps.last() {
                None => Err(format!("{} has no snapshot to clone from", g.name)),
                Some(s) => Op::Clone { name: g.name.clone(), snap: s.name.clone(), to: next_name(&g.name, &inv.names()) }.guard(inv),
            },
            k => recipes::op_for(k, &g.name, Some(g)).ok_or_else(|| format!("unknown action {k}"))?.guard(inv),
        }
    }

    fn clamp(&mut self) {
        let Some(inv) = self.inv.as_ref() else { return };
        let fix = |i: &mut usize, n: usize| *i = if n == 0 { 0 } else { (*i).min(n - 1) };
        let (g, sw, iso, lg) = (inv.guests.len(), inv.switches.len(), inv.isos.len(), self.log.entries.len());
        let snaps = self.snap_rows().len();
        fix(&mut self.sel, g);
        fix(&mut self.snap_sel, snaps);
        fix(&mut self.sw_sel, sw);
        fix(&mut self.iso_sel, iso);
        fix(&mut self.log_sel, lg);
    }

    fn not_run(&mut self, cmd: String, why: String) {
        self.toast = Some(Toast { code: 1, cmd: format!("{cmd} (not run)"), stderr: why });
    }

    /// Guard the first op (honest, predictive), then hand it to the worker.
    fn request(&mut self, ops: Vec<Op>, tag: Tag) -> Option<Effect> {
        let first = ops.first()?;
        if let Some(inv) = self.inv.as_ref() {
            if let Err(why) = first.guard(inv) {
                let cmd = first.cmd();
                self.not_run(cmd, why);
                return None;
            }
        }
        Some(Effect::Run { ops, tag })
    }

    // ───────────────────────── worker events ─────────────────────────

    pub fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Busy(b) => self.busy = b,
            Event::Inventory(inv) => {
                self.log.reload();
                self.inv = Some(*inv);
                if let Some(n) = self.pending_select.take() {
                    match self.inv.as_ref().and_then(|i| i.guests.iter().position(|g| g.name == n)) {
                        Some(i) => {
                            self.sel = i;
                            self.view = View::Machines;
                        }
                        None => self.pending_select = Some(n),
                    }
                }
                self.clamp();
            }
            Event::Step { tag, result } => {
                if let Err(e) = self.log.push(Entry::from_result(SOURCE, &result)) {
                    self.toast = Some(Toast { code: 1, cmd: "activity log".into(), stderr: format!("could not write activity log: {e}") });
                }
                match tag {
                    Tag::Plan(id) => {
                        if let Some(m) = self.msgs.iter_mut().find(|m| m.id == id) {
                            if m.next < m.states.len() {
                                m.states[m.next] = if result.code == 0 { StepState::Ok } else { StepState::Failed };
                                m.stderr[m.next] = result.stderr.trim().to_string();
                            }
                            m.next += 1;
                        }
                    }
                    _ => self.toast = Some(Toast { code: result.code, cmd: result.cmd, stderr: result.stderr.trim().to_string() }),
                }
            }
            Event::RunDone { tag, failed_at } => match tag {
                Tag::Plan(id) => {
                    if let Some(m) = self.msgs.iter_mut().find(|m| m.id == id) {
                        if let Some(i) = failed_at {
                            for s in m.states.iter_mut().skip(i + 1) {
                                *s = StepState::Skipped;
                            }
                        }
                    }
                }
                Tag::Create(n) | Tag::Clone(n) if failed_at.is_none() => self.pending_select = Some(n),
                _ => {}
            },
        }
    }

    // ───────────────────────── keys ─────────────────────────

    pub fn on_key(&mut self, k: KeyEvent) -> Option<Effect> {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            return Some(Effect::Quit);
        }
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::None => {}
            Modal::Help => return None, // any key closes help
            Modal::Confirm(c) => return self.key_confirm(c, k),
            Modal::Wizard(w) => return self.key_wizard(w, k),
            Modal::Input(i) => return self.key_input(i, k),
        }
        // Global navigation first.
        match k.code {
            KeyCode::Tab => {
                self.view = self.view.step(1);
                return None;
            }
            KeyCode::BackTab => {
                self.view = self.view.step(-1);
                return None;
            }
            KeyCode::F(5) => return Some(Effect::Refresh),
            _ => {}
        }
        if self.view == View::Assistant {
            return self.key_assistant(k);
        }
        match k.code {
            KeyCode::Char('q') => return Some(Effect::Quit),
            KeyCode::Char(c @ '1'..='6') => {
                self.view = View::ALL[(c as u8 - b'1') as usize];
                return None;
            }
            KeyCode::Char('r') => return Some(Effect::Refresh),
            KeyCode::Char('p') => return Some(Effect::Reprobe),
            KeyCode::Char('?') => {
                self.modal = Modal::Help;
                return None;
            }
            KeyCode::Char('n') => {
                self.open_wizard();
                return None;
            }
            KeyCode::Char('a') if self.view != View::Network => {
                self.view = View::Assistant;
                return None;
            }
            KeyCode::Esc => {
                self.toast = None;
                return None;
            }
            _ => {}
        }
        match self.view {
            View::Machines => self.key_machines(k),
            View::Snapshots => self.key_snapshots(k),
            View::Network => self.key_network(k),
            View::Storage => {
                let n = self.inv.as_ref().map(|i| i.isos.len()).unwrap_or(0);
                move_in(&mut self.iso_sel, n, k.code);
                None
            }
            View::Activity => {
                move_in(&mut self.log_sel, self.log.entries.len(), k.code);
                None
            }
            View::Assistant => None,
        }
    }

    fn key_machines(&mut self, k: KeyEvent) -> Option<Effect> {
        let n = self.inv.as_ref().map(|i| i.guests.len()).unwrap_or(0);
        match k.code {
            KeyCode::Left | KeyCode::Char('h') => self.action = (self.action + ACTIONS.len() - 1) % ACTIONS.len(),
            KeyCode::Right | KeyCode::Char('l') => self.action = (self.action + 1) % ACTIONS.len(),
            KeyCode::Enter => return self.activate(ACTIONS[self.action].1),
            KeyCode::Char('c') => return self.activate("console"),
            KeyCode::Char('s') => return self.activate("start"),
            KeyCode::Char('S') => return self.activate("stop"),
            KeyCode::Char('t') => return self.activate("snapshot"),
            code => move_in(&mut self.sel, n, code),
        }
        None
    }

    /// Run a machine action for the selected guest.
    pub fn activate(&mut self, key: &str) -> Option<Effect> {
        let g = self.guest()?.clone();
        if let Err(why) = self.action_state(key) {
            let cmd = match key {
                "console" => format!("vm console {}", g.name),
                k => recipes::op_for(k, &g.name, Some(&g)).map(|o| o.cmd()).unwrap_or_else(|| k.to_string()),
            };
            self.not_run(cmd, why);
            return None;
        }
        match key {
            "console" => Some(Effect::Console(console_argv(self.demo, self.privilege(), &g.name))),
            "clone" => {
                let snap = g.snaps.last().map(|s| s.name.clone()).unwrap_or_default();
                let names = self.inv.as_ref().map(|i| i.names()).unwrap_or_default();
                self.modal = Modal::Input(Input {
                    label: format!("Clone {}@{snap} as", g.name),
                    value: next_name(&g.name, &names),
                    purpose: InputPurpose::Clone { name: g.name.clone(), snap },
                });
                None
            }
            k if CONFIRMED.contains(&k) => {
                self.open_confirm(k, &g.name, "");
                None
            }
            k => {
                let op = recipes::op_for(k, &g.name, Some(&g))?;
                self.request(vec![op], Tag::Action)
            }
        }
    }

    pub fn open_confirm(&mut self, kind: &str, name: &str, snap: &str) {
        let Some(inv) = self.inv.as_ref() else { return };
        let safety = kind == "poweroff";
        let ops = if kind == "switchdestroy" { vec![Op::SwitchDestroy(name.into())] } else { recipes::confirm_ops(inv, kind, name, snap, safety) };
        // Guard the destructive op itself (the safety snapshot is not the point).
        if let Some(main) = ops.iter().find(|o| o.is_destructive()).or(ops.first()) {
            if let Err(why) = main.guard(inv) {
                let cmd = main.cmd();
                self.not_run(cmd, why);
                return;
            }
        }
        self.modal = Modal::Confirm(Confirm { kind: kind.into(), name: name.into(), snap: snap.into(), safety, ops });
    }

    fn key_confirm(&mut self, mut c: Confirm, k: KeyEvent) -> Option<Effect> {
        match k.code {
            KeyCode::Char('y') | KeyCode::Enter => Some(Effect::Run { ops: c.ops, tag: Tag::Action }),
            KeyCode::Char(' ') if c.kind == "poweroff" => {
                c.safety = !c.safety;
                if let Some(inv) = self.inv.as_ref() {
                    c.ops = recipes::confirm_ops(inv, &c.kind, &c.name, &c.snap, c.safety);
                }
                self.modal = Modal::Confirm(c);
                None
            }
            KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => None,
            _ => {
                self.modal = Modal::Confirm(c);
                None
            }
        }
    }

    fn key_snapshots(&mut self, k: KeyEvent) -> Option<Effect> {
        let rows: Vec<(String, String)> = self.snap_rows().iter().map(|(g, s)| (g.name.clone(), s.name.clone())).collect();
        let cur = rows.get(self.snap_sel).cloned();
        match (k.code, cur) {
            (KeyCode::Char('b'), Some((g, s))) => self.open_confirm("rollback", &g, &s),
            (KeyCode::Char('d') | KeyCode::Delete, Some((g, s))) => self.open_confirm("snapdelete", &g, &s),
            (KeyCode::Char('c'), Some((g, s))) => {
                let names = self.inv.as_ref().map(|i| i.names()).unwrap_or_default();
                self.modal = Modal::Input(Input { label: format!("Clone {g}@{s} as"), value: next_name(&g, &names), purpose: InputPurpose::Clone { name: g, snap: s } });
            }
            (code, _) => move_in(&mut self.snap_sel, rows.len(), code),
        }
        None
    }

    fn key_network(&mut self, k: KeyEvent) -> Option<Effect> {
        let names: Vec<String> = self.inv.as_ref().map(|i| i.switches.iter().map(|s| s.name.clone()).collect()).unwrap_or_default();
        match k.code {
            KeyCode::Char('a') => self.modal = Modal::Input(Input { label: "New private switch name".into(), value: String::new(), purpose: InputPurpose::SwitchCreate }),
            KeyCode::Char('d') | KeyCode::Delete => {
                if let Some(n) = names.get(self.sw_sel).cloned() {
                    self.open_confirm("switchdestroy", &n, "");
                }
            }
            code => move_in(&mut self.sw_sel, names.len(), code),
        }
        None
    }

    fn key_input(&mut self, mut i: Input, k: KeyEvent) -> Option<Effect> {
        match k.code {
            KeyCode::Esc => None,
            KeyCode::Enter => {
                let v = i.value.trim().to_string();
                if !valid_name(&v) {
                    self.not_run(i.label.clone(), "Use letters, digits, - and _ only.".into());
                    self.modal = Modal::Input(i);
                    return None;
                }
                match i.purpose {
                    InputPurpose::Clone { name, snap } => self.request(vec![Op::Clone { name, snap, to: v.clone() }], Tag::Clone(v)),
                    InputPurpose::SwitchCreate => self.request(vec![Op::SwitchCreate { name: v, private: true }], Tag::Action),
                }
            }
            KeyCode::Backspace => {
                i.value.pop();
                self.modal = Modal::Input(i);
                None
            }
            KeyCode::Char(c) => {
                i.value.push(c);
                self.modal = Modal::Input(i);
                None
            }
            _ => {
                self.modal = Modal::Input(i);
                None
            }
        }
    }

    // ───────────────────────── wizard ─────────────────────────

    pub fn open_wizard(&mut self) {
        let Some(inv) = self.inv.as_ref() else { return };
        if inv.templates.is_empty() {
            self.not_run("vm create".into(), "no templates found in <datastore>/.templates".into());
            return;
        }
        let mut w = Wizard { field: 0, spec: WizSpec { disk_gib: 20, ..Default::default() }, tpl: 0, ds: 0, sw: 0, iso: 0 };
        self.wiz_apply_template(&mut w);
        self.modal = Modal::Wizard(w);
    }

    /// Template change → its defaults (cpu, memory, switch, a matching ISO, a free name).
    fn wiz_apply_template(&self, w: &mut Wizard) {
        let Some(inv) = self.inv.as_ref() else { return };
        let t = &inv.templates[w.tpl.min(inv.templates.len() - 1)];
        w.spec.tpl = t.id.clone();
        w.spec.cpu = t.cpu.max(1);
        w.spec.mem_gib = t.mem_mib.div_ceil(1024).max(1) as u32;
        w.spec.name = next_name(&t.id, &inv.names());
        if let Some(i) = inv.switches.iter().position(|s| s.name == t.switch) {
            w.sw = i;
        }
        w.iso = inv.isos.iter().position(|i| i.name.to_ascii_lowercase().contains(&t.id.to_ascii_lowercase())).map(|i| i + 1).unwrap_or(0);
        self.wiz_sync(w);
    }

    fn wiz_sync(&self, w: &mut Wizard) {
        let Some(inv) = self.inv.as_ref() else { return };
        w.spec.ds = inv.datastores.get(w.ds).map(|d| d.name.clone()).unwrap_or_else(|| "default".into());
        w.spec.sw = inv.switches.get(w.sw).map(|s| s.name.clone()).unwrap_or_default();
        w.spec.iso = if w.iso == 0 { String::new() } else { inv.isos.get(w.iso - 1).map(|i| i.name.clone()).unwrap_or_default() };
        w.spec.install = w.spec.install && !w.spec.iso.is_empty();
    }

    /// Why the wizard can't create yet (None = ready).
    pub fn wiz_error(&self, w: &Wizard) -> Option<String> {
        let n = &w.spec.name;
        if n.is_empty() {
            Some("Give it a name.".into())
        } else if !valid_name(n) {
            Some("Use letters, digits, - and _ only.".into())
        } else if self.inv.as_ref().and_then(|i| i.guest(n)).is_some() {
            Some("A machine with this name already exists.".into())
        } else {
            None
        }
    }

    pub fn wiz_ops(&self, w: &Wizard) -> Vec<Op> {
        self.inv.as_ref().map(|inv| recipes::wizard_ops(inv, &w.spec)).unwrap_or_default()
    }

    fn key_wizard(&mut self, mut w: Wizard, k: KeyEvent) -> Option<Effect> {
        let (nt, nd, ns, ni) = self.inv.as_ref().map(|i| (i.templates.len(), i.datastores.len(), i.switches.len(), i.isos.len())).unwrap_or_default();
        let d: isize = match k.code {
            KeyCode::Left => -1,
            KeyCode::Right => 1,
            _ => 0,
        };
        let cyc = |v: usize, n: usize| if n == 0 { 0 } else { ((v as isize + d).rem_euclid(n as isize)) as usize };
        let bump = |v: u32, step: u32, max: u32| if d < 0 { v.saturating_sub(step).max(1) } else { (v + step).min(max) };
        match k.code {
            KeyCode::Esc => return None,
            KeyCode::Up | KeyCode::BackTab => w.field = (w.field + WIZ_FIELDS.len() - 1) % WIZ_FIELDS.len(),
            KeyCode::Down | KeyCode::Tab => w.field = (w.field + 1) % WIZ_FIELDS.len(),
            KeyCode::Enter => {
                if let Some(why) = self.wiz_error(&w) {
                    self.not_run(format!("vm create {}", w.spec.name), why);
                    self.modal = Modal::Wizard(w);
                    return None;
                }
                let ops = self.wiz_ops(&w);
                let name = w.spec.name.clone();
                return self.request(ops, Tag::Create(name));
            }
            KeyCode::Left | KeyCode::Right => match w.field {
                0 => {
                    w.tpl = cyc(w.tpl, nt);
                    self.wiz_apply_template(&mut w);
                }
                2 => w.spec.cpu = bump(w.spec.cpu, 1, 64),
                3 => w.spec.mem_gib = bump(w.spec.mem_gib, 1, 512),
                4 => w.spec.disk_gib = bump(w.spec.disk_gib, 5, 4096),
                5 => w.ds = cyc(w.ds, nd),
                6 => w.sw = cyc(w.sw, ns),
                7 => w.iso = cyc(w.iso, ni + 1),
                8 => w.spec.install = !w.spec.install,
                9 => w.spec.autostart = !w.spec.autostart,
                _ => {}
            },
            KeyCode::Char(' ') if w.field == 8 => w.spec.install = !w.spec.install,
            KeyCode::Char(' ') if w.field == 9 => w.spec.autostart = !w.spec.autostart,
            KeyCode::Backspace if w.field == 1 => {
                w.spec.name.pop();
            }
            KeyCode::Char(c) if w.field == 1 => w.spec.name.push(c),
            _ => {}
        }
        self.wiz_sync(&mut w);
        self.modal = Modal::Wizard(w);
        None
    }

    // ───────────────────────── assistant ─────────────────────────

    fn key_assistant(&mut self, k: KeyEvent) -> Option<Effect> {
        match k.code {
            KeyCode::Esc => self.view = View::Machines,
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            KeyCode::Enter => {
                let text = self.input.trim().to_string();
                if !text.is_empty() {
                    self.input.clear();
                    self.ask(&text);
                } else {
                    return self.run_last_plan();
                }
            }
            _ => {}
        }
        None
    }

    /// Plan a reply (never runs anything).
    pub fn ask(&mut self, text: &str) {
        let Some(inv) = self.inv.as_ref() else { return };
        let selected = self.guest().map(|g| g.name.clone());
        let reply = planner::plan(text, inv, selected.as_deref(), &self.log.entries);
        let n = reply.steps.len();
        let id = self.next_id;
        self.next_id += 1;
        self.msgs.push(Msg { id, user: text.into(), reply, states: vec![StepState::Pending; n], stderr: vec![String::new(); n], next: 0, ran: false });
    }

    /// Run the newest unrun plan (Enter on an empty prompt).
    pub fn run_last_plan(&mut self) -> Option<Effect> {
        if !self.privilege().can_write() {
            self.not_run("plan".into(), "virtual machines can only be managed by root".into());
            return None;
        }
        let m = self.msgs.last_mut().filter(|m| m.reply.has_plan() && !m.ran)?;
        m.ran = true;
        let ops = m.reply.steps.iter().map(|s| s.op.clone()).collect();
        Some(Effect::Run { ops, tag: Tag::Plan(m.id) })
    }

    /// After the console child exits and the TUI is back.
    pub fn console_closed(&mut self, argv: &[String], status: std::io::Result<std::process::ExitStatus>) {
        let cmd = argv.join(" ");
        self.toast = Some(match status {
            Ok(s) => Toast { code: s.code().unwrap_or(-1), cmd, stderr: String::new() },
            Err(e) => Toast { code: 127, cmd, stderr: format!("could not start console: {e}") },
        });
    }
}

fn move_in(i: &mut usize, n: usize, code: KeyCode) {
    if n == 0 {
        *i = 0;
        return;
    }
    match code {
        KeyCode::Up | KeyCode::Char('k') => *i = (*i + n - 1) % n,
        KeyCode::Down | KeyCode::Char('j') => *i = (*i + 1) % n,
        KeyCode::Home | KeyCode::Char('g') => *i = 0,
        KeyCode::End | KeyCode::Char('G') => *i = n - 1,
        KeyCode::PageUp => *i = i.saturating_sub(10),
        KeyCode::PageDown => *i = (*i + 10).min(n - 1),
        _ => {}
    }
}


#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use vmctl_core::backend::ExecResult;
    use vmctl_core::demo::DemoHost;
    use vmctl_core::inventory::Hypervisor;

    pub fn demo_app() -> App {
        let mut a = App::new(ActivityLog::memory(), true);
        a.on_event(Event::Inventory(Box::new(DemoHost::new().inventory())));
        a
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn select(a: &mut App, name: &str) {
        a.sel = a.inv.as_ref().unwrap().guests.iter().position(|g| g.name == name).unwrap();
    }

    fn ops_of(e: Option<Effect>) -> Vec<Op> {
        match e {
            Some(Effect::Run { ops, .. }) => ops,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn start_stopped_guest_runs_vm_start() {
        let mut a = demo_app();
        select(&mut a, "win11-dev");
        assert_eq!(ops_of(a.on_key(key(KeyCode::Char('s')))), vec![Op::Start("win11-dev".into())]);
    }

    #[test]
    fn guard_blocks_and_explains_without_running() {
        let mut a = demo_app();
        select(&mut a, "ubuntu-2404"); // already running
        assert!(a.on_key(key(KeyCode::Char('s'))).is_none());
        let t = a.toast.clone().expect("toast");
        assert!(t.cmd.ends_with("(not run)"), "{t:?}");
        assert!(a.action_state("start").is_err());
        assert!(a.action_state("stop").is_ok());
    }

    #[test]
    fn read_only_greys_every_write_action() {
        let mut h = DemoHost::new();
        h.privilege = Privilege::None;
        let mut a = App::new(ActivityLog::memory(), false);
        a.on_event(Event::Inventory(Box::new(h.inventory())));
        for (_, k) in ACTIONS {
            assert!(a.action_state(k).is_err(), "{k} should be greyed in read-only");
        }
        assert!(a.action_state("start").unwrap_err().contains("root"));
    }

    #[test]
    fn destroy_goes_through_confirm() {
        let mut a = demo_app();
        select(&mut a, "win11-dev");
        a.action = ACTIONS.iter().position(|(_, k)| *k == "destroy").unwrap();
        assert!(a.on_key(key(KeyCode::Enter)).is_none());
        assert!(matches!(a.modal, Modal::Confirm(_)));
        assert_eq!(ops_of(a.on_key(key(KeyCode::Char('y')))), vec![Op::Destroy("win11-dev".into())]);
        assert!(matches!(a.modal, Modal::None));
    }

    #[test]
    fn confirm_cancel_runs_nothing() {
        let mut a = demo_app();
        select(&mut a, "win11-dev");
        a.activate("destroy");
        assert!(a.on_key(key(KeyCode::Char('n'))).is_none());
        assert!(matches!(a.modal, Modal::None));
    }

    #[test]
    fn poweroff_snapshots_first_and_space_toggles() {
        let mut a = demo_app();
        select(&mut a, "ubuntu-2404");
        a.activate("poweroff");
        let Modal::Confirm(c) = &a.modal else { panic!("no confirm") };
        assert!(matches!(c.ops[0], Op::Snapshot { force: true, .. }));
        assert_eq!(c.ops[1], Op::Poweroff("ubuntu-2404".into()));
        a.on_key(key(KeyCode::Char(' ')));
        assert_eq!(ops_of(a.on_key(key(KeyCode::Char('y')))), vec![Op::Poweroff("ubuntu-2404".into())]);
    }

    #[test]
    fn console_suspends_only_for_running_guests() {
        let mut a = demo_app();
        select(&mut a, "win11-dev");
        assert!(a.on_key(key(KeyCode::Char('c'))).is_none());
        assert!(a.toast.as_ref().unwrap().stderr.contains("isn't running"));
        select(&mut a, "ubuntu-2404");
        match a.on_key(key(KeyCode::Char('c'))) {
            Some(Effect::Console(argv)) => assert_eq!(argv[0], "sh", "demo console must not touch vm(8)"),
            other => panic!("{other:?}"),
        }
        assert_eq!(console_argv(false, Privilege::Sudo, "x"), vec!["sudo", "vm", "console", "x"]);
        assert_eq!(console_argv(false, Privilege::Root, "x"), vec!["vm", "console", "x"]);
    }

    #[test]
    fn rollback_from_snapshots_view_fans_out_per_dataset() {
        let mut a = demo_app();
        a.on_key(key(KeyCode::Char('2')));
        assert_eq!(a.view, View::Snapshots);
        a.snap_sel = a.snap_rows().iter().position(|(g, _)| g.name == "win11-dev").unwrap();
        a.on_key(key(KeyCode::Char('b')));
        let Modal::Confirm(c) = &a.modal else { panic!("no confirm: {:?}", a.toast) };
        assert!(!c.ops.is_empty());
        assert!(c.ops.iter().all(|o| matches!(o, Op::Rollback { name, snap, .. } if name == "win11-dev" && snap == "activated")));
    }

    #[test]
    fn wizard_creates_and_selects_new_guest() {
        let mut a = demo_app();
        a.on_key(key(KeyCode::Char('n')));
        let Modal::Wizard(w) = &a.modal else { panic!("no wizard") };
        let name = w.spec.name.clone();
        assert_eq!(name, "ubuntu-01");
        let (ops, tag) = match a.on_key(key(KeyCode::Enter)) {
            Some(Effect::Run { ops, tag }) => (ops, tag),
            o => panic!("{o:?} {:?}", a.toast),
        };
        assert!(matches!(&ops[0], Op::Create { name: n, template, .. } if *n == name && template == "ubuntu"));
        let mut h = DemoHost::new();
        for o in &ops {
            let r = h.run(o.program(), &o.argv());
            assert_eq!(r.code, 0, "{}: {}", r.cmd, r.stderr);
        }
        a.on_event(Event::RunDone { tag, failed_at: None });
        a.on_event(Event::Inventory(Box::new(h.inventory())));
        assert_eq!(a.guest().unwrap().name, name);
    }

    #[test]
    fn wizard_rejects_bad_and_taken_names() {
        let mut a = demo_app();
        a.open_wizard();
        let Modal::Wizard(mut w) = std::mem::replace(&mut a.modal, Modal::None) else { panic!() };
        w.spec.name = "ubuntu-2404".into();
        assert!(a.wiz_error(&w).unwrap().contains("already exists"));
        w.spec.name = "bad name".into();
        assert!(a.wiz_error(&w).is_some());
        a.modal = Modal::Wizard(w);
        assert!(a.on_key(key(KeyCode::Enter)).is_none());
        assert!(matches!(a.modal, Modal::Wizard(_)), "wizard stays open on error");
    }

    #[test]
    fn assistant_plans_then_runs_with_live_step_states() {
        let mut a = demo_app();
        a.on_key(key(KeyCode::Char('6')));
        for c in "snapshot everything that's running".chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
        assert!(a.on_key(key(KeyCode::Enter)).is_none(), "asking never runs");
        let n = a.msgs[0].reply.steps.len();
        assert!(n >= 3, "{n}");
        let (ops, tag) = match a.on_key(key(KeyCode::Enter)) {
            Some(Effect::Run { ops, tag }) => (ops, tag),
            o => panic!("{o:?}"),
        };
        assert_eq!(ops.len(), n);
        assert!(a.on_key(key(KeyCode::Enter)).is_none(), "a plan runs once");
        a.on_event(Event::Step { tag: tag.clone(), result: ExecResult::ok(ops[0].cmd()) });
        a.on_event(Event::Step { tag: tag.clone(), result: ExecResult::fail(ops[1].cmd(), 1, "boom") });
        a.on_event(Event::RunDone { tag, failed_at: Some(1) });
        let m = &a.msgs[0];
        assert_eq!(m.states[0], StepState::Ok);
        assert_eq!(m.states[1], StepState::Failed);
        assert_eq!(m.stderr[1], "boom");
        assert!(m.states[2..].iter().all(|s| *s == StepState::Skipped));
        assert_eq!(a.log.entries.len(), 2, "every real step is logged");
        assert_eq!(a.log.entries[0].source, SOURCE);
    }

    #[test]
    fn quit_keys_and_view_cycle() {
        let mut a = demo_app();
        assert!(matches!(a.on_key(key(KeyCode::Char('q'))), Some(Effect::Quit)));
        assert!(matches!(a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), Some(Effect::Quit)));
        for _ in 0..View::ALL.len() {
            a.on_key(key(KeyCode::Tab));
        }
        assert_eq!(a.view, View::Machines);
        a.view = View::Assistant;
        assert!(a.on_key(key(KeyCode::Char('q'))).is_none(), "q types in the assistant");
        assert_eq!(a.input, "q");
    }

    #[test]
    fn selection_clamps_when_guests_disappear() {
        let mut a = demo_app();
        a.sel = 5;
        let mut h = DemoHost::new();
        h.guests.truncate(2);
        a.on_event(Event::Inventory(Box::new(h.inventory())));
        assert_eq!(a.sel, 1);
    }
}
