//! The ratatui application: a VirtualBox-style VM list with an action bar.
//!
//! Layout (top → bottom):
//!   ┌ header: title + host hint ─────────────────────────────┐
//!   │ VM list (selectable rows)                              │
//!   │                                                        │
//!   ├ action bar: Start/Stop/Restart/Suspend/Console/Info ───┤
//!   │ status line: last outcome / error                      │
//!   └────────────────────────────────────────────────────────┘
//!
//! Keys: ↑/↓ or j/k move · Enter runs the highlighted action · 1-7 pick an
//! action · r refresh · q quit.

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};

use vmctl_core::actions::{self, Action};
use crate::panes::{self, Assistant, LogTail, Pane, Wizard};
use crate::panes2;
use vmctl_core::vmbhyve::{Vm, VmSummary};

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub struct App {
    vm: Vm,
    rows: Vec<VmSummary>,
    state: ListState,
    action_idx: usize,
    status: String,
    should_quit: bool,
    pane: Pane,
    wizard: Wizard,
    log: LogTail,
    assistant: Assistant,
    snapshots: panes2::ListPane,
    switches: panes2::ListPane,
    datastores: panes2::ListPane,
    images: panes2::ListPane,
}

impl App {
    pub fn new(vm: Vm) -> Self {
        let mut app = App {
            vm,
            rows: Vec::new(),
            state: ListState::default(),
            action_idx: 0,
            status: "1-7 pane · ↑↓ select · Enter run · q quit".into(),
            should_quit: false,
            pane: Pane::Guests,
            wizard: Wizard::default(),
            log: LogTail::default(),
            assistant: Assistant::default(),
            snapshots: panes2::ListPane::new(
                "Snapshots",
                "SNAPSHOT                          GUEST        AGE",
                vec!["Rollback", "Clone", "Delete"],
                "no snapshots — press Enter on a guest in pane 1, then snapshot it",
            ),
            switches: panes2::ListPane::new(
                "Virtual switches",
                "NAME        TYPE      VLAN   ADDRESS        INTERFACES",
                vec!["Info", "Create", "Destroy"],
                "no switches — create one with `vm switch create <name>`",
            ),
            datastores: panes2::ListPane::new(
                "Datastores",
                "NAME        SPEC",
                vec!["Info", "Add ISO", "Remove"],
                "no datastores",
            ),
            images: panes2::ListPane::new(
                "VM images",
                "UUID                                  NAME        DESCRIPTION",
                vec!["Provision", "Create", "Destroy"],
                "no images — create one with `vm image create <name>`",
            ),
        };
        app.refresh();
        if !app.rows.is_empty() {
            app.state.select(Some(0));
        }
        app.reload_isos();
        app
    }

