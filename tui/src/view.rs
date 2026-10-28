//! Rendering. Pure function of `App` → frame; no I/O.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Gauge, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

use vmctl_core::inventory::{Guest, Privilege};
use vmctl_core::model::{fmt_bytes, fmt_local, fmt_mib, now, VmState};
use vmctl_core::ops::suggest;
use vmctl_core::planner::explain_stderr;

use crate::app::{App, Modal, StepState, View, ACTIONS, WIZ_FIELDS};
use crate::theme::{self as t, AMBER, BG, CRIMSON, EMBER, GREEN, MUTED, RAISED, SUB, TEXT, VIOLET};

fn block(title: impl AsRef<str>) -> Block<'static> {
    let title = title.as_ref();
    Block::default().borders(Borders::ALL).border_style(t::border()).title(Span::styled(format!(" {title} "), t::sub())).style(t::bg())
}

pub fn state_color(s: &VmState) -> ratatui::style::Color {
    match s {
        VmState::Running(_) | VmState::Bootloader(_) => GREEN,
        VmState::Suspended => VIOLET,
        VmState::Locked(_) => AMBER,
        VmState::Stopped => MUTED,
        VmState::Other(_) => CRIMSON,
    }
}

fn pill(s: &VmState) -> Span<'static> {
    let glyph = match s {
        VmState::Running(_) | VmState::Bootloader(_) => "●",
        VmState::Suspended => "◐",
        VmState::Locked(_) => "⚿",
        _ => "○",
    };
    Span::styled(format!("{glyph} {}", s.kind()), Style::default().fg(state_color(s)).bg(BG).add_modifier(Modifier::BOLD))
}

pub fn draw(f: &mut Frame, app: &App) {
    f.render_widget(Block::default().style(t::bg()), f.area());
    let banner = banner_text(app);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(if banner.is_some() { 1 } else { 0 }), Constraint::Min(5), Constraint::Length(if app.toast.is_some() { 3 } else { 0 }), Constraint::Length(1)])
        .split(f.area());
    tabs(f, app, rows[0]);
    if let Some((txt, st)) = banner {
        f.render_widget(Paragraph::new(Span::styled(txt, st)), rows[1]);
    }
    match app.view {
        View::Machines => machines(f, app, rows[2]),
        View::Snapshots => snapshots(f, app, rows[2]),
        View::Network => network(f, app, rows[2]),
        View::Storage => storage(f, app, rows[2]),
        View::Activity => activity(f, app, rows[2]),
        View::Assistant => assistant(f, app, rows[2]),
    }
    if app.toast.is_some() {
        toast(f, app, rows[3]);
    }
    status(f, app, rows[4]);
    match &app.modal {
        Modal::None => {}
        Modal::Help => help(f),
        Modal::Confirm(_) => confirm(f, app),
        Modal::Wizard(_) => wizard(f, app),
        Modal::Input(i) => {
            let a = centered(f.area(), 56, 5);
            f.render_widget(Clear, a);
            let p = Paragraph::new(vec![Line::from(Span::styled(format!("{}_", i.value), t::text())), Line::from(Span::styled("Enter create · Esc cancel", t::muted()))]).block(block(&i.label));
            f.render_widget(p, a);
        }
    }
}

/// Demo / read-only / partial-inventory banner (None when all is well).
pub fn banner_text(app: &App) -> Option<(String, Style)> {
    if app.demo {
        return Some((" DEMO HOST — nothing here touches vm-bhyve. Run without --demo for the real host.".into(), t::pill(BG, EMBER)));
    }
    let inv = app.inv.as_ref()?;
    if inv.privilege == Privilege::None {
        return Some((
            format!(" READ-ONLY — {} is not root and `sudo -n` needs a password. Fix sudo, then press p to re-check.", if inv.host.user.is_empty() { "this user" } else { &inv.host.user }),
            t::pill(BG, AMBER),
        ));
    }
    inv.error.as_ref().map(|e| (format!(" PARTIAL — {}", e.lines().next().unwrap_or("")), t::pill(TEXT, CRIMSON)))
}

