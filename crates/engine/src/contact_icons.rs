//! Contact icons: what the owner marks a contact or group with in place of
//! its first letter. Our own thin line drawings on a 24×24 grid, shared by
//! the desktop and Android apps; the terminal shows two ASCII characters.
//! The choice stays on this device: the contact never learns it.

use std::f32::consts::PI;

/// The grid every drawing is on.
pub const GRID: f32 = 24.0;
/// Line width on that grid; caps and joins are round.
pub const STROKE: f32 = 1.75;
/// Radius of a filled dot on that grid.
pub const DOT: f32 = 1.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactIcon {
    Person,
    Agent,
    Branch,
    Relay,
    Notes,
    Terminal,
    Bot,
    Pulse,
    Cloud,
    Home,
    Heart,
    Work,
    Star,
    Friends,
    Key,
}

/// All of them, in the order the picker shows them.
pub const ALL: [ContactIcon; 15] = [
    ContactIcon::Person,
    ContactIcon::Agent,
    ContactIcon::Branch,
    ContactIcon::Relay,
    ContactIcon::Notes,
    ContactIcon::Terminal,
    ContactIcon::Bot,
    ContactIcon::Pulse,
    ContactIcon::Cloud,
    ContactIcon::Home,
    ContactIcon::Heart,
    ContactIcon::Work,
    ContactIcon::Star,
    ContactIcon::Friends,
    ContactIcon::Key,
];

/// A drawing: polylines to stroke and dots to fill, on the `GRID`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drawing {
    pub lines: Vec<Vec<(f32, f32)>>,
    pub dots: Vec<(f32, f32)>,
}

enum Shape {
    Line(&'static [(f32, f32)]),
    Circle(f32, f32, f32),
    /// Centre, radius, from and to in degrees, clockwise from the right.
    Arc(f32, f32, f32, f32, f32),
    /// Left, top, width, height, corner radius.
    Rect(f32, f32, f32, f32, f32),
    Dot(f32, f32),
    Star,
}

use Shape::*;

impl ContactIcon {
    /// The name kept in the state; never change one.
    pub fn name(self) -> &'static str {
        match self {
            ContactIcon::Person => "person",
            ContactIcon::Agent => "agent",
            ContactIcon::Branch => "branch",
            ContactIcon::Relay => "relay",
            ContactIcon::Notes => "notes",
            ContactIcon::Terminal => "terminal",
            ContactIcon::Bot => "bot",
            ContactIcon::Pulse => "pulse",
            ContactIcon::Cloud => "cloud",
            ContactIcon::Home => "home",
            ContactIcon::Heart => "heart",
            ContactIcon::Work => "work",
            ContactIcon::Star => "star",
            ContactIcon::Friends => "friends",
            ContactIcon::Key => "key",
        }
    }

    /// `None` for a name this version does not know: the letter is shown.
    pub fn from_name(name: &str) -> Option<Self> {
        ALL.into_iter().find(|i| i.name() == name)
    }

    /// The one after `now` in the picker's order; after the last, `None`:
    /// the letter again.
    pub fn after(now: Option<&str>) -> Option<Self> {
        match now.and_then(Self::from_name) {
            None => Some(ALL[0]),
            Some(i) => ALL.iter().skip_while(|&&x| x != i).nth(1).copied(),
        }
    }

