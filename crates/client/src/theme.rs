//! Colour themes. The values are the public palettes of each theme, as
//! collected by herdr (Apache-2.0, github.com/herdrdev/herdr).
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

/// The first one is the default.
#[rustfmt::skip]
pub const THEMES: &[(&str, Palette)] = &[
    ("catppuccin", hex([0x89b4fa, 0x181825, 0x1e1e2e, 0x313244, 0x313244, 0x45475a, 0x1e1e2e, 0x6c7086, 0x7f849c, 0xcdd6f4, 0xa6adc8, 0xcba6f7, 0xa6e3a1, 0xf9e2af, 0xf38ba8, 0x94e2d5, 0xfab387])),
    ("catppuccin-latte", hex([0x1e66f5, 0xeff1f5, 0xe6e9ef, 0xbdd0f5, 0xccd0da, 0xbcc0cc, 0xe6e9ef, 0x9ca0b0, 0x8c8fa1, 0x4c4f69, 0x6c6f85, 0x8839ef, 0x40a02b, 0xdf8e1d, 0xd20f39, 0x179299, 0xfe640b])),
    ("terminal", TERMINAL),
    ("tokyo-night", hex([0x7aa2f7, 0x1a1b26, 0x232636, 0x2d3650, 0x24283b, 0x414868, 0x1a1b26, 0x565f89, 0x697196, 0xc0caf5, 0xa9b1d6, 0xbb9af7, 0x9ece6a, 0xe0af68, 0xf7768e, 0x7dcfff, 0xff9e64])),
    ("tokyo-night-day", hex([0x2e7de9, 0xe1e2e7, 0xd2d3da, 0xb6cae7, 0xc4c8da, 0xa8aecb, 0xd2d3da, 0x8990b3, 0x68709a, 0x3760bf, 0x6172b0, 0x7847bd, 0x587539, 0x8c6c3e, 0xf52a65, 0x118c74, 0xb15c00])),
    ("dracula", hex([0xbd93f9, 0x282a36, 0x373c52, 0x463f5d, 0x44475a, 0x6272a4, 0x282a36, 0x6272a4, 0x828cb4, 0xf8f8f2, 0xd2d2dc, 0xff79c6, 0x50fa7b, 0xf1fa8c, 0xff5555, 0x8be9fd, 0xffb86c])),
    ("nord", hex([0x88c0d0, 0x2e3440, 0x434c5e, 0x40505d, 0x3b4252, 0x434c5e, 0x2e3440, 0x4c566a, 0x646e82, 0xeceff4, 0xd8dee9, 0xb48ead, 0xa3be8c, 0xebcb8b, 0xbf616a, 0x8fbcbb, 0xd08770])),
    ("gruvbox", hex([0xd79921, 0x282828, 0x323130, 0x4b3f27, 0x3c3836, 0x504945, 0x282828, 0x928374, 0xa89984, 0xebdbb2, 0xd5c4a1, 0xd3869b, 0xb8bb26, 0xfabd2f, 0xfb4934, 0x8ec07c, 0xfe8019])),
    ("gruvbox-light", hex([0x076678, 0xfbf1c7, 0xf2e5bc, 0xebdbb2, 0xebdbb2, 0xd5c4a1, 0xf2e5bc, 0x928374, 0x7c6f64, 0x3c3836, 0x504945, 0x8f3f71, 0x79740e, 0xb57614, 0x9d0006, 0x427b58, 0xaf3a03])),
    ("one-dark", hex([0x61afef, 0x282c34, 0x313640, 0x334659, 0x2c313a, 0x3e4451, 0x282c34, 0x5c6370, 0x737a87, 0xabb2bf, 0x969ca8, 0xc678dd, 0x98c379, 0xe5c07b, 0xe06c75, 0x56b6c2, 0xd19a66])),
    ("one-light", hex([0x4078f2, 0xfafafa, 0xd8dbe2, 0xcddbf8, 0xf0f0f1, 0xe5e5e6, 0xf5f5f6, 0xa0a1a7, 0x686b77, 0x383a42, 0x686b77, 0xa626a4, 0x50a14f, 0xc18401, 0xe45649, 0x0184bc, 0x986801])),
    ("solarized", hex([0x268bd2, 0x002b36, 0x164b57, 0x083e55, 0x073642, 0x586e75, 0x002b36, 0x586e75, 0x657b83, 0x93a1a1, 0x839496, 0xd33682, 0x859900, 0xb58900, 0xdc322f, 0x2aa198, 0xcb4b16])),
    ("solarized-light", hex([0x268bd2, 0xfdf6e3, 0xeee8d5, 0xc9dcdf, 0xeee8d5, 0x93a1a1, 0xeee8d5, 0x93a1a1, 0x586e75, 0x657b83, 0x839496, 0xd33682, 0x859900, 0xb58900, 0xdc322f, 0x2aa198, 0xcb4b16])),
    ("kanagawa", hex([0x7e9cd8, 0x1f1f28, 0x363646, 0x32384b, 0x2a2a37, 0x363646, 0x1f1f28, 0x727169, 0x87867d, 0xdcd7ba, 0xc8c3aa, 0x957fb8, 0x76946a, 0xc0a36e, 0xc34043, 0x7fb4ca, 0xffa066])),
    ("kanagawa-lotus", hex([0x4d699b, 0xf2ecbc, 0xd5cea3, 0xdcd5ac, 0xdcd5ac, 0xc9cbd1, 0xd5cea3, 0xa09cac, 0x8a8980, 0x545464, 0x43436c, 0x624c83, 0x6f894e, 0x77713f, 0xc84053, 0x4e8ca2, 0xcc6d00])),
    ("rose-pine", hex([0xc4a7e7, 0x191724, 0x26233a, 0x3b344b, 0x1f1d2e, 0x26233a, 0x26233a, 0x6e6a86, 0x908caa, 0xe0def4, 0xc8c5dc, 0xc4a7e7, 0x31748f, 0xf6c177, 0xeb6f92, 0x9ccfd8, 0xea9a97])),
    ("rose-pine-dawn", hex([0x907aa9, 0xfaf4ed, 0xe3d9cf, 0xf2e9e1, 0xf2e9e1, 0xfffaf3, 0xf2e9e1, 0x9893a5, 0x797593, 0x464261, 0x797593, 0x907aa9, 0x286983, 0xea9d34, 0xb4637a, 0x56949f, 0xd7827e])),
    ("vesper", hex([0xffc799, 0x1a1a1a, 0x101010, 0x232323, 0x232323, 0x282828, 0x101010, 0x5c5c5c, 0x7e7e7e, 0xffffff, 0xa0a0a0, 0xffd1a8, 0x99ffe4, 0xffc799, 0xff8080, 0x66ddcc, 0xffc799])),
];

/// Unknown names fall back to the default.
pub fn by_name(name: &str) -> &'static Palette {
    THEMES
        .iter()
        .find(|(n, _)| *n == name)
        .map_or(&THEMES[0].1, |(_, p)| p)
}

/// The theme after `name`, for cycling through them.
pub fn next(name: &str) -> &'static str {
    let i = THEMES
        .iter()
        .position(|(n, _)| *n == name)
        .map_or(0, |i| i + 1);
    THEMES[i % THEMES.len()].0
}
