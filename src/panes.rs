//! M3 — the terminal panes that mirror the desktop feature-for-feature:
//! a creation wizard, a config editor, a console view, and a log tail.
//!
//! These are pure render + state helpers. They draw into a `ratatui::Frame`
//! and own only the small amount of form state a pane needs. The `App` in
//! `ui.rs` drives which pane is active and feeds them the live guest list.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs};

/// Which pane is currently on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Guests,
    Wizard,
    Config,
    Console,
    Log,
}

impl Pane {
    pub const ALL: [Pane; 5] = [
        Pane::Guests,
        Pane::Wizard,
        Pane::Config,
        Pane::Console,
        Pane::Log,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            Pane::Guests => "Guests",
            Pane::Wizard => "New machine",
            Pane::Config => "Config",
            Pane::Console => "Console",
            Pane::Log => "Activity",
        }
    }

    /// 1-5 hotkey to jump straight to a pane.
    pub fn from_digit(d: char) -> Option<Pane> {
        match d {
            '1' => Some(Pane::Guests),
            '2' => Some(Pane::Wizard),
            '3' => Some(Pane::Config),
            '4' => Some(Pane::Console),
            '5' => Some(Pane::Log),
            _ => None,
        }
    }
}

/// Render the top tab bar showing all panes, highlighting the active one.
pub fn render_tabs(f: &mut ratatui::Frame, area: Rect, active: Pane) {
    let titles: Vec<Line> = Pane::ALL
        .iter()
        .enumerate()
        .map(|(i, p)| Line::from(format!("{}.{}", i + 1, p.title())))
        .collect();
    let idx = Pane::ALL.iter().position(|p| *p == active).unwrap_or(0);
    let tabs = Tabs::new(titles)
        .block(Block::default().borders(Borders::ALL).title("vmctl"))
        .select(idx)
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::raw(" │ "));
    f.render_widget(tabs, area);
}

/// A single wizard form field.
#[derive(Debug, Clone)]
pub struct Field {
    pub label: &'static str,
    pub value: String,
    pub hint: &'static str,
}

/// The creation wizard: template → cpu/memory/disk/ISO → `vm create` + `vm install`.
/// This mirrors the desktop wizard's steps in a linear, keyboard-driven form.
pub struct Wizard {
    pub fields: Vec<Field>,
    pub cursor: usize,
    pub template_idx: usize,
    pub templates: Vec<(&'static str, &'static str)>, // (id, iso)
}

impl Default for Wizard {
    fn default() -> Self {
        Self {
            fields: vec![
                Field { label: "Name", value: String::new(), hint: "guest name (e.g. ubuntu-01)" },
                Field { label: "CPUs", value: "2".into(), hint: "vCPU count" },
                Field { label: "Memory", value: "4G".into(), hint: "e.g. 4G, 512M" },
                Field { label: "Disk", value: "20G".into(), hint: "e.g. 20G" },
            ],
            cursor: 0,
            template_idx: 0,
            templates: vec![
                ("ubuntu", "ubuntu-24.04-live-server.iso"),
                ("freebsd", "FreeBSD-14.3-RELEASE-amd64-disc1.iso"),
                ("debian", "debian-13-netinst.iso"),
                ("windows", "Win11_24H2.iso"),
                ("openbsd", "install77.iso"),
                ("alpine", "alpine-virt.iso"),
            ],
        }
    }
}

