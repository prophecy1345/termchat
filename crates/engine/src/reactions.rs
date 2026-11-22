//! How the apps draw the seven reactions (ADR-025): 1980s arcade sprites,
//! the same on every device, in a colour of the theme. The wire carries only
//! the index (`proto::REACTIONS`).

/// One sprite per reaction, by index; `1` is a lit pixel. At most 11×11.
pub const SPRITES: [&[&str]; 7] = [
    // like
    &[
        "....1....",
        "...111...",
        "..11111..",
        ".1111111.",
        "111111111",
        "...111...",
        "...111...",
        "...111...",
        "...111...",
    ],
    // love
    &[
        ".11...11.",
        "1111.1111",
        "111111111",
        "111111111",
        ".1111111.",
        "..11111..",
        "...111...",
        "....1....",
    ],
    // laugh
    &[
        "...11111...",
        ".11.....11.",
        ".1.......1.",
        "1..1...1..1",
        "1.1.1.1.1.1",
        "1.........1",
        "1.1111111.1",
        "1..11111..1",
        ".1..111..1.",
        ".11.....11.",
        "...11111...",
    ],
    // wow
    &[
        "...11111...",
        ".11.....11.",
        ".1.......1.",
        "1..1...1..1",
        "1..1...1..1",
        "1.........1",
        "1...111...1",
        "1...1.1...1",
        ".1..111..1.",
        ".11.....11.",
        "...11111...",
    ],
    // sad
    &[
        "...11111...",
        ".11.....11.",
        ".1.......1.",
        "1..1...1..1",
        "1..1...1..1",
        "1.......1.1",
        "1.......1.1",
        "1...111...1",
        ".1.1...1.1.",
        ".11.....11.",
        "...11111...",
    ],
    // thanks
    &[
        "...1.1...",
        "..11.11..",
        "..11.11..",
        "..11.11..",
        ".111.111.",
        ".111.111.",
        "1111.1111",
        "11.....11",
    ],
    // dislike
    &[
        "...111...",
        "...111...",
        "...111...",
        "...111...",
        "111111111",
        ".1111111.",
        "..11111..",
        "...111...",
        "....1....",
    ],
];

/// The theme colour of each, as an index into `themes::SLOTS`: teal, mauve,
/// yellow, accent, overlay1, peach, red.
pub const COLOURS: [usize; 7] = [15, 11, 13, 0, 8, 16, 14];

/// Lit pixels of reaction `r` as (column, row), centred in an 11×11 square.
pub fn pixels(r: u8) -> impl Iterator<Item = (u8, u8)> + Clone {
    crate::icons::lit(SPRITES[usize::from(r).min(SPRITES.len() - 1)], 11)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_sprite_per_reaction_and_each_fits() {
        assert_eq!(SPRITES.len(), usize::from(termchat_core::proto::REACTIONS));
        for (r, rows) in SPRITES.iter().enumerate() {
            assert!(rows.len() <= 11 && rows.iter().all(|row| row.len() <= 11));
            assert!(pixels(r as u8).all(|(x, y)| x < 11 && y < 11));
            assert!(pixels(r as u8).count() > 10);
        }
    }
}
