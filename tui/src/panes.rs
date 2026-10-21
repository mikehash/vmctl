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
    Assistant,
    Wizard,
    Config,
    Console,
    Log,
    Deploy,
    Snapshots,
    Switches,
    Datastores,
    Images,
}

impl Pane {
    pub const ALL: [Pane; 11] = [
        Pane::Guests,
        Pane::Assistant,
        Pane::Wizard,
        Pane::Config,
        Pane::Console,
        Pane::Log,
        Pane::Deploy,
        Pane::Snapshots,
        Pane::Switches,
        Pane::Datastores,
        Pane::Images,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            Pane::Guests => "Guests",
            Pane::Assistant => "Ask vmctl",
            Pane::Wizard => "New machine",
            Pane::Config => "Config",
            Pane::Console => "Console",
            Pane::Log => "Activity",
            Pane::Deploy => "Deploy",
            Pane::Snapshots => "Snapshots",
            Pane::Switches => "Switches",
            Pane::Datastores => "Datastores",
            Pane::Images => "Images",
        }
    }

    /// 1-9,0,- hotkey to jump straight to a pane (11 panes).
    pub fn from_digit(d: char) -> Option<Pane> {
        match d {
            '1' => Some(Pane::Guests),
            '2' => Some(Pane::Assistant),
            '3' => Some(Pane::Wizard),
            '4' => Some(Pane::Config),
            '5' => Some(Pane::Console),
            '6' => Some(Pane::Log),
            '7' => Some(Pane::Deploy),
            '8' => Some(Pane::Snapshots),
            '9' => Some(Pane::Switches),
            '0' => Some(Pane::Datastores),
            '-' => Some(Pane::Images),
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
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(crate::theme::border())
                .title(Span::styled(" ⬢ VMCTL ", crate::theme::action()))
                .title_alignment(Alignment::Left)
                .style(crate::theme::bg()),
        )
        .select(idx)
        .style(crate::theme::sub())
        .highlight_style(crate::theme::focus())
        .divider(Span::styled(" │ ", crate::theme::muted()));
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
///
/// The ISO step is a real **picker**, not a text box: it lists the install
/// media actually present in `~/installers` and the datastore's `.iso` dir, so
/// the user picks from what's on disk instead of typing a filename and hoping.
pub struct Wizard {
    pub fields: Vec<Field>,
    pub cursor: usize,
    pub template_idx: usize,
    pub templates: Vec<(&'static str, &'static str)>, // (id, iso) — fallback guesses
    /// Discovered install media (from `vmctl_core::iso::discover`).
    pub isos: Vec<vmctl_core::Iso>,
    /// True while the ISO picker overlay is open.
    pub picker_open: bool,
    /// Highlighted row in the picker.
    pub picker_idx: usize,
}

impl Default for Wizard {
    fn default() -> Self {
        Self {
            fields: vec![
                Field { label: "Name", value: String::new(), hint: "guest name (e.g. ubuntu-01)" },
                Field { label: "CPUs", value: "2".into(), hint: "vCPU count" },
                Field { label: "Memory", value: "4G".into(), hint: "e.g. 4G, 512M" },
                Field { label: "Disk", value: "20G".into(), hint: "e.g. 20G" },
                Field { label: "ISO", value: String::new(), hint: "Enter opens the picker" },
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
            isos: Vec::new(),
            picker_open: false,
            picker_idx: 0,
        }
    }
}

impl Wizard {
    pub fn template_id(&self) -> &'static str {
        self.templates[self.template_idx].0
    }

    /// The ISO to install from: whatever the picker filled in, else the
    /// template's suggested filename (so the preview is never blank).
    pub fn iso(&self) -> String {
        let picked = self.fields[4].value.trim();
        if !picked.is_empty() {
            picked.to_string()
        } else {
            self.templates[self.template_idx].1.to_string()
        }
    }

    /// Load discovered install media into the picker (called when the wizard
    /// opens, so the list reflects what's on disk right now).
    pub fn load_isos(&mut self, isos: Vec<vmctl_core::Iso>) {
        self.isos = isos;
        self.picker_idx = 0;
    }

    /// Open the ISO picker overlay (only from the ISO field).
    pub fn open_picker(&mut self) {
        self.picker_open = true;
        self.picker_idx = self
            .isos
            .iter()
            .position(|i| i.name == self.fields[4].value.trim())
            .unwrap_or(0);
    }

    pub fn close_picker(&mut self) {
        self.picker_open = false;
    }

    pub fn picker_move(&mut self, delta: isize) {
        if self.isos.is_empty() {
            return;
        }
        let len = self.isos.len() as isize;
        self.picker_idx = (self.picker_idx as isize + delta).rem_euclid(len) as usize;
    }

    /// Commit the highlighted picker row into the ISO field and close.
    pub fn picker_select(&mut self) {
        if let Some(iso) = self.isos.get(self.picker_idx) {
            self.fields[4].value = iso.name.clone();
        }
        self.picker_open = false;
    }

    /// True when the cursor is on the ISO field (the picker's trigger).
    pub fn on_iso_field(&self) -> bool {
        self.cursor == 4
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

    /// The `vm install` argv (boot the installer from the chosen ISO).
    pub fn install_argv(&self) -> Vec<String> {
        let name = self.fields[0].value.trim();
        vec!["install".into(), name.into(), self.iso()]
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
            .block(Block::default().borders(Borders::ALL).title("Template (←→ cycle)"));
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

        // ISO picker overlay — a centered box listing real install media.
        if self.picker_open {
            self.render_picker(f, area);
        }
    }

    /// Draw the ISO picker as a centered overlay over the whole wizard area.
    fn render_picker(&self, f: &mut ratatui::Frame, area: Rect) {
        // Center a 70% x 70% box.
        let w = (area.width * 70 / 100).max(30).min(area.width);
        let h = (area.height * 70 / 100).max(10).min(area.height);
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let box_area = Rect { x, y, width: w, height: h };

        // Dim the background so the picker reads as modal.
        let backdrop = Paragraph::new("").style(Style::default().bg(Color::Black));
        f.render_widget(backdrop, area);

        let title = if self.isos.is_empty() {
            "Install media — none found (Esc to close)".to_string()
        } else {
            format!(
                "Install media — {} found (↑↓ move · Enter select · Esc close)",
                self.isos.len()
            )
        };

        let items: Vec<ListItem> = if self.isos.is_empty() {
            vec![ListItem::new(
                "No .iso/.img in ~/installers or <datastore>/.iso",
            )]
        } else {
            self.isos
                .iter()
                .enumerate()
                .map(|(i, iso)| {
                    let marker = if i == self.picker_idx { "▶ " } else { "  " };
                    let line = format!(
                        "{}{:<34} {:>7}  [{}]",
                        marker,
                        iso.name,
                        iso.size_h(),
                        iso.source
                    );
                    let style = if i == self.picker_idx {
                        Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    ListItem::new(line).style(style)
                })
                .collect()
        };

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(title),
        );
        f.render_widget(list, box_area);
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
    let cfg = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Config"))
        .alignment(Alignment::Left);
    f.render_widget(cfg, area);
}

/// The Deploy pane — the "launch" tab. It shows the exact commands that will
/// bring a guest up (create + install for a new one, or start + console for an
/// existing one) and runs them on Enter. This is the one-button path from
/// "I have an ISO / a guest" to "it's booting".
pub fn render_deploy(
    f: &mut ratatui::Frame,
    area: Rect,
    guest: &str,
    has_guest: bool,
    running: bool,
    iso: &str,
) {
    let title = format!("Deploy / Launch — {guest}");
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "One-button launch. Enter runs the steps below in order.",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    if !has_guest {
        lines.push(Line::from(Span::styled(
            "No guest selected — pick one in Guests (pane 1), or build one in the wizard (pane 3).",
            Style::default().fg(Color::Yellow),
        )));
    } else if running {
        lines.push(Line::from(format!("  {guest} is already running.")));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Enter attaches the console:", Style::default().fg(Color::Cyan))));
        lines.push(Line::from(Span::styled(
            format!("   vm console {guest}"),
            Style::default().fg(Color::Green),
        )));
    } else {
        lines.push(Line::from(Span::styled("Enter boots it:", Style::default().fg(Color::Cyan))));
        lines.push(Line::from(Span::styled(
            format!("   vm start {guest}"),
            Style::default().fg(Color::Green),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Then attach the console:", Style::default().fg(Color::Cyan))));
        lines.push(Line::from(Span::styled(
            format!("   vm console {guest}"),
            Style::default().fg(Color::Green),
        )));
    }
    if !iso.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("  install media: {iso}")));
    }
    let deploy = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .alignment(Alignment::Left);
    f.render_widget(deploy, area);
}

/// The assistant pane: a plain-English prompt that turns intent into a
/// validated plan (via `assistant::plan_for`) and runs it step by step. This
/// is the "ease of use like VirtualBox" core — you type what you want, review
/// the plan it understood, and confirm.
pub struct Assistant {
    pub input: String,
    pub last_plan: Option<vmctl_core::assistant::Plan>,
    pub transcript: Vec<String>,
}

impl Default for Assistant {
    fn default() -> Self {
        Self {
            input: String::new(),
            last_plan: None,
            transcript: vec![
                "Ask vmctl in plain English, e.g.:".into(),
                "  \"make a 4GB ubuntu machine with 2 cpus\"".into(),
                "  \"start testvm\"".into(),
                "  \"stop everything\"".into(),
                "Type a request and press Enter to see the plan.".into(),
            ],
        }
    }
}

impl Assistant {
    pub fn type_char(&mut self, c: char) {
        self.input.push(c);
    }

    pub fn backspace(&mut self) {
        self.input.pop();
    }

    /// Turn the current input into a plan and stage it for confirmation.
    pub fn submit(&mut self, known: &[String]) {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.transcript.push(format!("> {text}"));
        let plan = vmctl_core::assistant::plan_for(&text, known);
        self.transcript.push(format!("  plan: {}", plan.summary));
        for s in &plan.steps {
            self.transcript.push(format!("    - {} ({})", s.cmd(), s.effect));
        }
        self.last_plan = Some(plan);
        self.input.clear();
    }

    pub fn render(&mut self, f: &mut ratatui::Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),    // transcript
                Constraint::Length(3), // plan preview (if any)
                Constraint::Length(3), // input line
            ])
            .split(area);

        // Transcript.
        let items: Vec<ListItem> = self
            .transcript
            .iter()
            .map(|l| {
                let style = if l.starts_with('>') {
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else if l.trim_start().starts_with('-') {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };
                ListItem::new(l.clone()).style(style)
            })
            .collect();
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Ask vmctl (plain English)"),
        );
        f.render_widget(list, chunks[0]);

        // Plan preview.
        if let Some(plan) = &self.last_plan {
            let preview = if plan.steps.is_empty() {
                format!("{}  [{}]", plan.summary, plan.risk)
            } else {
                format!(
                    "{} step(s) · risk {} · touches {}  — Enter to run",
                    plan.steps.len(),
                    plan.risk,
                    plan.touches
                )
            };
            let p = Paragraph::new(preview).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Plan (Enter run · Esc clear)"),
            );
            f.render_widget(p, chunks[1]);
        }

        // Input line.
        let input = Paragraph::new(format!("{}_", self.input)).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Request"),
        );
        f.render_widget(input, chunks[2]);
    }
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

    fn sample_isos() -> Vec<vmctl_core::Iso> {
        use std::path::PathBuf;
        vec![
            vmctl_core::Iso { name: "debian.iso".into(), path: PathBuf::from("debian.iso"), size: 756 * 1024 * 1024, source: "installers" },
            vmctl_core::Iso { name: "ubuntu.iso".into(), path: PathBuf::from("ubuntu.iso"), size: 2 * 1024 * 1024 * 1024, source: "installers" },
        ]
    }

    #[test]
    fn iso_field_triggers_picker_and_select_fills_field() {
        let mut w = Wizard::default();
        w.load_isos(sample_isos());
        // Cursor starts at 0 (Name) — not the ISO field.
        assert!(!w.on_iso_field());
        // Move to the ISO field (index 4).
        for _ in 0..4 {
            w.next_field();
        }
        assert!(w.on_iso_field());
        // Open the picker, move to the second ISO, select it.
        w.open_picker();
        assert!(w.picker_open);
        w.picker_move(1);
        w.picker_select();
        assert!(!w.picker_open);
        assert_eq!(w.fields[4].value, "ubuntu.iso");
        // install_argv now uses the picked ISO, not the template guess.
        assert_eq!(w.install_argv()[2], "ubuntu.iso");
    }

    #[test]
    fn picker_move_wraps_and_empty_is_safe() {
        let mut w = Wizard::default();
        // Empty picker: move/select are no-ops, never panic.
        w.picker_move(1);
        w.picker_select();
        assert!(!w.picker_open);

        w.load_isos(sample_isos());
        w.open_picker();
        w.picker_move(-1); // wraps to the last row
        assert_eq!(w.picker_idx, 1);
        w.picker_move(1); // wraps back to the first
        assert_eq!(w.picker_idx, 0);
    }

    #[test]
    fn iso_falls_back_to_template_guess_when_unset() {
        let mut w = Wizard::default();
        // No ISO picked → the template's suggested filename shows in the preview.
        assert_eq!(w.iso(), w.templates[w.template_idx].1);
        w.fields[4].value = "picked.iso".into();
        assert_eq!(w.iso(), "picked.iso");
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
        assert_eq!(Pane::from_digit('2'), Some(Pane::Assistant));
        assert_eq!(Pane::from_digit('6'), Some(Pane::Log));
        assert_eq!(Pane::from_digit('7'), Some(Pane::Deploy));
        assert_eq!(Pane::from_digit('8'), Some(Pane::Snapshots));
        assert_eq!(Pane::from_digit('9'), Some(Pane::Switches));
        assert_eq!(Pane::from_digit('0'), Some(Pane::Datastores));
        assert_eq!(Pane::from_digit('-'), Some(Pane::Images));
        assert_eq!(Pane::from_digit('x'), None);
    }

    #[test]
    fn feature_panes_are_last_and_reachable() {
        // The four feature panes (Snapshots/Switches/Datastores/Images) are the
        // trailing tabs; Images is last. Deploy remains reachable at hotkey 7.
        assert_eq!(*Pane::ALL.last().unwrap(), Pane::Images);
        assert_eq!(Pane::Images.title(), "Images");
        assert_eq!(Pane::ALL.len(), 11);
        assert_eq!(Pane::Deploy.title(), "Deploy");
    }

    #[test]
    fn log_tail_records_exit_codes() {
        let mut log = LogTail::default();
        log.push("vm start x".into(), 0, None);
        log.push("vm start y".into(), 1, Some("locked".into()));
        assert_eq!(log.entries.len(), 2);
        assert_eq!(log.entries[1].code, 1);
    }

    #[test]
    fn assistant_submits_to_plan() {
        let mut a = Assistant::default();
        a.input = "make a 4GB ubuntu machine with 2 cpus".into();
        a.submit(&["testvm".into()]);
        assert!(a.last_plan.is_some());
        let plan = a.last_plan.unwrap();
        assert!(!plan.steps.is_empty());
        assert!(a.transcript.iter().any(|l| l.starts_with("> ")));
    }

    #[test]
    fn assistant_empty_input_no_plan() {
        let mut a = Assistant::default();
        a.submit(&[]);
        assert!(a.last_plan.is_none());
    }
}