fn tabs(f: &mut Frame, app: &App, a: Rect) {
    let mut spans = vec![Span::styled(" vmctl ", t::pill(BG, CRIMSON)), Span::raw(" ")];
    for (i, v) in View::ALL.iter().enumerate() {
        let st = if *v == app.view { t::focus().add_modifier(Modifier::UNDERLINED) } else { t::muted() };
        spans.push(Span::styled(format!(" {} {} ", i + 1, v.title()), st));
    }
    if app.busy {
        spans.push(Span::styled("  ⟳ working…", t::ember()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(t::bg()), a);
}

fn status(f: &mut Frame, app: &App, a: Rect) {
    let left = match &app.inv {
        None => "vm list · loading…".to_string(),
        Some(inv) => {
            let (c, m) = inv.allocated();
            let priv_ = match inv.privilege {
                Privilege::Root => "root",
                Privilege::Sudo => "sudo",
                Privilege::None => "read-only",
            };
            format!(
                " {} · {} · vm {} · {} guests · {}/{} vCPU · {}/{} · {} · list {}ms",
                inv.host.hostname,
                inv.host.os,
                inv.host.vm_version,
                inv.guests.len(),
                c,
                inv.host.threads,
                fmt_mib(m),
                fmt_mib(inv.host.mem_mib),
                priv_,
                inv.list_ms
            )
        }
    };
    let keys = match app.view {
        View::Machines => "←→ action · ⏎ run · c console · n new · ? help · q quit",
        View::Snapshots => "b rollback · c clone · d delete · ? help",
        View::Network => "a add switch · d delete · ? help",
        View::Assistant => "type · ⏎ ask · ⏎ on empty runs plan · Esc back",
        _ => "Tab/1-6 views · r refresh · ? help · q quit",
    };
    let cols = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Min(10), Constraint::Length(keys.chars().count() as u16 + 1)]).split(a);
    f.render_widget(Paragraph::new(Span::styled(left, t::muted())), cols[0]);
    f.render_widget(Paragraph::new(Span::styled(keys, t::muted())).alignment(Alignment::Right), cols[1]);
}

fn toast(f: &mut Frame, app: &App, a: Rect) {
    let Some(ts) = &app.toast else { return };
    let (tag, col) = if ts.code == 0 { ("✓ exit 0".to_string(), GREEN) } else { (format!("✗ exit {}", ts.code), CRIMSON) };
    let mut lines = vec![Line::from(vec![Span::styled(format!(" {tag} "), t::pill(BG, col)), Span::raw(" "), Span::styled(ts.cmd.clone(), t::text())])];
    if !ts.stderr.is_empty() {
        lines.push(Line::from(Span::styled(ts.stderr.lines().next().unwrap_or("").to_string(), Style::default().fg(col).bg(BG))));
    } else if ts.code != 0 {
        lines.push(Line::from(Span::styled("(no stderr)", t::muted())));
    }
    if ts.code != 0 && !ts.stderr.is_empty() {
        lines[1].spans.push(Span::styled(format!("  — {}", explain_stderr(&ts.stderr)), t::muted()));
    }
    f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::TOP).border_style(Style::default().fg(col).bg(BG))).wrap(Wrap { trim: true }), a);
}

// ───────────────────────── machines ─────────────────────────

