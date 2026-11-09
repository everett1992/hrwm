//! Touchscreen gestures for `gesturebind`, an hrwm extension (herbstluftwm
//! has no gestures). river-touch-gestures-v1 recognises them; hrwm only
//! names them and sets their thresholds.

use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// `N-swipe-DIR`, `N-pinch-in`, `N-pinch-out` or `edge-DIR`, where an edge
/// gesture is one finger swiping in from that edge of the touchscreen.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Gesture {
    Swipe { fingers: u32, dir: Dir },
    Pinch { fingers: u32, out: bool },
    Edge(Dir),
}

/// Centroid travel before a swipe or edge gesture starts, in pixels.
pub const SWIPE_DISTANCE: i32 = 100;
/// How close to the edge an edge gesture must start, in pixels.
pub const EDGE_DISTANCE: i32 = 30;
/// Pinch scale below which `pinch-in` (and above which `pinch-out`) starts.
pub const PINCH_IN: f64 = 0.8;
pub const PINCH_OUT: f64 = 1.25;

impl Dir {
    fn parse(s: &str) -> Option<Dir> {
        Some(match s {
            "up" => Dir::Up,
            "down" => Dir::Down,
            "left" => Dir::Left,
            "right" => Dir::Right,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Dir::Up => "up",
            Dir::Down => "down",
            Dir::Left => "left",
            Dir::Right => "right",
        }
    }

    /// Edges are named top/bottom/left/right; Up is the top edge.
    fn parse_edge(s: &str) -> Option<Dir> {
        Some(match s {
            "top" => Dir::Up,
            "bottom" => Dir::Down,
            "left" => Dir::Left,
            "right" => Dir::Right,
            _ => return None,
        })
    }

    fn edge_name(self) -> &'static str {
        match self {
            Dir::Up => "top",
            Dir::Down => "bottom",
            Dir::Left => "left",
            Dir::Right => "right",
        }
    }

    /// The direction a finger moves when swiping in from this edge.
    pub fn inward(self) -> Dir {
        match self {
            Dir::Up => Dir::Down,
            Dir::Down => Dir::Up,
            Dir::Left => Dir::Right,
            Dir::Right => Dir::Left,
        }
    }
}

impl Gesture {
    pub fn parse(s: &str) -> Result<Gesture, String> {
        let bad = || format!("unknown gesture \"{s}\"");
        let parts: Vec<&str> = s.split('-').collect();
        match parts.as_slice() {
            ["edge", d] => Ok(Gesture::Edge(Dir::parse_edge(d).ok_or_else(bad)?)),
            [n, kind, arg] => {
                let fingers: u32 = n.parse().ok().filter(|&n| n > 0).ok_or_else(bad)?;
                match *kind {
                    "swipe" => Ok(Gesture::Swipe {
                        fingers,
                        dir: Dir::parse(arg).ok_or_else(bad)?,
                    }),
                    "pinch" if fingers < 2 => Err(format!("{s}: a pinch needs 2 or more fingers")),
                    "pinch" => match *arg {
                        "in" => Ok(Gesture::Pinch {
                            fingers,
                            out: false,
                        }),
                        "out" => Ok(Gesture::Pinch { fingers, out: true }),
                        _ => Err(bad()),
                    },
                    _ => Err(bad()),
                }
            }
            _ => Err(bad()),
        }
    }

    pub fn fingers(self) -> u32 {
        match self {
            Gesture::Swipe { fingers, .. } | Gesture::Pinch { fingers, .. } => fingers,
            Gesture::Edge(_) => 1,
        }
    }
}

impl fmt::Display for Gesture {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Gesture::Swipe { fingers, dir } => write!(f, "{fingers}-swipe-{}", dir.name()),
            Gesture::Pinch { fingers, out } => {
                write!(f, "{fingers}-pinch-{}", if *out { "out" } else { "in" })
            }
            Gesture::Edge(d) => write!(f, "edge-{}", d.edge_name()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips() {
        for s in [
            "3-swipe-left",
            "1-swipe-up",
            "2-pinch-in",
            "4-pinch-out",
            "edge-bottom",
        ] {
            assert_eq!(Gesture::parse(s).unwrap().to_string(), s);
        }
    }

    #[test]
    fn parse_rejects() {
        for s in [
            "",
            "swipe-left",
            "0-swipe-left",
            "3-swipe-sideways",
            "1-pinch-in",
            "2-pinch-up",
            "edge-middle",
            "edge-up",
            "3-twist-left",
        ] {
            assert!(Gesture::parse(s).is_err(), "{s}");
        }
    }

    #[test]
    fn edge_fingers_and_inward() {
        let g = Gesture::parse("edge-left").unwrap();
        assert_eq!(g.fingers(), 1);
        assert_eq!(Dir::Left.inward(), Dir::Right);
    }
}