    /// The terminal's two characters.
    pub fn ascii(self) -> &'static str {
        match self {
            ContactIcon::Person => "@ ",
            ContactIcon::Agent => "<>",
            ContactIcon::Branch => "Y ",
            ContactIcon::Relay => "==",
            ContactIcon::Notes => "# ",
            ContactIcon::Terminal => ">_",
            ContactIcon::Bot => "[]",
            ContactIcon::Pulse => "^v",
            ContactIcon::Cloud => "()",
            ContactIcon::Home => "/\\",
            ContactIcon::Heart => "<3",
            ContactIcon::Work => "[=",
            ContactIcon::Star => "* ",
            ContactIcon::Friends => "@@",
            ContactIcon::Key => "o-",
        }
    }

    fn shapes(self) -> &'static [Shape] {
        match self {
            ContactIcon::Person => &[Circle(12.0, 8.0, 4.0), Arc(12.0, 21.0, 8.0, 180.0, 360.0)],
            ContactIcon::Agent => &[
                Line(&[(8.0, 7.0), (3.0, 12.0), (8.0, 17.0)]),
                Line(&[(16.0, 7.0), (21.0, 12.0), (16.0, 17.0)]),
                Line(&[(14.0, 4.0), (10.0, 20.0)]),
            ],
            ContactIcon::Branch => &[
                Line(&[(6.0, 3.0), (6.0, 15.0)]),
                Circle(6.0, 18.0, 3.0),
                Circle(18.0, 6.0, 3.0),
                Arc(9.0, 9.0, 9.0, 0.0, 90.0),
            ],
            ContactIcon::Relay => &[
                Rect(3.0, 3.0, 18.0, 7.0, 2.0),
                Rect(3.0, 14.0, 18.0, 7.0, 2.0),
                Dot(7.0, 6.5),
                Dot(7.0, 17.5),
            ],
            ContactIcon::Notes => &[
                Rect(5.0, 3.0, 14.0, 18.0, 2.0),
                Line(&[(9.0, 8.0), (15.0, 8.0)]),
                Line(&[(9.0, 12.0), (15.0, 12.0)]),
                Line(&[(9.0, 16.0), (12.5, 16.0)]),
            ],
            ContactIcon::Terminal => &[
                Rect(3.0, 4.0, 18.0, 16.0, 2.0),
                Line(&[(7.0, 9.0), (10.0, 12.0), (7.0, 15.0)]),
                Line(&[(12.5, 15.0), (17.0, 15.0)]),
            ],
            ContactIcon::Bot => &[
                Rect(4.0, 8.0, 16.0, 12.0, 3.0),
                Line(&[(12.0, 5.0), (12.0, 8.0)]),
                Dot(12.0, 4.0),
                Line(&[(9.0, 12.5), (9.0, 14.5)]),
                Line(&[(15.0, 12.5), (15.0, 14.5)]),
            ],
            ContactIcon::Pulse => &[Line(&[
                (2.0, 12.0),
                (6.0, 12.0),
                (9.0, 5.0),
                (15.0, 19.0),
                (18.0, 12.0),
                (22.0, 12.0),
            ])],
            ContactIcon::Cloud => &[
                Arc(7.5, 15.0, 4.0, 90.0, 270.0),
                Arc(12.5, 11.0, 5.0, 180.0, 360.0),
                Arc(17.0, 15.0, 4.0, 270.0, 450.0),
                Line(&[(17.0, 19.0), (7.5, 19.0)]),
            ],
            ContactIcon::Home => &[
                Line(&[(3.0, 11.0), (12.0, 3.5), (21.0, 11.0)]),
                Line(&[(5.5, 9.0), (5.5, 20.0), (18.5, 20.0), (18.5, 9.0)]),
                Line(&[(10.0, 20.0), (10.0, 14.5), (14.0, 14.5), (14.0, 20.0)]),
            ],
            ContactIcon::Heart => &[
                Arc(8.0, 9.0, 4.0, 135.0, 360.0),
                Arc(16.0, 9.0, 4.0, 180.0, 405.0),
                Line(&[(5.17, 11.83), (12.0, 19.0), (18.83, 11.83)]),
            ],
            ContactIcon::Work => &[
                Rect(3.0, 7.0, 18.0, 13.0, 2.0),
                Line(&[(9.0, 7.0), (9.0, 4.5), (15.0, 4.5), (15.0, 7.0)]),
                Line(&[(3.0, 12.5), (21.0, 12.5)]),
            ],
            ContactIcon::Star => &[Star],
            ContactIcon::Friends => &[
                Circle(9.0, 8.0, 3.5),
                Arc(9.0, 20.5, 6.5, 180.0, 360.0),
                Circle(17.0, 8.5, 3.0),
                Arc(17.0, 20.5, 5.0, 230.0, 360.0),
            ],
            ContactIcon::Key => &[
                Circle(8.0, 15.5, 4.5),
                Line(&[(11.2, 12.3), (20.0, 3.5)]),
                Line(&[(17.2, 6.3), (19.7, 8.8)]),
                Line(&[(14.6, 8.9), (16.6, 10.9)]),
            ],
        }
    }
}