fn machines(f: &mut Frame, app: &App, a: Rect) {
    let cols = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Percentage(42), Constraint::Percentage(58)]).split(a);
    let Some(inv) = &app.inv else {
        f.render_widget(Paragraph::new("loading inventory…").style(t::muted()).block(block("Machines")), cols[0]);
        return;
    };
    let rows: Vec<Row> = inv
        .guests
        .iter()
        .map(|g| {
            Row::new(vec![
                Cell::from(pill(&g.state)),
                Cell::from(Span::styled(g.name.clone(), t::text())),
                Cell::from(Span::styled(format!("{}c", g.cpu), t::sub())),
                Cell::from(Span::styled(fmt_mib(g.mem_mib), t::sub())),
                Cell::from(Span::styled(if g.autostart { "auto" } else { "" }, t::muted())),
            ])
        })
        .collect();
    let table = Table::new(rows, [Constraint::Length(12), Constraint::Min(10), Constraint::Length(4), Constraint::Length(7), Constraint::Length(4)])
        .header(Row::new(vec!["STATE", "NAME", "CPU", "MEM", ""]).style(t::muted()))
        .row_highlight_style(t::selection())
        .block(block(format!("Machines ({})", inv.guests.len())));
    let mut ts = TableState::default().with_selected(if inv.guests.is_empty() { None } else { Some(app.sel) });
    f.render_stateful_widget(table, cols[0], &mut ts);
    if inv.guests.is_empty() {
        let inner = cols[0].inner(ratatui::layout::Margin { horizontal: 2, vertical: 3 });
        f.render_widget(Paragraph::new("No machines yet. Press n to create one.").style(t::muted()), inner);
    }
    detail(f, app, cols[1]);
}

fn detail(f: &mut Frame, app: &App, a: Rect) {
    let Some(g) = app.guest() else {
        f.render_widget(block("Details"), a);
        return;
    };
    let parts = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(6), Constraint::Length(3)]).split(a);
    let mut l: Vec<Line> = Vec::new();
    let kv = |k: &str, v: String| Line::from(vec![Span::styled(format!("{k:<11}"), t::muted()), Span::styled(v, t::text())]);
    l.push(Line::from(vec![Span::styled(g.name.clone(), t::text().add_modifier(Modifier::BOLD)), Span::raw("  "), pill(&g.state), Span::styled(format!("  {}", g.state.label()), t::muted())]));
    l.push(kv("loader", g.loader.clone()));
    l.push(kv("cpu / mem", format!("{} vCPU · {}", g.cpu, fmt_mib(g.mem_mib))));
    l.push(kv("datastore", format!("{} · {}", g.datastore, g.dir.display())));
    l.push(kv("console", g.console()));
    l.push(kv("autostart", if g.autostart { "yes".into() } else { "no".into() }));
    if let Some(i) = &g.info {
        for n in &i.nics {
            l.push(kv(&format!("nic{}", n.number), format!("{} → {} · {}", n.emulation, n.switch, n.mac)));
        }
        for d in &i.disks {
            l.push(kv(&format!("disk{}", d.number), format!("{} {} · {} (used {})", d.emulation, d.path, d.size, d.used)));
        }
    }
    l.push(kv("snapshots", if g.snaps.is_empty() { "none".into() } else { g.snaps.iter().map(|s| s.name.clone()).collect::<Vec<_>>().join(", ") }));
    let s = suggest(g);
    if s.kind != "none" {
        l.push(Line::raw(""));
        l.push(Line::from(vec![Span::styled("▸ ", t::ember()), Span::styled(s.text, t::sub())]));
        if !s.cmd.is_empty() {
            l.push(Line::from(Span::styled(format!("  $ {}", s.cmd), t::muted())));
        }
    }
    if !g.log_tail.is_empty() {
        l.push(Line::raw(""));
        l.push(Line::from(Span::styled("vm-bhyve.log", t::muted())));
        for x in g.log_tail.iter().rev().take(4).rev() {
            l.push(Line::from(Span::styled(format!("  {x}"), t::muted())));
        }
    }
    f.render_widget(Paragraph::new(l).wrap(Wrap { trim: false }).block(block("Details")), parts[0]);
    action_bar(f, app, g, parts[1]);
}