    /// Refresh the wizard's ISO picker from `~/installers` + each datastore's
    /// `.iso` dir. Called on startup and after the guest list changes, so the
    /// picker always reflects what's actually on disk.
    fn reload_isos(&mut self) {
        use std::collections::BTreeSet;
        use std::path::PathBuf;

        // Gather the distinct datastores we know about, plus a sane default.
        let mut stores: BTreeSet<PathBuf> = BTreeSet::new();
        for r in &self.rows {
            if !r.datastore.is_empty() {
                stores.insert(PathBuf::from(&r.datastore));
            }
        }
        if stores.is_empty() {
            stores.insert(PathBuf::from("/zroot/vm"));
        }

        // installers first, then each datastore's .iso dir.
        let mut isos = vmctl_core::iso::scan_dir(&vmctl_core::iso::installers_dir(), "installers");
        for ds in &stores {
            isos.extend(vmctl_core::iso::scan_dir(
                &vmctl_core::iso::datastore_iso_dir(ds),
                "datastore",
            ));
        }
        isos.sort_by(|a, b| {
            let rank = |s: &str| if s == "installers" { 0 } else { 1 };
            rank(a.source)
                .cmp(&rank(b.source))
                .then_with(|| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()))
        });
        isos.dedup_by(|a, b| a.name == b.name);
        self.wizard.load_isos(isos);
    }

    pub fn refresh(&mut self) {
        match self.vm.list() {
            Ok(rows) => {
                self.status = format!("{} guest(s)", rows.len());
                self.rows = rows;
                if self.rows.is_empty() {
                    self.state.select(None);
                } else if self.state.selected().is_none() {
                    self.state.select(Some(0));
                }
            }
            Err(e) => self.status = format!("error: {e}"),
        }
    }

    fn selected(&self) -> Option<&VmSummary> {
        self.state.selected().and_then(|i| self.rows.get(i))
    }

    fn move_sel(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        let len = self.rows.len() as isize;
        let next = (cur + delta).rem_euclid(len) as usize;
        self.state.select(Some(next));
    }

    fn run_selected_action(&mut self) {
        let action = Action::ALL[self.action_idx];
        let Some(row) = self.selected() else {
            self.status = "no guest selected".into();
            return;
        };
        let name = row.name.clone();
        let cmd = format!("vm {}", action.argv(&name).join(" "));
        match actions::run_action(&self.vm, action, &name) {
            Ok(msg) => {
                self.log.push(cmd, 0, None);
                self.status = msg;
            }
            Err(e) => {
                let m = format!("{e}");
                self.log.push(cmd, 1, Some(m.clone()));
                self.status = format!("error: {m}");
            }
        }
        // State likely changed (start/stop/etc) — refresh the list.
        if !action.is_interactive() {
            self.refresh();
        }
    }

    fn on_key(&mut self, code: KeyCode) {
        // Global keys.
        match code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Esc => {
                // Esc backs out to the Guests pane, or quits from it.
                if self.pane == Pane::Guests {
                    self.should_quit = true;
                } else {
                    self.pane = Pane::Guests;
                }
                return;
            }
            KeyCode::Char(c @ '1'..='9') | KeyCode::Char(c @ '0') | KeyCode::Char(c @ '-') => {
                if let Some(p) = Pane::from_digit(c) {
                    self.pane = p;
                }
                return;
            }
            _ => {}
        }

        // Pane-local keys.
        match self.pane {
            Pane::Guests => match code {
                KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
                KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
                KeyCode::Char('r') => self.refresh(),
                KeyCode::Enter => self.run_selected_action(),
                KeyCode::Char(c @ '6') => {
                    self.action_idx = (c as u8 - b'1') as usize;
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    self.action_idx = (self.action_idx + Action::ALL.len() - 1) % Action::ALL.len()
                }
                KeyCode::Right | KeyCode::Char('l') => {
                    self.action_idx = (self.action_idx + 1) % Action::ALL.len()
                }
                _ => {}
            },
            Pane::Assistant => match code {
                KeyCode::Backspace => self.assistant.backspace(),
                KeyCode::Char(c) => self.assistant.type_char(c),
                KeyCode::Enter => {
                    if self.assistant.last_plan.is_some() && self.assistant.input.trim().is_empty() {
                        self.run_assistant_plan();
                    } else {
                        let known: Vec<String> = self.rows.iter().map(|r| r.name.clone()).collect();
                        self.assistant.submit(&known);
                    }
                }
                _ => {}
            },
            Pane::Wizard => {
                // The ISO picker is modal: while it's open it owns the keys.
                if self.wizard.picker_open {
                    match code {
                        KeyCode::Down | KeyCode::Char('j') => self.wizard.picker_move(1),
                        KeyCode::Up | KeyCode::Char('k') => self.wizard.picker_move(-1),
                        KeyCode::Enter => self.wizard.picker_select(),
                        KeyCode::Esc => self.wizard.close_picker(),
                        _ => {}
                    }
                    return;
                }
                match code {
                    KeyCode::Down | KeyCode::Tab => self.wizard.next_field(),
                    KeyCode::Up | KeyCode::BackTab => self.wizard.prev_field(),
                    KeyCode::Left => self.wizard.cycle_template(-1),
                    KeyCode::Right => self.wizard.cycle_template(1),
                    KeyCode::Backspace => self.wizard.backspace(),
                    KeyCode::Char(c) => self.wizard.type_char(c),
                    // Enter on the ISO field opens the picker; elsewhere it runs.
                    KeyCode::Enter => {
                        if self.wizard.on_iso_field() {
                            self.wizard.open_picker();
                        } else {
                            self.run_wizard();
                        }
                    }
                    _ => {}
                }
            }
            Pane::Deploy => match code {
                KeyCode::Enter => self.run_deploy(),
                _ => {}
            },
            Pane::Snapshots | Pane::Switches | Pane::Datastores | Pane::Images => {
                let pane = match self.pane {
                    Pane::Snapshots => &mut self.snapshots,
                    Pane::Switches => &mut self.switches,
                    Pane::Datastores => &mut self.datastores,
                    Pane::Images => &mut self.images,
                    _ => unreachable!(),
                };
                match code {
                    KeyCode::Down | KeyCode::Char('j') => pane.move_sel(1),
                    KeyCode::Up | KeyCode::Char('k') => pane.move_sel(-1),
                    KeyCode::Right | KeyCode::Tab => pane.move_action(1),
                    KeyCode::Left | KeyCode::BackTab => pane.move_action(-1),
                    KeyCode::Char('r') => self.refresh_feature_panes(),
                    KeyCode::Enter => self.run_feature_action(),
                    _ => {}
                }
            }
            Pane::Config | Pane::Console | Pane::Log => {
                // Read-only panes: navigation handled by globals.
            }
        }
    }

    /// Refresh the four feature panes (Snapshots/Switches/Datastores/Images)
    /// from the live hypervisor via `vm(8)`.
    fn refresh_feature_panes(&mut self) {
        // Switches: `vm switch list`.
        match self.vm.run(&["switch", "list"]) {
            Ok(out) => {
                let rows: Vec<String> = out
                    .lines()
                    .skip(1) // header
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| l.to_string())
                    .collect();
                self.switches.set_rows(rows);
            }
            Err(e) => self.switches.status = format!("error: {e}"),
        }
        // Datastores: `vm datastore list`.
        match self.vm.run(&["datastore", "list"]) {
            Ok(out) => {
                let rows: Vec<String> = out
                    .lines()
                    .skip(1)
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| l.to_string())
                    .collect();
                self.datastores.set_rows(rows);
            }
            Err(e) => self.datastores.status = format!("error: {e}"),
        }
        // Images: `vm image list`.
        match self.vm.run(&["image", "list"]) {
            Ok(out) => {
                let rows: Vec<String> = out
                    .lines()
                    .skip(1)
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| l.to_string())
                    .collect();
                self.images.set_rows(rows);
            }
            Err(e) => self.images.status = format!("error: {e}"),
        }
        // Snapshots: derived from `vm list -v` snapshot sections.
        match self.vm.run(&["list", "-v"]) {
            Ok(out) => {
                let rows: Vec<String> = out
                    .lines()
                    .filter(|l| l.contains('@'))
                    .map(|l| l.trim().to_string())
                    .collect();
                self.snapshots.set_rows(rows);
            }
            Err(e) => self.snapshots.status = format!("error: {e}"),
        }
    }

    /// Run the highlighted action on the active feature pane.
    fn run_feature_action(&mut self) {
        let (argv, label): (Vec<String>, String) = match self.pane {
            Pane::Snapshots => {
                let Some(row) = self.snapshots.selected() else {
                    self.snapshots.status = "no snapshot selected".into();
                    return;
                };
                let snap = row.split_whitespace().next().unwrap_or("").to_string();
                match self.snapshots.action_idx {
                    0 => (vec!["rollback".into(), "-r".into(), snap.clone()], format!("vm rollback -r {snap}")),
                    1 => (vec!["clone".into(), snap.clone(), format!("{snap}-copy")], format!("vm clone {snap} {snap}-copy")),
                    _ => (vec!["snapshot".into(), "-f".into(), snap.clone()], format!("vm snapshot -f {snap}")),
                }
            }
            Pane::Switches => {
                let Some(row) = self.switches.selected() else {
                    self.switches.status = "no switch selected".into();
                    return;
                };
                let name = row.split_whitespace().next().unwrap_or("").to_string();
                match self.switches.action_idx {
                    0 => (vec!["switch".into(), "info".into(), name.clone()], format!("vm switch info {name}")),
                    1 => (vec!["switch".into(), "create".into(), name.clone()], format!("vm switch create {name}")),
                    _ => (vec!["switch".into(), "destroy".into(), name.clone()], format!("vm switch destroy {name}")),
                }
            }
            Pane::Datastores => {
                let Some(row) = self.datastores.selected() else {
                    self.datastores.status = "no datastore selected".into();
                    return;
                };
                let name = row.split_whitespace().next().unwrap_or("").to_string();
                match self.datastores.action_idx {
                    0 => (vec!["datastore".into(), "list".into()], "vm datastore list".into()),
                    _ => (vec!["datastore".into(), "remove".into(), name.clone()], format!("vm datastore remove {name}")),
                }
            }
            Pane::Images => {
                let Some(row) = self.images.selected() else {
                    self.images.status = "no image selected".into();
                    return;
                };
                let uuid = row.split_whitespace().next().unwrap_or("").to_string();
                match self.images.action_idx {
                    0 => (vec!["image".into(), "provision".into(), uuid.clone(), "new-vm".into()], format!("vm image provision {uuid} new-vm")),
                    _ => (vec!["image".into(), "destroy".into(), uuid.clone()], format!("vm image destroy {uuid}")),
                }
            }
            _ => return,
        };
        let argv_refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        match self.vm.run(&argv_refs) {
            Ok(out) => {
                self.log.push(label, 0, None);
                let msg = if out.trim().is_empty() { "ok".to_string() } else { out.trim().to_string() };
                self.set_feature_status(msg);
            }
            Err(e) => {
                let m = format!("{e}");
                self.log.push(label, 1, Some(m.clone()));
                self.set_feature_status(format!("error: {m}"));
            }
        }
        self.refresh_feature_panes();
    }

    fn set_feature_status(&mut self, msg: String) {
        match self.pane {
            Pane::Snapshots => self.snapshots.status = msg,
            Pane::Switches => self.switches.status = msg,
            Pane::Datastores => self.datastores.status = msg,
            Pane::Images => self.images.status = msg,
            _ => {}
        }
    }

    /// Run the staged assistant plan step by step, stopping at the first
    /// failure and keeping the real stderr — mirroring `assistant::run_plan`.
    fn run_assistant_plan(&mut self) {
        let Some(plan) = self.assistant.last_plan.clone() else {
            return;
        };
        if plan.steps.is_empty() {
            self.status = "nothing to run".into();
            return;
        }
        self.assistant.transcript.push(format!("running {} step(s)…", plan.steps.len()));
        for step in &plan.steps {
            let cmd = step.cmd();
            let refs: Vec<&str> = step.argv.iter().map(|s| s.as_str()).collect();
            match self.vm.run(&refs) {
                Ok(msg) => {
                    self.log.push(cmd.clone(), 0, None);
                    self.assistant.transcript.push(format!("  ✓ {cmd}"));
                    self.status = msg;
                }
                Err(e) => {
                    let m = format!("{e}");
                    self.log.push(cmd.clone(), 1, Some(m.clone()));
                    self.assistant.transcript.push(format!("  ✗ {cmd}  ↳ {m}"));
                    self.assistant.transcript.push("  stopped at first failure; nothing after ran.".into());
                    self.status = format!("plan stopped: {m}");
                    self.refresh();
                    return;
                }
            }
        }
        self.assistant.transcript.push("  done — all steps succeeded.".into());
        self.assistant.last_plan = None;
        self.refresh();
    }

    /// Run the wizard's `vm create` + `vm install` against the live backend.
    fn run_wizard(&mut self) {
        if !self.wizard.ready() {
            self.status = "wizard: fill in name, cpus, memory, disk".into();
            return;
        }
        let create: Vec<String> = self.wizard.create_argv();
        let create_refs: Vec<&str> = create.iter().map(|s| s.as_str()).collect();
        let cmd = format!("vm {}", create.join(" "));
        match self.vm.run(&create_refs) {
            Ok(msg) => {
                self.log.push(cmd, 0, None);
                self.status = msg;
                // Then boot the installer.
                let install: Vec<String> = self.wizard.install_argv();
                let install_refs: Vec<&str> = install.iter().map(|s| s.as_str()).collect();
                let icmd = format!("vm {}", install.join(" "));
                match self.vm.run(&install_refs) {
                    Ok(m2) => {
                        self.log.push(icmd, 0, None);
                        self.status = format!("{} · {}", self.status, m2);
                    }
                    Err(e) => {
                        let m = format!("{e}");
                        self.log.push(icmd, 1, Some(m.clone()));
                        self.status = format!("created, but install failed: {m}");
                    }
                }
                self.refresh();
                self.pane = Pane::Guests;
            }
            Err(e) => {
                let m = format!("{e}");
                self.log.push(cmd, 1, Some(m.clone()));
                self.status = format!("error: {m}");
            }
        }
    }

    /// Deploy pane: one-button launch. Boots the selected guest (or, if it's
    /// already running, hands off to the console command). Mirrors the exact
    /// `vm(8)` commands shown in the pane so there are no surprises.
    fn run_deploy(&mut self) {
        let Some(row) = self.selected() else {
            self.status = "deploy: no guest selected".into();
            return;
        };
        let name = row.name.clone();
        if row.is_running() {
            self.status = format!("'{name}' is running — attach with: vm console {name}");
            self.log.push(format!("vm console {name}"), 0, None);
            return;
        }
        let cmd = format!("vm start {name}");
        match self.vm.run(&["start", &name]) {
            Ok(msg) => {
                self.log.push(cmd, 0, None);
                self.status = format!("{msg} — attach with: vm console {name}");
            }
            Err(e) => {
                let m = format!("{e}");
                self.log.push(cmd, 1, Some(m.clone()));
                self.status = format!("error: {m}");
            }
        }
        self.refresh();
    }

    fn draw(&mut self, f: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // tab bar
                Constraint::Min(3),    // active pane
                Constraint::Length(3), // action bar / hint
                Constraint::Length(3), // status
            ])
            .split(f.area());

        // Tab bar.
        panes::render_tabs(f, chunks[0], self.pane);

        // Active pane.
        match self.pane {
            Pane::Guests => self.draw_guests(f, chunks[1]),
            Pane::Assistant => self.assistant.render(f, chunks[1]),
            Pane::Wizard => self.wizard.render(f, chunks[1]),
            Pane::Config => {
                let (cpu, mem) = self
                    .selected()
                    .map(|r| (r.cpu.clone(), r.memory.clone()))
                    .unwrap_or_else(|| ("-".into(), "-".into()));
                let name = self
                    .selected()
                    .map(|r| r.name.clone())
                    .unwrap_or_else(|| "(none)".into());
                panes::render_config(f, chunks[1], &name, &cpu, &mem);
            }
            Pane::Console => {
                let (name, running) = self
                    .selected()
                    .map(|r| (r.name.clone(), r.state.starts_with("Running")))
                    .unwrap_or_else(|| ("(none)".into(), false));
                panes::render_console(f, chunks[1], &name, running);
            }
            Pane::Log => self.log.render(f, chunks[1]),
            Pane::Deploy => {
                let (name, has_guest, running) = self
                    .selected()
                    .map(|r| (r.name.clone(), true, r.is_running()))
                    .unwrap_or_else(|| ("(none)".into(), false, false));
                let iso = self.wizard.iso();
                panes::render_deploy(f, chunks[1], &name, has_guest, running, &iso);
            }
            Pane::Snapshots => self.snapshots.render(f, chunks[1]),
            Pane::Switches => self.switches.render(f, chunks[1]),
            Pane::Datastores => self.datastores.render(f, chunks[1]),
            Pane::Images => self.images.render(f, chunks[1]),
        }

        // Action bar (context-sensitive).
        use crate::theme as t;
        match self.pane {
            Pane::Guests => {
                let spans: Vec<Span> = Action::ALL
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        let label = format!(" {}.{} ", i + 1, a.label());
                        if i == self.action_idx {
                            Span::styled(label, t::pill(t::BG, t::CRIMSON))
                        } else {
                            Span::styled(label, t::sub())
                        }
                    })
                    .collect();
                let bar = Paragraph::new(Line::from(spans)).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(t::border())
                        .title(Span::styled(" Actions (\u{2190}\u{2192} pick \u{00b7} Enter run) ", t::ember()))
                        .style(t::bg()),
                );
                f.render_widget(bar, chunks[2]);
            }
            Pane::Assistant => {
                let hint = Paragraph::new("Type a request \u{00b7} Enter to plan \u{00b7} Enter again to run \u{00b7} Esc back")
                    .block(Block::default().borders(Borders::ALL).title("Ask vmctl"));
                f.render_widget(hint, chunks[2]);
            }
            Pane::Wizard => {
                let hint = Paragraph::new("Tab/\u{2191}\u{2193} field \u{00b7} \u{2190}\u{2192} template \u{00b7} type to edit \u{00b7} Enter create \u{00b7} Esc back")
                    .block(Block::default().borders(Borders::ALL).title("Wizard"));
                f.render_widget(hint, chunks[2]);
            }
            Pane::Deploy => {
                let hint = Paragraph::new("Enter launch \u{00b7} \u{2191}\u{2193} pick guest in pane 1 \u{00b7} Esc back")
                    .block(Block::default().borders(Borders::ALL).title("Deploy"));
                f.render_widget(hint, chunks[2]);
            }
            _ => {
                let hint = Paragraph::new("1-7 switch pane \u{00b7} Esc back to guests \u{00b7} q quit")
                    .block(Block::default().borders(Borders::ALL).title("Navigate"));
                f.render_widget(hint, chunks[2]);
            }
        }

        // Status line.
        let status = Paragraph::new(Span::styled(self.status.clone(), t::text())).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(t::border())
                .title(Span::styled(" Status ", t::muted()))
                .style(t::bg()),
        );
        f.render_widget(status, chunks[3]);
    }

    fn draw_guests(&mut self, f: &mut Frame, area: Rect) {
        use crate::theme as t;
        use ratatui::style::Color;

        // Column header row, matching the prototype's Machines table.
        let header = ListItem::new(Line::from(vec![
            Span::styled(format!("{:<16}", "NAME"), t::muted()),
            Span::styled(format!("{:<10}", "STATE"), t::muted()),
            Span::styled(format!("{:<5}", "CPU"), t::muted()),
            Span::styled(format!("{:<8}", "MEM"), t::muted()),
            Span::styled(format!("{:<12}", "DATASTORE"), t::muted()),
            Span::styled("LOADER", t::muted()),
        ]));

        let items: Vec<ListItem> = self
            .rows
            .iter()
            .map(|r| {
                // State pill: glyph + tinted background, per the prototype.
                let (glyph, fg, bg) = state_pill(&r.state);
                let pill = Span::styled(format!(" {} {} ", glyph, short_state(&r.state)), t::pill(fg, bg));
                let line = Line::from(vec![
                    Span::styled(format!("{:<16}", truncate(&r.name, 15)), t::text()),
                    pill,
                    Span::styled(format!("{:<5}", r.cpu), t::sub()),
                    Span::styled(format!("{:<8}", r.memory), t::sub()),
                    Span::styled(format!("{:<12}", truncate(&r.datastore, 11)), t::muted()),
                    Span::styled(r.loader.clone(), t::muted()),
                ]);
                ListItem::new(line)
            })
            .collect();

        let mut all = vec![header];
        all.extend(items);

        let list = List::new(all)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(t::border())
                    .title(Span::styled(" Machines ", t::ember()))
                    .style(t::bg()),
            )
            .highlight_style(t::selection())
            .highlight_symbol("\u{25b6} ");
        f.render_stateful_widget(list, area, &mut self.state);
    }
}