impl Wizard {
    pub fn template_id(&self) -> &'static str {
        self.templates[self.template_idx].0
    }

    pub fn iso(&self) -> &'static str {
        self.templates[self.template_idx].1
    }

    /// The `vm create` argv this wizard would run.
    pub fn create_argv(&self) -> Vec<String> {
        let name = self.fields[0].value.trim();
        let cpu = self.fields[1].value.trim();
        let mem = self.fields[2].value.trim();
        let disk = self.fields[3].value.trim();
        vec![
            "create".into(),
            "-t".into(),
            self.template_id().into(),
            "-c".into(),
            cpu.into(),
            "-m".into(),
            mem.into(),
            "-s".into(),
            disk.into(),
            name.into(),
        ]
    }

    /// The `vm install` argv (boot the installer from the template's ISO).
    pub fn install_argv(&self) -> Vec<String> {
        let name = self.fields[0].value.trim();
        vec!["install".into(), name.into(), self.iso().into()]
    }

    /// True when the required fields are filled in enough to run.
    pub fn ready(&self) -> bool {
        !self.fields[0].value.trim().is_empty()
            && !self.fields[1].value.trim().is_empty()
            && !self.fields[2].value.trim().is_empty()
            && !self.fields[3].value.trim().is_empty()
    }

    pub fn next_field(&mut self) {
        self.cursor = (self.cursor + 1) % self.fields.len();
    }

    pub fn prev_field(&mut self) {
        self.cursor = (self.cursor + self.fields.len() - 1) % self.fields.len();
    }

    pub fn cycle_template(&mut self, delta: isize) {
        let len = self.templates.len() as isize;
        self.template_idx = (self.template_idx as isize + delta).rem_euclid(len) as usize;
    }

    pub fn type_char(&mut self, c: char) {
        self.fields[self.cursor].value.push(c);
    }

    pub fn backspace(&mut self) {
        self.fields[self.cursor].value.pop();
    }

    /// Render the wizard form into `area`.
    pub fn render(&self, f: &mut ratatui::Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // template picker
                Constraint::Min(3),    // fields
                Constraint::Length(3), // command preview
            ])
            .split(area);

        // Template picker.
        let tpl_line = Line::from(vec![
            Span::styled(" Template: ", Style::default().fg(Color::Cyan)),
            Span::styled(
                format!("< {} >", self.template_id()),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("   ISO: {}", self.iso())),
        ]);
        let tpl = Paragraph::new(tpl_line)
            .block(Block::default().borders(Borders::ALL).title("Template (Tab to cycle)"));
        f.render_widget(tpl, chunks[0]);

        // Fields.
        let items: Vec<ListItem> = self
            .fields
            .iter()
            .enumerate()
            .map(|(i, fld)| {
                let marker = if i == self.cursor { "▶ " } else { "  " };
                let line = format!("{}{:<10} {:<16}  ({})", marker, fld.label, fld.value, fld.hint);
                let style = if i == self.cursor {
                    Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(line).style(style)
            })
            .collect();
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Fields (↑↓ move · type to edit)"),
        );
        f.render_widget(list, chunks[1]);

        // Command preview.
        let create = format!("vm {}", self.create_argv().join(" "));
        let install = format!("vm {}", self.install_argv().join(" "));
        let preview = Paragraph::new(vec![
            Line::from(Span::styled(create, Style::default().fg(Color::Green))),
            Line::from(Span::styled(install, Style::default().fg(Color::Green))),
        ])
        .block(Block::default().borders(Borders::ALL).title("Will run (Enter to create)"))
        .alignment(Alignment::Left);
        f.render_widget(preview, chunks[2]);
    }
}

/// A single activity/log entry.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub cmd: String,
    pub code: i32,
    pub stderr: Option<String>,
}

/// The activity log tail — the real commands run this session with their exit codes.
pub struct LogTail {
    pub entries: Vec<LogEntry>,
    pub state: ListState,
}

impl Default for LogTail {
    fn default() -> Self {
        Self { entries: Vec::new(), state: ListState::default() }
    }
}

impl LogTail {
    pub fn push(&mut self, cmd: String, code: i32, stderr: Option<String>) {
        self.entries.push(LogEntry { cmd, code, stderr });
        if self.entries.len() > 200 {
            self.entries.remove(0);
        }
    }