fn action_bar(f: &mut Frame, app: &App, _g: &Guest, a: Rect) {
    let mut spans = Vec::new();
    for (i, (label, key)) in ACTIONS.iter().enumerate() {
        let ok = app.action_state(key).is_ok();
        let mut st = if ok { if *key == "destroy" || *key == "poweroff" { Style::default().fg(CRIMSON).bg(BG) } else { t::text() } } else { Style::default().fg(RAISED).bg(BG) };
        if !ok {
            st = Style::default().fg(MUTED).bg(BG).add_modifier(Modifier::DIM | Modifier::CROSSED_OUT);
        }
        if i == app.action {
            st = st.bg(RAISED).fg(if ok { EMBER } else { MUTED }).add_modifier(Modifier::BOLD).remove_modifier(Modifier::DIM);
        }
        spans.push(Span::styled(format!(" {label} "), st));
    }
    let why = app.action_state(ACTIONS[app.action].1).err().map(|w| format!("  ⓘ {w}")).unwrap_or_default();
    let p = Paragraph::new(vec![Line::from(spans), Line::from(Span::styled(why, t::warn()))]).block(Block::default().borders(Borders::TOP).border_style(t::border()));
    f.render_widget(p, a);
}

// ───────────────────────── other views ─────────────────────────

fn snapshots(f: &mut Frame, app: &App, a: Rect) {
    let rows: Vec<Row> = app
        .snap_rows()
        .iter()
        .map(|(g, s)| {
            Row::new(vec![
                Cell::from(Span::styled(g.name.clone(), t::sub())),
                Cell::from(Span::styled(format!("@{}", s.name), t::text())),
                Cell::from(Span::styled(fmt_local(s.created, true), t::muted())),
                Cell::from(Span::styled(fmt_bytes(s.used), t::sub())),
                Cell::from(Span::styled(s.datasets.len().to_string(), t::muted())),
            ])
        })
        .collect();
    let n = rows.len();
    let table = Table::new(rows, [Constraint::Length(18), Constraint::Min(16), Constraint::Length(17), Constraint::Length(9), Constraint::Length(4)])
        .header(Row::new(vec!["MACHINE", "SNAPSHOT", "TAKEN", "USED", "DS"]).style(t::muted()))
        .row_highlight_style(t::selection())
        .block(block(format!("Snapshots ({n}) · zfs")));
    let mut ts = TableState::default().with_selected(if n == 0 { None } else { Some(app.snap_sel) });
    f.render_stateful_widget(table, a, &mut ts);
}

fn network(f: &mut Frame, app: &App, a: Rect) {
    let Some(inv) = &app.inv else { return };
    let rows: Vec<Row> = inv
        .switches
        .iter()
        .map(|s| {
            let users = inv.guests.iter().filter(|g| g.switches().contains(&s.name)).count();
            Row::new(vec![s.name.clone(), s.kind.clone(), s.iface.clone(), s.address.clone(), s.private.clone(), s.ports.clone(), users.to_string()]).style(t::text())
        })
        .collect();
    let table = Table::new(rows, [Constraint::Length(12), Constraint::Length(9), Constraint::Length(14), Constraint::Length(18), Constraint::Length(8), Constraint::Min(8), Constraint::Length(6)])
        .header(Row::new(vec!["NAME", "TYPE", "IFACE", "ADDRESS", "PRIVATE", "PORTS", "VMS"]).style(t::muted()))
        .row_highlight_style(t::selection())
        .block(block(format!("Virtual switches ({})", inv.switches.len())));
    let mut ts = TableState::default().with_selected(if inv.switches.is_empty() { None } else { Some(app.sw_sel) });
    f.render_stateful_widget(table, a, &mut ts);
}

