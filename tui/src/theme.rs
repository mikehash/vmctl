//! Mission Control theme — the prototype's exact design tokens mapped to
//! ratatui `Color::Rgb`. This is the single source of truth for the TUI's
//! palette; no pane should hard-code a `Color::` variant.
//!
//! Tokens mirror `desktop/ui/vmctl.slint` so both front-ends look identical.

use ratatui::style::{Color, Modifier, Style};

// ── Surfaces ────────────────────────────────────────────────────────────────
pub const BG: Color = Color::Rgb(0x0A, 0x0B, 0x0D); // #0A0B0D app background
pub const PANEL: Color = Color::Rgb(0x0F, 0x11, 0x14); // #0F1114 panel
pub const SURFACE: Color = Color::Rgb(0x14, 0x17, 0x1B); // #14171B
pub const RAISED: Color = Color::Rgb(0x1A, 0x1E, 0x23); // #1A1E23
pub const LINE: Color = Color::Rgb(0x1E, 0x22, 0x28); // #1E2228 borders

// ── Text ────────────────────────────────────────────────────────────────────
pub const TEXT: Color = Color::Rgb(0xEC, 0xEE, 0xF1); // #ECEEF1 primary
pub const SUB: Color = Color::Rgb(0xB4, 0xBA, 0xC4); // #B4BAC4 secondary
pub const MUTED: Color = Color::Rgb(0x9A, 0xA1, 0xAC); // #9AA1AC dim

// ── Accents ─────────────────────────────────────────────────────────────────
pub const CRIMSON: Color = Color::Rgb(0xDC, 0x2F, 0x45); // #DC2F45 action
pub const EMBER: Color = Color::Rgb(0xFF, 0x7A, 0x3D); // #FF7A3D focus/assistant

// ── State ───────────────────────────────────────────────────────────────────
pub const GREEN: Color = Color::Rgb(0x34, 0xD3, 0x99); // #34D399 running/ok
pub const AMBER: Color = Color::Rgb(0xF5, 0xB5, 0x3D); // #F5B53D warning
pub const VIOLET: Color = Color::Rgb(0xB7, 0x9C, 0xFF); // #B79CFF suspended

// ── Style helpers ───────────────────────────────────────────────────────────

/// App background fill.
pub fn bg() -> Style {
    Style::default().bg(BG)
}

/// Panel background with a dim border.
pub fn panel() -> Style {
    Style::default().bg(PANEL).fg(SUB)
}

/// Primary text on the app background.
pub fn text() -> Style {
    Style::default().fg(TEXT).bg(BG)
}

/// Secondary/dim text.
pub fn sub() -> Style {
    Style::default().fg(SUB).bg(BG)
}

/// Even dimmer text (hints, column headers).
pub fn muted() -> Style {
    Style::default().fg(MUTED).bg(BG)
}

/// Border style for panels.
pub fn border() -> Style {
    Style::default().fg(LINE).bg(BG)
}

/// Active-tab / focus underline in ember.
pub fn focus() -> Style {
    Style::default().fg(EMBER).bg(BG).add_modifier(Modifier::BOLD)
}

/// Primary action (crimson) — the "do it" color.
pub fn action() -> Style {
    Style::default().fg(CRIMSON).bg(BG).add_modifier(Modifier::BOLD)
}

/// Ember action (assistant / secondary emphasis).
pub fn ember() -> Style {
    Style::default().fg(EMBER).bg(BG).add_modifier(Modifier::BOLD)
}

/// Success / running.
pub fn ok() -> Style {
    Style::default().fg(GREEN).bg(BG)
}

/// Warning.
pub fn warn() -> Style {
    Style::default().fg(AMBER).bg(BG)
}

/// Suspended / special state.
pub fn special() -> Style {
    Style::default().fg(VIOLET).bg(BG)
}

/// Selection bar — ember-tinted highlight, matching the prototype's active row.
pub fn selection() -> Style {
    Style::default()
        .fg(TEXT)
        .bg(RAISED)
        .add_modifier(Modifier::BOLD)
}

/// A state pill: glyph + label on a tinted background.
pub fn pill(fg: Color, bg: Color) -> Style {
    Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The palette must match the prototype / desktop Slint tokens exactly.
    #[test]
    fn palette_matches_prototype_tokens() {
        assert_eq!(BG, Color::Rgb(0x0A, 0x0B, 0x0D));
        assert_eq!(PANEL, Color::Rgb(0x0F, 0x11, 0x14));
        assert_eq!(SURFACE, Color::Rgb(0x14, 0x17, 0x1B));
        assert_eq!(RAISED, Color::Rgb(0x1A, 0x1E, 0x23));
        assert_eq!(LINE, Color::Rgb(0x1E, 0x22, 0x28));
        assert_eq!(TEXT, Color::Rgb(0xEC, 0xEE, 0xF1));
        assert_eq!(SUB, Color::Rgb(0xB4, 0xBA, 0xC4));
        assert_eq!(MUTED, Color::Rgb(0x9A, 0xA1, 0xAC));
        assert_eq!(CRIMSON, Color::Rgb(0xDC, 0x2F, 0x45));
        assert_eq!(EMBER, Color::Rgb(0xFF, 0x7A, 0x3D));
        assert_eq!(GREEN, Color::Rgb(0x34, 0xD3, 0x99));
        assert_eq!(AMBER, Color::Rgb(0xF5, 0xB5, 0x3D));
        assert_eq!(VIOLET, Color::Rgb(0xB7, 0x9C, 0xFF));
    }

    /// Focus is ember + bold; action is crimson + bold.
    #[test]
    fn focus_and_action_styles() {
        assert_eq!(focus().fg, Some(EMBER));
        assert_eq!(action().fg, Some(CRIMSON));
    }
}
