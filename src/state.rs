//! Window manager state kept across restarts (`wmexec`, or hrwm killed and
//! started again): the tags, their frame trees and where every window is.
//! river keeps the windows when its window manager goes away and names each
//! with an identifier that is never reused, so the next hrwm finds them by
//! that identifier.
//!
//! The file sits next to the command socket and holds one tab-separated
//! record per line:
//!
//! ```text
//! hrwm-state 1
//! win      N IDENTIFIER FLOATING PSEUDOTILE FULLSCREEN FLOAT_RECT
//! tag      NAME FLOATING FLOAT_FOCUS FLOATS TREE
//! monitor  OUTPUT TAG
//! focus    OUTPUT
//! ```
//!
//! N numbers the windows within the file. TREE is `dump`'s syntax with those
//! numbers in place of window ids; FLOATS lists them comma-separated, bottom
//! to top. Flags are 0 or 1, absent values `-`.

use std::path::PathBuf;

use crate::tree::{Align, Frame, Layout, Leaf, Rect, Split, WinId};

const HEADER: &str = "hrwm-state 1";

pub struct SavedWin {
    pub identifier: String,
    pub floating: bool,
    pub pseudotile: bool,
    pub fullscreen: bool,
    pub float_rect: Option<Rect>,
}

pub struct SavedTag {
    pub name: String,
    pub floating: bool,
    pub float_focus: Option<WinId>,
    pub floats: Vec<WinId>,
    /// window ids are indexes into `Saved::wins`
    pub tree: Frame,
}

#[derive(Default)]
pub struct Saved {
    pub wins: Vec<SavedWin>,
    pub tags: Vec<SavedTag>,
    /// output name, tag name
    pub monitors: Vec<(String, String)>,
    pub focus: Option<String>,
}

pub fn path() -> PathBuf {
    crate::ipc::socket_path().with_extension("state")
}

/// The saved state, if there is a readable one.
pub fn load() -> Option<Saved> {
    let path = path();
    let text = std::fs::read_to_string(&path).ok()?;
    match Saved::parse(&text) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("hrwm: ignoring {}: {e}", path.display());
            None
        }
    }
}

/// Replace the state file, atomically so a crash never leaves half of one.
pub fn store(text: &str) {
    let path = path();
    let tmp = path.with_extension("state.tmp");
    if let Err(e) = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &path)) {
        eprintln!("hrwm: cannot save {}: {e}", path.display());
    }
}

fn flag(b: bool) -> &'static str {
    if b { "1" } else { "0" }
}

fn parse_flag(s: &str) -> Result<bool, String> {
    match s {
        "1" => Ok(true),
        "0" => Ok(false),
        _ => Err(format!("bad flag \"{s}\"")),
    }
}

fn parse_win(s: &str) -> Result<WinId, String> {
    s.parse().map_err(|_| format!("bad window number \"{s}\""))
}