fn storage(f: &mut Frame, app: &App, a: Rect) {
    let Some(inv) = &app.inv else { return };
    let h = (inv.datastores.len() as u16) * 2 + 2;
    let parts = Layout::default().direction(Direction::Vertical).constraints([Constraint::Length(h), Constraint::Min(3)]).split(a);
    let b = block(format!("Datastores ({})", inv.datastores.len()));
    let inner = b.inner(parts[0]);
    f.render_widget(b, parts[0]);
    for (i, d) in inv.datastores.iter().enumerate() {
        let y = inner.y + (i as u16) * 2;
        if y + 2 > inner.y + inner.height {
            break;
        }
        let label = format!("{}  {}  {}  {} used · {} free", d.name, d.kind, d.path, fmt_bytes(d.used), fmt_bytes(d.avail));
        f.render_widget(Paragraph::new(Span::styled(label, t::text())), Rect { x: inner.x, y, width: inner.width, height: 1 });
        let pct = d.used_pct();
        let col = if pct > 85.0 { CRIMSON } else if pct > 70.0 { AMBER } else { GREEN };
        f.render_widget(
            Gauge::default().gauge_style(Style::default().fg(col).bg(RAISED)).ratio((pct / 100.0).clamp(0.0, 1.0) as f64).label(format!("{pct:.0}%")),
            Rect { x: inner.x, y: y + 1, width: inner.width, height: 1 },
        );
    }
    let items: Vec<ListItem> = inv
        .isos
        .iter()
        .map(|i| ListItem::new(Line::from(vec![Span::styled(format!("{:<48}", i.name), t::text()), Span::styled(format!("{:>9}  {}", i.size_h(), i.source), t::muted())])))
        .collect();
    let mut ls = ListState::default().with_selected(if inv.isos.is_empty() { None } else { Some(app.iso_sel) });
    f.render_stateful_widget(List::new(items).highlight_style(t::selection()).block(block(format!("Install images ({})", inv.isos.len()))), parts[1], &mut ls);
}

fn activity(f: &mut Frame, app: &App, a: Rect) {
    let today = fmt_local(now(), true).chars().take(10).collect::<String>();
    let rows: Vec<Row> = app
        .log
        .entries
        .iter()
        .take(500)
        .map(|e| {
            let full = fmt_local(e.ts, true);
            let time = if full.starts_with(&today) { fmt_local(e.ts, false) } else { full };
            let code = Span::styled(format!("{:>3}", e.code), Style::default().fg(if e.code == 0 { GREEN } else { CRIMSON }).bg(BG));
            Row::new(vec![
                Cell::from(Span::styled(time, t::muted())),
                Cell::from(code),
                Cell::from(Span::styled(e.source.clone(), t::muted())),
                Cell::from(Span::styled(e.cmd.clone(), t::text())),
                Cell::from(Span::styled(e.stderr.lines().next().unwrap_or("").to_string(), Style::default().fg(CRIMSON).bg(BG))),
            ])
        })
        .collect();
    let n = rows.len();
    let table = Table::new(rows, [Constraint::Length(16), Constraint::Length(4), Constraint::Length(8), Constraint::Percentage(45), Constraint::Min(10)])
        .header(Row::new(vec!["TIME", "RC", "FROM", "COMMAND", "STDERR"]).style(t::muted()))
        .row_highlight_style(t::selection())
        .block(block(format!("Activity ({n}) · shared with desktop")));
    let mut ts = TableState::default().with_selected(if n == 0 { None } else { Some(app.log_sel) });
    f.render_stateful_widget(table, a, &mut ts);
}

