//! The apps' icons: fine pixel line art beside the reaction sprites (ADR-025),
//! drawn by the desktop and Android apps in a colour of the theme, so no
//! stock emoji or system glyph stands in for one. The terminal keeps text.

/// Every icon the apps draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Pin,
    Document,
    Warning,
    Check,
    Edit,
    Reply,
    Close,
    Menu,
    Back,
    Plus,
    Dot,
    Timer,
    /// A folded list section.
    Folded,
    /// An open list section.
    Unfolded,
}

/// All of them, in the order `ICONS` lists their names.
pub const ALL: [Icon; 14] = [
    Icon::Pin,
    Icon::Document,
    Icon::Warning,
    Icon::Check,
    Icon::Edit,
    Icon::Reply,
    Icon::Close,
    Icon::Menu,
    Icon::Back,
    Icon::Plus,
    Icon::Dot,
    Icon::Timer,
    Icon::Folded,
    Icon::Unfolded,
];

/// The side of the square every icon is centred in.
pub const SIDE: u8 = 11;

impl Icon {
    pub fn name(self) -> &'static str {
        match self {
            Icon::Pin => "pin",
            Icon::Document => "document",
            Icon::Warning => "warning",
            Icon::Check => "check",
            Icon::Edit => "edit",
            Icon::Reply => "reply",
            Icon::Close => "close",
            Icon::Menu => "menu",
            Icon::Back => "back",
            Icon::Plus => "plus",
            Icon::Dot => "dot",
            Icon::Timer => "timer",
            Icon::Folded => "folded",
            Icon::Unfolded => "unfolded",
        }
    }

    /// `1` is a lit pixel: one-pixel lines, at most `SIDE`×`SIDE`.
    fn rows(self) -> &'static [&'static str] {
        match self {
            Icon::Pin => &[
                "...11111...", //
                "....1.1....", //
                "....1.1....", //
                "....1.1....", //
                "...1...1...", //
                "..1111111..", //
                ".....1.....", //
                ".....1.....", //
                ".....1.....", //
            ],
            Icon::Document => &[
                "1111111....", //
                "1.....11...", //
                "1.....1.1..", //
                "1.....1111.", //
                "1........1.", //
                "1.11111..1.", //
                "1........1.", //
                "1.11111..1.", //
                "1........1.", //
                "1111111111.", //
            ],
            Icon::Warning => &[
                ".....1.....", //
                "....1.1....", //
                "....1.1....", //
                "...1...1...", //
                "...1.1.1...", //
                "..1..1..1..", //
                "..1..1..1..", //
                ".1.......1.", //
                ".1...1...1.", //
                "11111111111", //
            ],
            Icon::Check => &[
                "..........1", //
                ".........1.", //
                "........1..", //
                "1......1...", //
                ".1....1....", //
                "..1..1.....", //
                "...11......", //
            ],
            Icon::Edit => &[
                "........11.", //
                ".......1..1", //
                "......1..1.", //
                ".....1..1..", //
                "....1..1...", //
                "...1..1....", //
                "..1..1.....", //
                ".1..1......", //
                ".111.......", //
                "1..........", //
            ],
            Icon::Reply => &[
                "1..........", //
                "1..........", //
                "1..........", //
                "1.......1..", //
                "1........1.", //
                "11111111111", //
                ".........1.", //
                "........1..", //
            ],
            Icon::Close => &[
                "1.......1", //
                ".1.....1.", //
                "..1...1..", //
                "...1.1...", //
                "....1....", //
                "...1.1...", //
                "..1...1..", //
                ".1.....1.", //
                "1.......1", //
            ],
            Icon::Menu => &[
                "11", //
                "11", //
                "..", //
                "..", //
                "11", //
                "11", //
                "..", //
                "..", //
                "11", //
                "11", //
            ],
            Icon::Back => &[
                "....1", //
                "...1.", //
                "..1..", //
                ".1...", //
                "1....", //
                ".1...", //
                "..1..", //
                "...1.", //
                "....1", //
            ],
            Icon::Plus => &[
                "....1....", //
                "....1....", //
                "....1....", //
                "....1....", //
                "111111111", //
                "....1....", //
                "....1....", //
                "....1....", //
                "....1....", //
            ],
            Icon::Dot => &[
                ".111.", //
                "11111", //
                "11111", //
                "11111", //
                ".111.", //
            ],
            Icon::Timer => &[
                "...111...", //
                "....1....", //
                "..11111..", //
                ".1.....1.", //
                "1...1...1", //
                "1...1...1", //
                "1...11..1", //
                "1.......1", //
                ".1.....1.", //
                "..11111..", //
            ],
            Icon::Folded => &[
                "1...", //
                ".1..", //
                "..1.", //
                "...1", //
                "..1.", //
                ".1..", //
                "1...", //
            ],
            Icon::Unfolded => &[
                "1.....1", //
                ".1...1.", //
                "..1.1..", //
                "...1...", //
            ],
        }
    }
}

/// Lit pixels of `icon` as (column, row), centred in a `SIDE`×`SIDE` square.
pub fn pixels(icon: Icon) -> impl Iterator<Item = (u8, u8)> + Clone {
    lit(icon.rows(), SIDE)
}

/// Lit pixels of a sprite as (column, row), centred in a `side`×`side` square.
pub(crate) fn lit(
    rows: &'static [&'static str],
    side: u8,
) -> impl Iterator<Item = (u8, u8)> + Clone {
    let side = usize::from(side);
    let top = side.saturating_sub(rows.len()) / 2;
    rows.iter().enumerate().flat_map(move |(y, row)| {
        let left = side.saturating_sub(row.len()) / 2;
        row.bytes()
            .enumerate()
            .filter(|&(_, b)| b == b'1')
            .map(move |(x, _)| ((left + x) as u8, (top + y) as u8))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_fits_has_pixels_and_a_unique_name() {
        let mut names = std::collections::HashSet::new();
        for icon in ALL {
            let rows = icon.rows();
            assert!(rows.len() <= usize::from(SIDE), "{}", icon.name());
            assert!(
                rows.iter().all(|r| r.len() <= usize::from(SIDE)),
                "{}",
                icon.name()
            );
            assert!(
                rows.iter()
                    .all(|r| r.bytes().all(|b| b == b'1' || b == b'.'))
            );
            assert!(pixels(icon).count() > 0, "{}", icon.name());
            assert!(pixels(icon).all(|(x, y)| x < SIDE && y < SIDE));
            assert!(names.insert(icon.name()), "{}", icon.name());
        }
    }
}
