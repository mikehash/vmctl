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
//! Keys: ↑/↓ or j/k move · Enter runs the highlighted action · 1-6 pick an
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
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};

use vmctl_core::actions::{self, Action};
use crate::panes::{self, Assistant, LogTail, Pane, Wizard};
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
}

impl App {
    pub fn new(vm: Vm) -> Self {
        let mut app = App {
            vm,
            rows: Vec::new(),
            state: ListState::default(),
            action_idx: 0,
            status: "1-5 pane · ↑↓ select · Enter run · q quit".into(),
            should_quit: false,
            pane: Pane::Guests,
            wizard: Wizard::default(),
            log: LogTail::default(),
            assistant: Assistant::default(),
        };
        app.refresh();
        if !app.rows.is_empty() {
            app.state.select(Some(0));
        }
        app
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
            KeyCode::Char(c @ '1'..='6') => {
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
            Pane::Wizard => match code {
                KeyCode::Down | KeyCode::Tab => self.wizard.next_field(),
                KeyCode::Up | KeyCode::BackTab => self.wizard.prev_field(),
                KeyCode::Left => self.wizard.cycle_template(-1),
                KeyCode::Right => self.wizard.cycle_template(1),
                KeyCode::Backspace => self.wizard.backspace(),
                KeyCode::Char(c) => self.wizard.type_char(c),
                KeyCode::Enter => self.run_wizard(),
                _ => {}
            },
            Pane::Config | Pane::Console | Pane::Log => {
                // Read-only panes: navigation handled by globals.
            }
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
        }

        // Action bar (context-sensitive).
        match self.pane {
            Pane::Guests => {
                let spans: Vec<Span> = Action::ALL
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        let label = format!(" {}.{} ", i + 1, a.label());
                        if i == self.action_idx {
                            Span::styled(label, Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD))
                        } else {
                            Span::raw(label)
                        }
                    })
                    .collect();
                let bar = Paragraph::new(Line::from(spans))
                    .block(Block::default().borders(Borders::ALL).title("Actions (\u{2190}\u{2192} pick \u{00b7} Enter run)"));
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
            _ => {
                let hint = Paragraph::new("1-5 switch pane \u{00b7} Esc back to guests \u{00b7} q quit")
                    .block(Block::default().borders(Borders::ALL).title("Navigate"));
                f.render_widget(hint, chunks[2]);
            }
        }

        // Status line.
        let status = Paragraph::new(self.status.clone())
            .block(Block::default().borders(Borders::ALL).title("Status"));
        f.render_widget(status, chunks[3]);
    }

    fn draw_guests(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .rows
            .iter()
            .map(|r| {
                let line = format!(
                    "{:<14} {:<12} {:<4} {:<8} {:<6} {}",
                    r.name, r.loader, r.cpu, r.memory, r.autostart, r.state
                );
                ListItem::new(line)
            })
            .collect();
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title("Guests"))
            .highlight_style(Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD))
            .highlight_symbol("\u{25b6} ");
        f.render_stateful_widget(list, area, &mut self.state);
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