fn assistant(f: &mut Frame, app: &App, a: Rect) {
    let parts = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(3), Constraint::Length(3)]).split(a);
    let mut l: Vec<Line> = Vec::new();
    if app.msgs.is_empty() {
        l.push(Line::from(Span::styled("Ask in plain English. I plan with real vm(8)/zfs commands; nothing runs until you press Enter on an empty prompt.", t::sub())));
        l.push(Line::raw(""));
        for e in vmctl_core::planner::examples() {
            l.push(Line::from(vec![Span::styled("  • ", t::ember()), Span::styled(e, t::muted())]));
        }
    }
    for m in &app.msgs {
        l.push(Line::from(vec![Span::styled("you ▸ ", t::ember()), Span::styled(m.user.clone(), t::text())]));
        for line in m.reply.text.lines() {
            l.push(Line::from(Span::styled(line.to_string(), t::sub())));
        }
        if let Some(e) = &m.reply.stderr {
            l.push(Line::from(Span::styled(format!("  stderr: {e}"), Style::default().fg(CRIMSON).bg(BG))));
        }
        if !m.reply.chips.is_empty() {
            let chips: Vec<Span> = m.reply.chips.iter().map(|c| Span::styled(format!(" {}={} ", c.key, c.value), if c.filled { t::muted() } else { t::ember() })).collect();
            l.push(Line::from(chips));
        }
        for (i, s) in m.reply.steps.iter().enumerate() {
            let (g, col) = match m.states[i] {
                StepState::Pending => ("·", MUTED),
                StepState::Ok => ("✓", GREEN),
                StepState::Failed => ("✗", CRIMSON),
                StepState::Skipped => ("–", MUTED),
            };
            l.push(Line::from(vec![Span::styled(format!("  {g} {}. ", i + 1), Style::default().fg(col).bg(BG)), Span::styled(s.title.clone(), t::text()), Span::styled(format!("  $ {}", s.op.cmd()), t::muted())]));
            if !m.stderr[i].is_empty() {
                l.push(Line::from(Span::styled(format!("      {}", m.stderr[i]), Style::default().fg(CRIMSON).bg(BG))));
                l.push(Line::from(Span::styled(format!("      {}", explain_stderr(&m.stderr[i])), t::muted())));
            }
        }
        for s in &m.reply.skipped {
            l.push(Line::from(Span::styled(format!("  skipped: {s}"), t::muted())));
        }
        if m.reply.has_plan() {
            let tail = if m.ran { "ran".to_string() } else { format!("risk {} · touches {} · ⏎ on empty prompt to run", m.reply.risk, m.reply.touches) };
            l.push(Line::from(Span::styled(format!("  {tail}"), t::muted())));
        }
        for c in &m.reply.code {
            l.push(Line::from(Span::styled(format!("  $ {c}"), t::muted())));
        }
        l.push(Line::raw(""));
    }
    let inner_h = parts[0].height.saturating_sub(2) as usize;
    let scroll = l.len().saturating_sub(inner_h) as u16;
    f.render_widget(Paragraph::new(l).wrap(Wrap { trim: false }).scroll((scroll, 0)).block(block("Ask vmctl")), parts[0]);
    let prompt = Paragraph::new(Line::from(vec![Span::styled("▸ ", t::ember()), Span::styled(format!("{}_", app.input), t::text())])).block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(EMBER).bg(BG)));
    f.render_widget(prompt, parts[1]);
}

// ───────────────────────── modals ─────────────────────────

pub fn centered(a: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(a.width);
    let h = h.min(a.height);
    Rect { x: a.x + (a.width - w) / 2, y: a.y + (a.height - h) / 2, width: w, height: h }
}