    pub fn render(&mut self, f: &mut ratatui::Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .entries
            .iter()
            .rev()
            .map(|e| {
                let badge = if e.code == 0 { "exit 0 " } else { "exit ! " };
                let color = if e.code == 0 { Color::Green } else { Color::Red };
                let mut spans = vec![
                    Span::styled(badge, Style::default().fg(color).add_modifier(Modifier::BOLD)),
                    Span::raw(e.cmd.clone()),
                ];
                if let Some(err) = &e.stderr {
                    spans.push(Span::styled(format!("   ↳ {err}"), Style::default().fg(Color::Red)));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Activity (real vm(8) commands this session)"),
        );
        f.render_stateful_widget(list, area, &mut self.state);
    }
}

/// The console view. We never exec `vm console` from inside the event loop (it
/// would swallow the terminal); instead we show the exact command to run and
/// the guest's console state, mirroring the desktop's console pane.
pub fn render_console(f: &mut ratatui::Frame, area: Rect, guest: &str, running: bool) {
    let lines: Vec<Line> = if running {
        vec![
            Line::from(Span::styled(
                "Console attach is interactive — run it in a real shell:",
                Style::default().fg(Color::Cyan),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("   vm console {guest}"),
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::raw(
                "This keeps the TUI from swallowing your terminal. Copy the line above,",
            )),
            Line::from(Span::raw(
                "or open a second terminal and run it there to attach to the guest's serial console.",
            )),
        ]
    } else {
        vec![
            Line::from(Span::styled(
                "Guest is not running — no console to attach.",
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
            Line::from(Span::raw(format!("Start {guest} first, then attach."))),
        ]
    };
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(format!("Console · {guest}")))
        .alignment(Alignment::Left);
    f.render_widget(p, area);
}

/// The config editor pane — shows the guest's `key=value` config surface and
/// the exact `vm` command to change it. Editing is deliberate: we show the
/// command rather than silently rewriting config files.
pub fn render_config(f: &mut ratatui::Frame, area: Rect, guest: &str, cpu: &str, mem: &str) {
    let lines = vec![
        Line::from(Span::styled(
            format!("{guest} configuration"),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("  cpus      = {cpu}")),
        Line::from(format!("  memory    = {mem}")),
        Line::from("  loader    = (from template)"),
        Line::from("  network   = virtio-net,switch=public"),
        Line::from(""),
        Line::from(Span::styled(
            "To change, stop the guest and edit its config:",
            Style::default().fg(Color::Cyan),
        )),
        Line::from(Span::styled(
            format!("   vm stop {guest} && ee /zroot/vm/{guest}/{guest}.conf"),
            Style::default().fg(Color::Green),
        )),
    ];
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Config"))
        .alignment(Alignment::Left);
    f.render_widget(p, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wizard_builds_create_and_install_argv() {
        let mut w = Wizard::default();
        w.fields[0].value = "ubuntu-01".into();
        w.template_idx = 0;
        let create = w.create_argv();
        assert_eq!(create[0], "create");
        assert!(create.contains(&"ubuntu-01".to_string()));
        assert!(create.contains(&"-c".to_string()));
        let install = w.install_argv();
        assert_eq!(install[0], "install");
        assert_eq!(install[1], "ubuntu-01");
    }

    #[test]
    fn wizard_ready_requires_name() {
        let mut w = Wizard::default();
        assert!(!w.ready());
        w.fields[0].value = "x".into();
        assert!(w.ready());
    }

    #[test]
    fn wizard_template_cycles() {
        let mut w = Wizard::default();
        let first = w.template_id();
        w.cycle_template(1);
        assert_ne!(w.template_id(), first);
        w.cycle_template(-1);
        assert_eq!(w.template_id(), first);
    }

    #[test]
    fn pane_hotkeys_map_to_panes() {
        assert_eq!(Pane::from_digit('1'), Some(Pane::Guests));
        assert_eq!(Pane::from_digit('5'), Some(Pane::Log));
        assert_eq!(Pane::from_digit('9'), None);
    }

    #[test]
    fn log_tail_records_exit_codes() {
        let mut log = LogTail::default();
        log.push("vm start x".into(), 0, None);
        log.push("vm start y".into(), 1, Some("locked".into()));
        assert_eq!(log.entries.len(), 2);
        assert_eq!(log.entries[1].code, 1);
    }
}