impl Saved {
    pub fn to_text(&self) -> String {
        let mut out = format!("{HEADER}\n");
        for (n, w) in self.wins.iter().enumerate() {
            let rect = w
                .float_rect
                .map(|r| format!("{} {} {} {}", r.x, r.y, r.w, r.h))
                .unwrap_or_else(|| "-".into());
            out.push_str(&format!(
                "win\t{n}\t{}\t{}\t{}\t{}\t{rect}\n",
                w.identifier,
                flag(w.floating),
                flag(w.pseudotile),
                flag(w.fullscreen),
            ));
        }
        for t in &self.tags {
            let focus = t
                .float_focus
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".into());
            let floats = if t.floats.is_empty() {
                "-".into()
            } else {
                t.floats
                    .iter()
                    .map(|n| n.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let mut tree = String::new();
            write_frame(&t.tree, &mut tree);
            out.push_str(&format!(
                "tag\t{}\t{}\t{focus}\t{floats}\t{tree}\n",
                t.name,
                flag(t.floating)
            ));
        }
        for (output, tag) in &self.monitors {
            out.push_str(&format!("monitor\t{output}\t{tag}\n"));
        }
        if let Some(f) = &self.focus {
            out.push_str(&format!("focus\t{f}\n"));
        }
        out
    }

    pub fn parse(text: &str) -> Result<Saved, String> {
        let mut lines = text.lines();
        if lines.next() != Some(HEADER) {
            return Err("not an hrwm-state 1 file".into());
        }
        let mut s = Saved::default();
        for line in lines {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["win", n, ident, floating, pseudotile, fullscreen, rect] => {
                    if parse_win(n)? as usize != s.wins.len() {
                        return Err(format!("window {n} out of order"));
                    }
                    let float_rect = match *rect {
                        "-" => None,
                        r => {
                            let v: Vec<i32> = r
                                .split(' ')
                                .map(|x| x.parse().map_err(|_| format!("bad rect \"{r}\"")))
                                .collect::<Result<_, _>>()?;
                            let [x, y, w, h] = v[..] else {
                                return Err(format!("bad rect \"{r}\""));
                            };
                            Some(Rect::new(x, y, w, h))
                        }
                    };
                    s.wins.push(SavedWin {
                        identifier: ident.to_string(),
                        floating: parse_flag(floating)?,
                        pseudotile: parse_flag(pseudotile)?,
                        fullscreen: parse_flag(fullscreen)?,
                        float_rect,
                    });
                }
                ["tag", name, floating, focus, floats, tree] => {
                    let mut rest = *tree;
                    let tree = parse_frame(&mut rest)?;
                    if !rest.trim().is_empty() {
                        return Err(format!("trailing text after tree: \"{rest}\""));
                    }
                    s.tags.push(SavedTag {
                        name: name.to_string(),
                        floating: parse_flag(floating)?,
                        float_focus: match *focus {
                            "-" => None,
                            n => Some(parse_win(n)?),
                        },
                        floats: match *floats {
                            "-" => Vec::new(),
                            l => l.split(',').map(parse_win).collect::<Result<_, _>>()?,
                        },
                        tree,
                    });
                }
                ["monitor", output, tag] => s.monitors.push((output.to_string(), tag.to_string())),
                ["focus", output] => s.focus = Some(output.to_string()),
                _ => return Err(format!("bad line \"{line}\"")),
            }
        }
        Ok(s)
    }
}

fn write_frame(f: &Frame, out: &mut String) {
    match f {
        Frame::Leaf(l) => {
            out.push_str(&format!("(clients {}:{}", l.layout.name(), l.sel));
            for w in &l.wins {
                out.push_str(&format!(" {w}"));
            }
            out.push(')');
        }
        Frame::Split(s) => {
            let align = match s.align {
                Align::Vertical => "vertical",
                Align::Horizontal => "horizontal",
            };
            // {:?} prints the shortest text that reads back as the same f64
            out.push_str(&format!("(split {align}:{:?}:{} ", s.frac, s.sel));
            write_frame(&s.a, out);
            out.push(' ');
            write_frame(&s.b, out);
            out.push(')');
        }
    }
}

/// Parse one frame from the start of `s`, leaving the rest in `s`.
fn parse_frame(s: &mut &str) -> Result<Frame, String> {
    let bad = |s: &str| format!("bad tree at \"{s}\"");
    *s = s.trim_start();
    let body = s.strip_prefix('(').ok_or_else(|| bad(s))?;
    let (kind, mut rest) = body.split_once(' ').ok_or_else(|| bad(s))?;
    let frame = match kind {
        "clients" => {
            let end = rest.find(')').ok_or_else(|| bad(s))?;
            let mut words = rest[..end].split(' ');
            let (layout, sel) = words
                .next()
                .and_then(|w| w.split_once(':'))
                .ok_or_else(|| bad(s))?;
            let mut leaf = Leaf::new(Layout::parse(layout).ok_or_else(|| bad(s))?);
            leaf.sel = sel.parse().map_err(|_| bad(s))?;
            leaf.wins = words.map(parse_win).collect::<Result<_, _>>()?;
            rest = &rest[end + 1..];
            Frame::Leaf(leaf)
        }
        "split" => {
            let (params, after) = rest.split_once(' ').ok_or_else(|| bad(s))?;
            let p: Vec<&str> = params.split(':').collect();
            let [align, frac, sel] = p[..] else {
                return Err(bad(s));
            };
            let align = match align {
                "vertical" => Align::Vertical,
                "horizontal" => Align::Horizontal,
                _ => return Err(bad(s)),
            };
            rest = after;
            let a = parse_frame(&mut rest)?;
            let b = parse_frame(&mut rest)?;
            rest = rest
                .trim_start()
                .strip_prefix(')')
                .ok_or_else(|| bad(rest))?;
            Frame::Split(Split {
                align,
                frac: crate::tree::clamp_frac(frac.parse().map_err(|_| bad(s))?),
                sel: sel.parse().map_err(|_| bad(s))?,
                a: Box::new(a),
                b: Box::new(b),
            })
        }
        _ => return Err(bad(s)),
    };
    *s = rest;
    Ok(frame)
}

/// Swap every window id in a tree through `map`, dropping those it has no
/// id for (windows closed while no window manager ran).
pub fn remap(f: &mut Frame, map: &dyn Fn(WinId) -> Option<WinId>) {
    match f {
        Frame::Leaf(l) => {
            let sel = l.wins.get(l.sel).copied();
            l.wins = l.wins.iter().filter_map(|&w| map(w)).collect();
            l.sel = sel
                .and_then(map)
                .and_then(|w| l.wins.iter().position(|&x| x == w))
                .unwrap_or(0)
                .min(l.wins.len().saturating_sub(1));
        }
        Frame::Split(s) => {
            s.sel = s.sel.min(1);
            remap(&mut s.a, map);
            remap(&mut s.b, map);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "hrwm-state 1
win\t0\tabc123\t0\t0\t0\t-
win\t1\tdef456\t1\t0\t0\t10 20 300 200
win\t2\tghi789\t0\t1\t0\t-
tag\tz\t0\t-\t-\t(split horizontal:0.333333:1 (clients vertical:0 0) (clients max:1 2 0))
tag\tx\t1\t1\t1\t(clients auto:0)
monitor\teDP-1\tz
monitor\tDP-2\tx
focus\teDP-1
";

    #[test]
    fn round_trips() {
        let s = Saved::parse(SAMPLE).unwrap();
        assert_eq!(s.wins.len(), 3);
        assert_eq!(s.wins[1].float_rect, Some(Rect::new(10, 20, 300, 200)));
        assert_eq!(s.tags[1].floats, vec![1]);
        assert_eq!(s.tags[1].float_focus, Some(1));
        assert_eq!(s.focus.as_deref(), Some("eDP-1"));
        assert_eq!(s.to_text(), SAMPLE);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Saved::parse("").is_err());
        assert!(Saved::parse("hrwm-state 2\n").is_err());
        assert!(Saved::parse(&format!("{HEADER}\ntag\tz\t0\t-\t-\t(clients nope:0)\n")).is_err());
        assert!(Saved::parse(&format!("{HEADER}\ntag\tz\t0\t-\t-\t(clients max:0 1\n")).is_err());
        assert!(Saved::parse(&format!("{HEADER}\nwin\t1\ta\t0\t0\t0\t-\n")).is_err());
    }

    #[test]
    fn remap_drops_missing_windows() {
        let mut s = Saved::parse(SAMPLE).unwrap();
        // window 0 is gone; 2 becomes 7
        let map = |w: WinId| match w {
            2 => Some(7),
            1 => Some(8),
            _ => None,
        };
        remap(&mut s.tags[0].tree, &map);
        let mut text = String::new();
        write_frame(&s.tags[0].tree, &mut text);
        assert_eq!(
            text,
            "(split horizontal:0.333333:1 (clients vertical:0) (clients max:0 7))"
        );
    }
}