fn confirm(f: &mut Frame, app: &App) {
    let Modal::Confirm(c) = &app.modal else { return };
    let a = centered(f.area(), 72, 9 + c.ops.len() as u16);
    f.render_widget(Clear, a);
    let what = match c.kind.as_str() {
        "poweroff" => format!("Hard power-off {}? Like pulling the plug; unsaved guest data is lost.", c.name),
        "destroy" => format!("Destroy {}? Its disks and every snapshot are deleted for good.", c.name),
        "rollback" => format!("Roll {} back to @{}? Newer snapshots are destroyed with it.", c.name, c.snap),
        "snapdelete" => format!("Delete {}@{}? This can't be undone.", c.name, c.snap),
        "unlock" => format!("Remove {}'s run.lock? Only if the owning host is really gone.", c.name),
        "switchdestroy" => format!("Delete switch {}? Guests attached to it lose that NIC's network.", c.name),
        k => format!("{k} {}?", c.name),
    };
    let mut l = vec![Line::from(Span::styled(what, t::text())), Line::raw(""), Line::from(Span::styled("Will run:", t::muted()))];
    for o in &c.ops {
        l.push(Line::from(Span::styled(format!("  $ {}", o.cmd()), if o.is_destructive() { Style::default().fg(CRIMSON).bg(BG) } else { t::sub() })));
    }
    l.push(Line::raw(""));
    if c.kind == "poweroff" {
        l.push(Line::from(Span::styled(format!("[{}] snapshot first (space toggles)", if c.safety { "x" } else { " " }), t::ember())));
    }
    l.push(Line::from(vec![Span::styled(" y ", t::pill(TEXT, CRIMSON)), Span::styled(" confirm   ", t::sub()), Span::styled(" n ", t::pill(TEXT, RAISED)), Span::styled(" cancel", t::sub())]));
    f.render_widget(Paragraph::new(l).wrap(Wrap { trim: false }).block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(CRIMSON).bg(BG)).title(" Confirm ").style(t::bg())), a);
}

fn wizard(f: &mut Frame, app: &App) {
    let Modal::Wizard(w) = &app.modal else { return };
    let ops = app.wiz_ops(w);
    let a = centered(f.area(), 76, 16 + ops.len() as u16);
    f.render_widget(Clear, a);
    let s = &w.spec;
    let vals = [
        s.tpl.clone(),
        format!("{}_", s.name),
        s.cpu.to_string(),
        format!("{} GiB", s.mem_gib),
        format!("{} GiB", s.disk_gib),
        s.ds.clone(),
        if s.sw.is_empty() { "(template default)".into() } else { s.sw.clone() },
        if s.iso.is_empty() { "none".into() } else { s.iso.clone() },
        if s.install { "yes".into() } else { "no".into() },
        if s.autostart { "yes".into() } else { "no".into() },
    ];
    let mut l = Vec::new();
    for (i, (k, v)) in WIZ_FIELDS.iter().zip(vals).enumerate() {
        let focus = i == w.field;
        let marker = if focus { "▸ " } else { "  " };
        let vst = if focus { t::focus() } else { t::text() };
        let v = if focus && i != 1 { format!("◂ {v} ▸") } else { v };
        l.push(Line::from(vec![Span::styled(marker, t::ember()), Span::styled(format!("{k:<15}"), t::muted()), Span::styled(v, vst)]));
    }
    l.push(Line::raw(""));
    match app.wiz_error(w) {
        Some(e) => l.push(Line::from(Span::styled(e, t::warn()))),
        None => {
            l.push(Line::from(Span::styled("Will run:", t::muted())));
            for o in &ops {
                l.push(Line::from(Span::styled(format!("  $ {}", o.cmd()), t::sub())));
            }
        }
    }
    l.push(Line::from(Span::styled("↑↓ field · ←→ change · type name · ⏎ create · Esc cancel", t::muted())));
    f.render_widget(Paragraph::new(l).block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(EMBER).bg(BG)).title(" New machine ").style(t::bg())), a);
}

