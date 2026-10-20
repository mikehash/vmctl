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
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};

use crate::actions::{self, Action};
use crate::vmbhyve::{Vm, VmSummary};

pub type Term = Terminal<CrosstermBackend<Stdout>>;

pub struct App {
    vm: Vm,
    rows: Vec<VmSummary>,
    state: ListState,
    action_idx: usize,
    status: String,
    should_quit: bool,
}

impl App {
    pub fn new(vm: Vm) -> Self {
        let mut app = App {
            vm,
            rows: Vec::new(),
            state: ListState::default(),
            action_idx: 0,
            status: "r refresh · ↑↓ select · 1-6 action · Enter run · q quit".into(),
            should_quit: false,
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
        match actions::run_action(&self.vm, action, &name) {
            Ok(msg) => self.status = msg,
            Err(e) => self.status = format!("error: {e}"),
        }
        // State likely changed (start/stop/etc) — refresh the list.
        if !action.is_interactive() {
            self.refresh();
        }
    }

    fn on_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Enter => self.run_selected_action(),
            KeyCode::Char(c @ '1'..='6') => {
                self.action_idx = (c as u8 - b'1') as usize;
            }
            _ => {}
        }
    }

    fn draw(&mut self, f: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(3),
                Constraint::Length(3),
            ])
            .split(f.area());

        // Header.
        let header = Paragraph::new(Line::from(vec![
            Span::styled(" vmctl-tui ", Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw("  vm-bhyve control — VirtualBox-style"),
        ]))
        .block(Block::default().borders(Borders::ALL).title("vmctl"));
        f.render_widget(header, chunks[0]);

        // VM list.
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
            .highlight_symbol("▶ ");
        f.render_stateful_widget(list, chunks[1], &mut self.state);

        // Action bar.
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
            .block(Block::default().borders(Borders::ALL).title("Actions (Enter to run)"));
        f.render_widget(bar, chunks[2]);

        // Status line.
        let status = Paragraph::new(self.status.clone())
            .block(Block::default().borders(Borders::ALL).title("Status"));
        f.render_widget(status, chunks[3]);
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
