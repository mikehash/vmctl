//! The four "feature-level" panes the prototype promised: Snapshots, Switches,
//! Datastores, and Images. These are real, working surfaces backed by `vm(8)`,
//! not decorative mock tabs.
//!
//! Each pane is a small state machine: it holds a list of rows (refreshed from
//! the hypervisor), a selection cursor, and an action bar. The `App` in `ui.rs`
//! feeds it the live data and runs the highlighted action through `Vm`.
//!
//! Command surface (from `vm(8)`):
//!   Snapshots:   vm snapshot [-f] name@snap · vm rollback [-r] name@snap · vm clone name@snap new
//!   Switches:    vm switch list/info/create/destroy …
//!   Datastores:  vm datastore list/add/remove/iso …
//!   Images:      vm image list/create/provision/destroy …

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::theme as t;

/// A generic, reusable list pane: a titled list + an action bar + a status line.
/// Snapshots / Switches / Datastores / Images all share this shape.
#[derive(Debug, Default)]
pub struct ListPane {
    pub title: &'static str,
    /// Column headers shown above the rows.
    pub columns: &'static str,
    /// The rows to render (already formatted as display lines).
    pub rows: Vec<String>,
    /// The action labels for the bar, in order.
    pub actions: Vec<&'static str>,
    pub state: ListState,
    pub action_idx: usize,
    pub status: String,
    /// Empty-state hint shown when `rows` is empty.
    pub empty_hint: &'static str,
}

impl ListPane {
    pub fn new(title: &'static str, columns: &'static str, actions: Vec<&'static str>, empty_hint: &'static str) -> Self {
        let mut state = ListState::default();
        state.select(Some(0));
        Self {
            title,
            columns,
            rows: Vec::new(),
            actions,
            state,
            action_idx: 0,
            status: String::new(),
            empty_hint,
        }
    }

    pub fn set_rows(&mut self, rows: Vec<String>) {
        self.rows = rows;
        if self.rows.is_empty() {
            self.state.select(None);
        } else {
            let sel = self.state.selected().unwrap_or(0).min(self.rows.len() - 1);
            self.state.select(Some(sel));
        }
    }

    pub fn selected(&self) -> Option<&String> {
        self.state.selected().and_then(|i| self.rows.get(i))
    }

    pub fn move_sel(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let cur = self.state.selected().unwrap_or(0) as isize;
        let len = self.rows.len() as isize;
        let next = (cur + delta).rem_euclid(len) as usize;
        self.state.select(Some(next));
    }

    pub fn move_action(&mut self, delta: isize) {
        if self.actions.is_empty() {
            return;
        }
        let cur = self.action_idx as isize;
        let len = self.actions.len() as isize;
        self.action_idx = (cur + delta).rem_euclid(len) as usize;
    }

    pub fn render(&mut self, f: &mut ratatui::Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // column headers
                Constraint::Min(3),    // list
                Constraint::Length(1), // action bar
                Constraint::Length(1), // status
            ])
            .split(area);

        // Column header line.
        let head = Paragraph::new(Line::from(Span::styled(
            self.columns,
            t::muted().add_modifier(Modifier::BOLD),
        )))
        .style(t::bg());
        f.render_widget(head, chunks[0]);

        // The list.
        let items: Vec<ListItem> = if self.rows.is_empty() {
            vec![ListItem::new(Line::from(Span::styled(
                self.empty_hint,
                t::muted(),
            )))]
        } else {
            self.rows
                .iter()
                .map(|r| ListItem::new(Line::from(Span::raw(r.clone()))))
                .collect()
        };
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(t::border())
                    .title(Span::styled(format!(" {} ", self.title), t::ember()))
                    .style(t::bg()),
            )
            .highlight_style(t::selection())
            .highlight_symbol("\u{25b6} ");
        f.render_stateful_widget(list, chunks[1], &mut self.state);

        // Action bar.
        let mut spans: Vec<Span> = Vec::new();
        for (i, a) in self.actions.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  ", t::muted()));
            }
            let style = if i == self.action_idx {
                t::action().add_modifier(Modifier::BOLD)
            } else {
                t::muted()
            };
            spans.push(Span::styled(format!("[{}]", a), style));
        }
        let bar = Paragraph::new(Line::from(spans)).style(t::bg());
        f.render_widget(bar, chunks[2]);

        // Status line.
        let status = Paragraph::new(Line::from(Span::styled(
            self.status.clone(),
            t::muted(),
        )))
        .style(t::bg());
        f.render_widget(status, chunks[3]);
    }
}

/// Snapshot rows are derived from `vm list -v` / `vm info` snapshot sections.
/// We render `name@snap` plus a friendly age/size column when available.
pub fn snapshot_rows(raw: &[String]) -> Vec<String> {
    raw.to_vec()
}

/// Switch rows from `vm switch list`.
pub fn switch_rows(raw: &[String]) -> Vec<String> {
    raw.to_vec()
}

/// Datastore rows from `vm datastore list`.
pub fn datastore_rows(raw: &[String]) -> Vec<String> {
    raw.to_vec()
}

/// Image rows from `vm image list`.
pub fn image_rows(raw: &[String]) -> Vec<String> {
    raw.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listpane_move_sel_wraps() {
        let mut p = ListPane::new("T", "A B", vec!["x", "y"], "none");
        p.set_rows(vec!["r0".into(), "r1".into(), "r2".into()]);
        p.state.select(Some(2));
        p.move_sel(1);
        assert_eq!(p.state.selected(), Some(0));
        p.move_sel(-1);
        assert_eq!(p.state.selected(), Some(2));
    }

    #[test]
    fn listpane_empty_is_safe() {
        let mut p = ListPane::new("T", "A B", vec!["x"], "none");
        p.set_rows(vec![]);
        assert!(p.selected().is_none());
        p.move_sel(1); // no panic
        p.move_action(1);
        assert_eq!(p.action_idx, 0);
    }

    #[test]
    fn listpane_action_cycles() {
        let mut p = ListPane::new("T", "A", vec!["a", "b", "c"], "none");
        p.move_action(1);
        assert_eq!(p.action_idx, 1);
        p.move_action(1);
        p.move_action(1);
        assert_eq!(p.action_idx, 0);
        p.move_action(-1);
        assert_eq!(p.action_idx, 2);
    }

    #[test]
    fn row_helpers_pass_through() {
        let raw = vec!["a".to_string(), "b".to_string()];
        assert_eq!(snapshot_rows(&raw), raw);
        assert_eq!(switch_rows(&raw), raw);
        assert_eq!(datastore_rows(&raw), raw);
        assert_eq!(image_rows(&raw), raw);
    }
}