fn help(f: &mut Frame) {
    let a = centered(f.area(), 64, 20);
    f.render_widget(Clear, a);
    let rows = [
        ("Tab / 1-6", "switch view"),
        ("↑↓ / j k", "move"),
        ("←→ / h l", "pick action (Machines)"),
        ("⏎", "run the highlighted action"),
        ("s / S / t", "start / stop / snapshot"),
        ("c", "console (suspends the TUI) · clone in Snapshots"),
        ("n", "new machine wizard"),
        ("a", "ask vmctl · add switch in Network"),
        ("b / d", "rollback / delete snapshot · delete switch"),
        ("r / F5", "refresh"),
        ("p", "re-check root/sudo"),
        ("Esc", "dismiss / back"),
        ("q / Ctrl-C", "quit"),
    ];
    let mut l: Vec<Line> = rows.iter().map(|(k, v)| Line::from(vec![Span::styled(format!("{k:<14}"), t::ember()), Span::styled(*v, t::sub())])).collect();
    l.push(Line::raw(""));
    l.push(Line::from(Span::styled("Every command is real vm(8)/zfs; results land in Activity.", t::muted())));
    f.render_widget(Paragraph::new(l).block(Block::default().borders(Borders::ALL).border_style(t::border()).title(" Keys ").style(Style::default().fg(SUB).bg(BG))), a);
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::demo_app;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use vmctl_core::demo::DemoHost;
    use vmctl_core::inventory::Hypervisor;
    use vmctl_core::worker::Event;

    fn render(app: &App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    /// Golden render. `UPDATE_SNAPSHOTS=1 cargo test -p vmctl-tui` rewrites them.
    /// Lines containing a clock (status/activity) are not in the goldens.
    fn golden(name: &str, got: &str) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(format!("{name}.txt"));
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() || !path.exists() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, got).unwrap();
            return;
        }
        let want = std::fs::read_to_string(&path).unwrap();
        assert_eq!(got, want, "render of {name} changed; rerun with UPDATE_SNAPSHOTS=1 if intended");
    }

    #[test]
    fn machines_view_golden() {
        let app = demo_app();
        let s = render(&app, 120, 34);
        for want in ["DEMO HOST", "ubuntu-2404", "alpine-test", "● Running", "◐ Suspended", "⚿ Locked", "Machines (6)", "Start"] {
            assert!(s.contains(want), "missing {want:?}\n{s}");
        }
        golden("machines", &s);
    }

    #[test]
    fn greyed_action_shows_reason() {
        let mut app = demo_app();
        app.action = 0; // Start on a running guest
        assert!(render(&app, 120, 34).contains("ⓘ"));
    }

    #[test]
    fn read_only_banner() {
        let mut h = DemoHost::new();
        h.privilege = Privilege::None;
        let mut app = App::new(vmctl_core::activity::ActivityLog::memory(), false);
        app.on_event(Event::Inventory(Box::new(h.inventory())));
        let s = render(&app, 160, 30);
        assert!(s.contains("READ-ONLY"), "{s}");
        assert!(s.contains("read-only ·"), "status bar shows privilege\n{s}");
    }

    #[test]
    fn every_view_renders_at_small_and_large_sizes() {
        let mut app = demo_app();
        for v in View::ALL {
            app.view = v;
            for (w, h) in [(20, 6), (40, 10), (80, 24), (200, 60)] {
                assert!(!render(&app, w, h).trim().is_empty());
            }
        }
        app.open_wizard();
        render(&app, 30, 8);
        app.modal = Modal::Help;
        render(&app, 30, 8);
    }

    #[test]
    fn views_show_inventory_sources() {
        let mut app = demo_app();
        app.view = View::Snapshots;
        assert!(render(&app, 120, 30).contains("@fresh-install"));
        app.view = View::Network;
        assert!(render(&app, 120, 30).contains("nat0"));
        app.view = View::Storage;
        let s = render(&app, 120, 30);
        assert!(s.contains("Datastores (2)") && s.contains("FreeBSD-14.3-RELEASE"), "{s}");
        app.view = View::Assistant;
        assert!(render(&app, 120, 30).contains("snapshot everything"));
    }

    #[test]
    fn confirm_and_wizard_modals_golden() {
        let mut app = demo_app();
        app.sel = app.inv.as_ref().unwrap().guests.iter().position(|g| g.name == "win11-dev").unwrap();
        app.activate("destroy");
        let s = render(&app, 120, 34);
        assert!(s.contains("win11-dev") && s.contains(" y ") && s.contains("$ vm destroy"), "{s}");
        golden("confirm_destroy", &s);
        app.modal = Modal::None;
        app.open_wizard();
        let s = render(&app, 120, 40);
        assert!(s.contains("New machine") && s.contains("$ vm create"), "{s}");
        golden("wizard", &s);
    }
}