/// Map a vm(8) state string to the prototype's pill glyph + colors.
fn state_pill(state: &str) -> (&'static str, ratatui::style::Color, ratatui::style::Color) {
    use crate::theme as t;
    use ratatui::style::Color;
    let s = state.to_lowercase();
    if s.contains("running") {
        ("\u{25cf}", t::GREEN, Color::Rgb(0x10, 0x2A, 0x22)) // ● green
    } else if s.contains("suspended") || s.contains("paused") {
        ("\u{25d0}", t::VIOLET, Color::Rgb(0x22, 0x1E, 0x33)) // ◐ violet
    } else if s.contains("locked") {
        ("\u{25c6}", t::AMBER, Color::Rgb(0x2E, 0x27, 0x12)) // ◆ amber
    } else {
        ("\u{25cb}", t::MUTED, Color::Rgb(0x1A, 0x1E, 0x23)) // ○ dim
    }
}

/// Short state label for the pill.
fn short_state(state: &str) -> &'static str {
    let s = state.to_lowercase();
    if s.contains("running") {
        "running"
    } else if s.contains("suspended") || s.contains("paused") {
        "susp"
    } else if s.contains("locked") {
        "locked"
    } else {
        "stopped"
    }
}

/// Truncate to `max` chars, appending `…` when clipped.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}\u{2026}")
    }
}

pub fn install_terminal() -> io::Result<Term> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

pub fn restore_terminal(term: &mut Term) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(term.backend_mut(), LeaveAlternateScreen)?;
    term.show_cursor()?;
    Ok(())
}

/// Run the event loop until the user quits, then restore the terminal.
pub fn run(mut term: Term, mut app: App) -> io::Result<()> {
    let result = event_loop(&mut term, &mut app);
    // Always restore the terminal, even if the loop errored — otherwise the
    // user's shell is left in raw mode with the alternate screen active.
    restore_terminal(&mut term)?;
    result
}

fn event_loop(term: &mut Term, app: &mut App) -> io::Result<()> {
    loop {
        term.draw(|f| app.draw(f))?;
        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.on_key(key.code);
                }
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}
