//! Colour themes: the shared list (`termchat_engine::themes`) as ratatui
//! palettes, plus the terminal's own 16 colours.
//!
//! Like herdr, the main area keeps the terminal's own background; a theme
//! paints only bars, menus and windows (ADR-008).

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub accent: Color,
    /// Bars, menus, windows.
    pub panel_bg: Color,
    /// The open chat in the contact list.
    pub active_row_bg: Color,
    pub selection_bg: Color,
    /// Input fields, secondary buttons.
    pub surface0: Color,
    pub surface1: Color,
    /// Separators.
    pub surface_dim: Color,
    /// Muted text.
    pub overlay0: Color,
    pub overlay1: Color,
    pub text: Color,
    pub subtext0: Color,
    pub mauve: Color,
    pub green: Color,
    pub yellow: Color,
    pub red: Color,
    pub teal: Color,
    pub peach: Color,
}

const fn rgb(h: u32) -> Color {
    Color::Rgb((h >> 16) as u8, (h >> 8) as u8, h as u8)
}

/// In field order.
const fn hex(c: [u32; 17]) -> Palette {
    Palette {
        accent: rgb(c[0]),
        panel_bg: rgb(c[1]),
        active_row_bg: rgb(c[2]),
        selection_bg: rgb(c[3]),
        surface0: rgb(c[4]),
        surface1: rgb(c[5]),
        surface_dim: rgb(c[6]),
        overlay0: rgb(c[7]),
        overlay1: rgb(c[8]),
        text: rgb(c[9]),
        subtext0: rgb(c[10]),
        mauve: rgb(c[11]),
        green: rgb(c[12]),
        yellow: rgb(c[13]),
        red: rgb(c[14]),
        teal: rgb(c[15]),
        peach: rgb(c[16]),
    }
}

/// The terminal's own 16 colours, for terminals without true colour.
const TERMINAL: Palette = Palette {
    accent: Color::Blue,
    panel_bg: Color::Reset,
    active_row_bg: Color::DarkGray,
    selection_bg: Color::Reset,
    surface0: Color::Reset,
    surface1: Color::DarkGray,
    surface_dim: Color::DarkGray,
    overlay0: Color::Gray,
    overlay1: Color::White,
    text: Color::Reset,
    subtext0: Color::Gray,
    mauve: Color::Gray,
    green: Color::Green,
    yellow: Color::Yellow,
    red: Color::LightRed,
    teal: Color::Cyan,
    peach: Color::Yellow,
};

/// The shared themes, with the terminal's own colours third as before; the
/// first is the default.
pub static THEMES: std::sync::LazyLock<Vec<(&str, Palette)>> = std::sync::LazyLock::new(|| {
    let mut all: Vec<_> = termchat_engine::themes::THEMES
        .iter()
        .map(|(n, c)| (*n, hex(*c)))
        .collect();
    all.insert(2.min(all.len()), ("terminal", TERMINAL));
    all
});

/// Position in `THEMES`; unknown names are the default.
pub fn index(name: &str) -> usize {
    THEMES.iter().position(|(n, _)| *n == name).unwrap_or(0)
}

pub fn by_name(name: &str) -> &'static Palette {
    &THEMES[index(name)].1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_terminal_keep_their_places() {
        assert_eq!(THEMES[0].0, "catppuccin");
        assert_eq!(THEMES[2].0, "terminal");
        assert_eq!(index("no such theme"), 0);
    }
}