/// The icon as lines and dots: curves become short straight steps.
pub fn drawing(icon: ContactIcon) -> Drawing {
    let mut d = Drawing::default();
    for shape in icon.shapes() {
        match *shape {
            Line(points) => d.lines.push(points.to_vec()),
            Circle(x, y, r) => d.lines.push(arc(x, y, r, 0.0, 360.0)),
            Arc(x, y, r, from, to) => d.lines.push(arc(x, y, r, from, to)),
            Rect(x, y, w, h, r) => {
                let mut line = arc(x + w - r, y + r, r, 270.0, 360.0);
                line.extend(arc(x + w - r, y + h - r, r, 0.0, 90.0));
                line.extend(arc(x + r, y + h - r, r, 90.0, 180.0));
                line.extend(arc(x + r, y + r, r, 180.0, 270.0));
                line.push(line[0]);
                d.lines.push(line);
            }
            Dot(x, y) => d.dots.push((x, y)),
            Star => {
                let mut line: Vec<_> = (0..10)
                    .map(|i| {
                        let r = if i % 2 == 0 { 9.2 } else { 3.9 };
                        let a = (-90.0 + 36.0 * i as f32) * PI / 180.0;
                        (12.0 + r * a.cos(), 12.8 + r * a.sin())
                    })
                    .collect();
                line.push(line[0]);
                d.lines.push(line);
            }
        }
    }
    d
}

/// Points along an arc, a step at most every 10 degrees.
fn arc(x: f32, y: f32, r: f32, from: f32, to: f32) -> Vec<(f32, f32)> {
    let steps = ((to - from) / 10.0).ceil().max(1.0) as usize;
    (0..=steps)
        .map(|i| {
            let a = (from + (to - from) * i as f32 / steps as f32) * PI / 180.0;
            (x + r * a.cos(), y + r * a.sin())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_draws_inside_the_grid_with_a_unique_name() {
        let mut names = std::collections::HashSet::new();
        let mut ascii = std::collections::HashSet::new();
        let margin = STROKE / 2.0;
        for icon in ALL {
            let d = drawing(icon);
            assert!(!d.lines.is_empty(), "{}", icon.name());
            assert!(d.lines.iter().all(|l| l.len() >= 2), "{}", icon.name());
            let inside = |&(x, y): &(f32, f32)| {
                (margin..=GRID - margin).contains(&x) && (margin..=GRID - margin).contains(&y)
            };
            assert!(d.lines.iter().flatten().all(inside), "{}", icon.name());
            assert!(d.dots.iter().all(inside), "{}", icon.name());
            assert!(names.insert(icon.name()), "{}", icon.name());
            assert_eq!(icon.ascii().len(), 2, "{}", icon.name());
            assert!(icon.ascii().is_ascii(), "{}", icon.name());
            assert!(ascii.insert(icon.ascii()), "{}", icon.name());
            assert_eq!(ContactIcon::from_name(icon.name()), Some(icon));
        }
        assert_eq!(ContactIcon::from_name("shield"), None);
    }

    #[test]
    fn after_walks_every_icon_then_the_letter() {
        let mut now: Option<ContactIcon> = None;
        let mut seen = Vec::new();
        loop {
            now = ContactIcon::after(now.map(ContactIcon::name));
            match now {
                Some(i) => seen.push(i),
                None => break,
            }
        }
        assert_eq!(seen, ALL);
        assert_eq!(ContactIcon::after(Some("gone")), Some(ALL[0]));
    }

    #[test]
    fn a_closed_shape_ends_where_it_starts() {
        let d = drawing(ContactIcon::Relay);
        assert_eq!(d.lines[0].first(), d.lines[0].last());
        let d = drawing(ContactIcon::Star);
        assert_eq!(d.lines[0].first(), d.lines[0].last());
    }
}
