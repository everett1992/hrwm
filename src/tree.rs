//! The frame tree of a tag. This is a port of herbstluftwm's FrameTree,
//! FrameLeaf and FrameSplit logic (layout.cpp, frametree.cpp,
//! layoutalgoimpl.cpp) without any Wayland specifics.
//!
//! A frame is addressed by a path of child indices (0 or 1) from the root.
//! The focused frame is found by following each split's `sel` field.

pub type WinId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

impl Dir {
    pub fn parse(s: &str) -> Option<Dir> {
        match s {
            "left" | "l" => Some(Dir::Left),
            "right" | "r" => Some(Dir::Right),
            "up" | "u" => Some(Dir::Up),
            "down" | "d" => Some(Dir::Down),
            _ => None,
        }
    }

    pub fn flip(self) -> Dir {
        match self {
            Dir::Left => Dir::Right,
            Dir::Right => Dir::Left,
            Dir::Up => Dir::Down,
            Dir::Down => Dir::Up,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Vertical,
    Horizontal,
    Max,
    Grid,
    /// hrwm extension: horizontal in a wide frame, vertical in a tall one
    Auto,
}

impl Layout {
    pub const ALL: [Layout; 4] = [
        Layout::Vertical,
        Layout::Horizontal,
        Layout::Max,
        Layout::Grid,
    ];

    pub fn parse(s: &str) -> Option<Layout> {
        match s {
            "vertical" => Some(Layout::Vertical),
            "horizontal" => Some(Layout::Horizontal),
            "max" => Some(Layout::Max),
            "grid" => Some(Layout::Grid),
            "auto" => Some(Layout::Auto),
            _ => s
                .parse::<usize>()
                .ok()
                .and_then(|i| Layout::ALL.get(i).copied()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Layout::Vertical => "vertical",
            Layout::Horizontal => "horizontal",
            Layout::Max => "max",
            Layout::Grid => "grid",
            Layout::Auto => "auto",
        }
    }

    /// Position in ALL; only for layouts other than Auto.
    fn index(self) -> usize {
        Layout::ALL.iter().position(|&l| l == self).unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// children stacked top to bottom
    Vertical,
    /// children side by side
    Horizontal,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn inset(self, n: i32) -> Rect {
        Rect::new(self.x + n, self.y + n, self.w - 2 * n, self.h - 2 * n)
    }

    pub fn center(self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

#[derive(Clone, Debug)]
pub struct Leaf {
    pub wins: Vec<WinId>,
    pub sel: usize,
    pub layout: Layout,
    /// what Auto resolved to at the last layout
    pub auto_horizontal: std::cell::Cell<bool>,
}

impl Leaf {
    pub fn new(layout: Layout) -> Leaf {
        Leaf {
            wins: Vec::new(),
            sel: 0,
            layout,
            auto_horizontal: Default::default(),
        }
    }

    /// The layout as shown, with Auto resolved.
    pub fn effective(&self) -> Layout {
        match self.layout {
            Layout::Auto if self.auto_horizontal.get() => Layout::Horizontal,
            Layout::Auto => Layout::Vertical,
            l => l,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Split {
    pub align: Align,
    /// size of the first child relative to the whole split
    pub frac: f64,
    pub sel: usize,
    pub a: Box<Frame>,
    pub b: Box<Frame>,
}

#[derive(Clone, Debug)]
pub enum Frame {
    Leaf(Leaf),
    Split(Split),
}

const MIN_FRAC: f64 = 0.1;
const MAX_DEPTH: usize = 40;

pub fn clamp_frac(f: f64) -> f64 {
    f.clamp(MIN_FRAC, 1.0 - MIN_FRAC)
}

/// Settings that influence tiling geometry.
#[derive(Clone, Debug)]
pub struct Geometry {
    pub frame_gap: i32,
    pub frame_border_width: i32,
    pub frame_padding: i32,
    pub window_gap: i32,
    pub smart_frame_surroundings: bool,
    pub smart_window_surroundings: bool,
    /// apply window_gap once between neighbouring windows, also across
    /// frames, instead of once per frame (herbstluftwm doubles it there)
    pub smart_gaps: bool,
}

/// Where a tiled window goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub win: WinId,
    /// outer geometry of the window, including its border
    pub rect: Rect,
    pub visible: bool,
}

/// A frame as it should be drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameGeom {
    pub path: Vec<usize>,
    /// the frame's content area; the frame border is drawn around it
    pub content: Rect,
    pub has_clients: bool,
    pub is_root: bool,
    pub focused: bool,
}

/// How `split` behaves for a given mode, mirroring herbstluftwm's SplitMode.
struct SplitMode {
    align: Align,
    /// whether the existing frame becomes the first child
    frame_to_first: bool,
    /// which child gets selected afterwards; None keeps it
    selection: Option<usize>,
}

impl Frame {
    pub fn new_leaf(layout: Layout) -> Frame {
        Frame::Leaf(Leaf::new(layout))
    }

    pub fn as_leaf(&self) -> Option<&Leaf> {
        match self {
            Frame::Leaf(l) => Some(l),
            Frame::Split(_) => None,
        }
    }

    pub fn at(&self, path: &[usize]) -> &Frame {
        let mut f = self;
        for &i in path {
            f = match f {
                Frame::Split(s) => {
                    if i == 0 {
                        &s.a
                    } else {
                        &s.b
                    }
                }
                Frame::Leaf(_) => panic!("path through leaf"),
            };
        }
        f
    }

    pub fn at_mut(&mut self, path: &[usize]) -> &mut Frame {
        let mut f = self;
        for &i in path {
            f = match f {
                Frame::Split(s) => {
                    if i == 0 {
                        &mut s.a
                    } else {
                        &mut s.b
                    }
                }
                Frame::Leaf(_) => panic!("path through leaf"),
            };
        }
        f
    }

    fn split_at_mut(&mut self, path: &[usize]) -> &mut Split {
        match self.at_mut(path) {
            Frame::Split(s) => s,
            Frame::Leaf(_) => panic!("not a split"),
        }
    }

    fn leaf_at_mut(&mut self, path: &[usize]) -> &mut Leaf {
        match self.at_mut(path) {
            Frame::Leaf(l) => l,
            Frame::Split(_) => panic!("not a leaf"),
        }
    }

    /// Path from this frame to the leaf reached by following selections.
    pub fn focus_path(&self) -> Vec<usize> {
        let mut path = Vec::new();
        let mut f = self;
        while let Frame::Split(s) = f {
            path.push(s.sel);
            f = if s.sel == 0 { &s.a } else { &s.b };
        }
        path
    }

    pub fn focused_leaf(&self) -> &Leaf {
        self.at(&self.focus_path()).as_leaf().unwrap()
    }

    pub fn focused_leaf_mut(&mut self) -> &mut Leaf {
        let p = self.focus_path();
        self.leaf_at_mut(&p)
    }

    pub fn focused_window(&self) -> Option<WinId> {
        let l = self.focused_leaf();
        l.wins.get(l.sel).copied()
    }

    pub fn windows(&self) -> Vec<WinId> {
        let mut v = Vec::new();
        self.for_each_leaf(&mut Vec::new(), &mut |_, l| v.extend(&l.wins));
        v
    }

    pub fn for_each_leaf(&self, path: &mut Vec<usize>, f: &mut dyn FnMut(&[usize], &Leaf)) {
        match self {
            Frame::Leaf(l) => f(path, l),
            Frame::Split(s) => {
                path.push(0);
                s.a.for_each_leaf(path, f);
                path.pop();
                path.push(1);
                s.b.for_each_leaf(path, f);
                path.pop();
            }
        }
    }

    pub fn leaf_count(&self) -> usize {
        let mut n = 0;
        self.for_each_leaf(&mut Vec::new(), &mut |_, _| n += 1);
        n
    }

    pub fn leaf_with(&self, w: WinId) -> Option<Vec<usize>> {
        let mut found = None;
        self.for_each_leaf(&mut Vec::new(), &mut |p, l| {
            if found.is_none() && l.wins.contains(&w) {
                found = Some(p.to_vec());
            }
        });
        found
    }

    /// Make the frame at `path` the focused one by adjusting all selections
    /// on the way to it.
    pub fn focus_frame(&mut self, path: &[usize]) {
        for i in 0..path.len() {
            self.split_at_mut(&path[..i]).sel = path[i];
        }
    }

    /// Insert after the selection of the focused frame.
    pub fn insert(&mut self, w: WinId, focus: bool) {
        let l = self.focused_leaf_mut();
        let idx = (l.sel + 1).min(l.wins.len());
        l.wins.insert(idx, w);
        if focus || l.wins.len() == 1 {
            l.sel = idx;
        }
    }

    pub fn remove_window(&mut self, w: WinId) -> bool {
        let Some(path) = self.leaf_with(w) else {
            return false;
        };
        let l = self.leaf_at_mut(&path);
        let idx = l.wins.iter().position(|&x| x == w).unwrap();
        l.wins.remove(idx);
        if l.sel >= idx && l.sel > 0 {
            l.sel -= 1;
        }
        l.sel = l.sel.min(l.wins.len().saturating_sub(1));
        true
    }

    /// Select `w` within its frame and focus that frame.
    pub fn focus_window(&mut self, w: WinId) -> bool {
        let Some(path) = self.leaf_with(w) else {
            return false;
        };
        let l = self.leaf_at_mut(&path);
        l.sel = l.wins.iter().position(|&x| x == w).unwrap();
        self.focus_frame(&path);
        true
    }

    /// Find the split that separates the leaf at `path` from its neighbour
    /// in `dir`. Returns the split's path and the index of the neighbour child.
    fn neighbour(&self, path: &[usize], dir: Dir) -> Option<(Vec<usize>, usize)> {
        for i in (0..path.len()).rev() {
            let parent = &path[..i];
            let child = path[i];
            let Frame::Split(s) = self.at(parent) else {
                unreachable!()
            };
            let hit = match dir {
                Dir::Right => s.align == Align::Horizontal && child == 0,
                Dir::Left => s.align == Align::Horizontal && child == 1,
                Dir::Down => s.align == Align::Vertical && child == 0,
                Dir::Up => s.align == Align::Vertical && child == 1,
            };
            if hit {
                return Some((parent.to_vec(), 1 - child));
            }
        }
        None
    }

    pub fn focus_dir(&mut self, dir: Dir, external_only: bool) -> bool {
        let path = self.focus_path();
        if !external_only {
            let l = self.leaf_at_mut(&path);
            if let Some(i) = inner_neighbour(l, dir) {
                l.sel = i;
                return true;
            }
        }
        match self.neighbour(&path, dir) {
            Some((parent, other)) => {
                self.split_at_mut(&parent).sel = other;
                true
            }
            None => false,
        }
    }

    pub fn shift_dir(&mut self, dir: Dir, external_only: bool) -> bool {
        let path = self.focus_path();
        let l = self.leaf_at_mut(&path);
        if l.wins.is_empty() {
            return false;
        }
        if !external_only {
            if let Some(i) = inner_neighbour(l, dir) {
                l.wins.swap(i, l.sel);
                l.sel = i;
                return true;
            }
        }
        let Some((parent, other)) = self.neighbour(&path, dir) else {
            return false;
        };
        let w = self.focused_window().unwrap();
        self.remove_window(w);
        let mut target = parent.clone();
        target.push(other);
        let sub = self.at_mut(&target);
        sub.insert(w, true);
        self.split_at_mut(&parent).sel = other;
        true
    }

    /// Swap the focused window with the window in direction `dir`, keeping
    /// focus on it (the old `swap` script).
    pub fn swap_dir(&mut self, dir: Dir) -> bool {
        let path = self.focus_path();
        let l = self.leaf_at_mut(&path);
        if l.wins.is_empty() {
            return false;
        }
        if let Some(i) = inner_neighbour(l, dir) {
            l.wins.swap(i, l.sel);
            l.sel = i;
            return true;
        }
        let Some((parent, other)) = self.neighbour(&path, dir) else {
            return false;
        };
        let mut target = parent.clone();
        target.push(other);
        target.extend(self.at(&target).focus_path());
        let Some(theirs) = self
            .at(&target)
            .as_leaf()
            .and_then(|l| l.wins.get(l.sel).copied())
        else {
            return self.shift_dir(dir, true);
        };
        let mine = self.focused_window().unwrap();
        let l = self.leaf_at_mut(&path);
        let li = l.sel;
        l.wins[li] = theirs;
        let t = self.leaf_at_mut(&target);
        let ti = t.sel;
        t.wins[ti] = mine;
        self.split_at_mut(&parent).sel = other;
        true
    }

    /// Put `new` where `old` is.
    pub fn replace_window(&mut self, old: WinId, new: WinId) -> bool {
        let Some(path) = self.leaf_with(old) else {
            return false;
        };
        let l = self.leaf_at_mut(&path);
        let i = l.wins.iter().position(|&x| x == old).unwrap();
        l.wins[i] = new;
        true
    }

    /// `split MODE [FRACTION]` on the focused frame.
    pub fn split(&mut self, mode: &str, frac: Option<f64>, rect_hint: Rect) -> Result<(), String> {
        let path = self.focus_path();
        let auto = if rect_hint.w > rect_hint.h {
            Align::Horizontal
        } else {
            Align::Vertical
        };
        let mut m = match mode {
            "top" | "t" => SplitMode {
                align: Align::Vertical,
                frame_to_first: false,
                selection: Some(1),
            },
            "bottom" | "b" | "vertical" | "v" => SplitMode {
                align: Align::Vertical,
                frame_to_first: true,
                selection: Some(0),
            },
            "right" | "r" | "horizontal" | "h" => SplitMode {
                align: Align::Horizontal,
                frame_to_first: true,
                selection: Some(0),
            },
            "left" | "l" => SplitMode {
                align: Align::Horizontal,
                frame_to_first: false,
                selection: Some(1),
            },
            "explode" => SplitMode {
                align: Align::Vertical,
                frame_to_first: true,
                selection: None,
            },
            "auto" => SplitMode {
                align: auto,
                frame_to_first: true,
                selection: Some(0),
            },
            _ => return Err(format!("invalid split mode \"{mode}\"")),
        };
        if path.len() >= MAX_DEPTH {
            return Err("too many splits".into());
        }
        let mut fraction = clamp_frac(frac.unwrap_or(0.5));
        let leaf = self.leaf_at_mut(&path);
        let count = leaf.wins.len();
        let mut leaving = 0;
        if mode == "explode" {
            m.align = match leaf.effective() {
                _ if count <= 1 => auto,
                Layout::Max => auto,
                Layout::Grid if count == 2 => Align::Horizontal,
                Layout::Horizontal => Align::Horizontal,
                _ => Align::Vertical,
            };
            let stay = count.div_ceil(2);
            if matches!(leaf.effective(), Layout::Horizontal | Layout::Vertical)
                && frac.is_none()
                && count > 1
            {
                fraction = stay as f64 / count as f64;
            }
            leaving = count / 2;
        }
        let staying = count - leaving;
        let mut second = Leaf::new(leaf.layout);
        second.wins = leaf.wins.split_off(staying);
        let mut sel = 0;
        if leaf.sel >= staying && count > 0 {
            sel = 1;
            second.sel = leaf.sel - staying;
            leaf.sel = staying.saturating_sub(1);
        }
        let old = std::mem::replace(self.at_mut(&path), Frame::new_leaf(Layout::Vertical));
        let mut split = Split {
            align: m.align,
            frac: fraction,
            sel,
            a: Box::new(old),
            b: Box::new(Frame::Leaf(second)),
        };
        if !m.frame_to_first {
            std::mem::swap(&mut split.a, &mut split.b);
        }
        if let Some(s) = m.selection {
            split.sel = s;
        }
        *self.at_mut(&path) = Frame::Split(split);
        Ok(())
    }

    /// `remove`: merge the focused frame into its sibling.
    pub fn remove_frame(&mut self) {
        let path = self.focus_path();
        let Some((&idx, parent_path)) = path.split_last() else {
            return;
        };
        let at_front = idx == 0;
        let leaf = self.leaf_at_mut(&path);
        let focus_index = leaf.sel;
        let removed = std::mem::take(&mut leaf.wins);

        let Frame::Split(parent) = self.at_mut(parent_path) else {
            unreachable!()
        };
        let align = parent.align;
        let sibling = if at_front {
            &mut parent.b
        } else {
            &mut parent.a
        };
        let mut target: &mut Frame = sibling;
        while let Frame::Split(s) = target {
            if s.align == align {
                s.sel = if at_front { 0 } else { 1 };
            }
            target = if s.sel == 0 { &mut s.a } else { &mut s.b };
        }
        let Frame::Leaf(t) = target else {
            unreachable!()
        };
        let old_count = t.wins.len();
        if at_front {
            let rest = std::mem::take(&mut t.wins);
            t.wins = removed.clone();
            t.wins.extend(rest);
        } else {
            t.wins.extend(removed.iter().copied());
        }
        if !removed.is_empty() {
            t.sel = if at_front {
                focus_index
            } else {
                focus_index + old_count
            };
        }
        let Frame::Split(parent) = self.at_mut(parent_path) else {
            unreachable!()
        };
        let sibling = std::mem::replace(
            if at_front {
                &mut parent.b
            } else {
                &mut parent.a
            },
            Box::new(Frame::new_leaf(Layout::Vertical)),
        );
        *self.at_mut(parent_path) = *sibling;
    }

    pub fn resize(&mut self, dir: Dir, delta: f64) -> bool {
        let delta = if matches!(dir, Dir::Left | Dir::Up) {
            -delta
        } else {
            delta
        };
        let path = self.focus_path();
        let found = self
            .neighbour(&path, dir)
            .or_else(|| self.neighbour(&path, dir.flip()));
        match found {
            Some((parent, _)) => {
                let s = self.split_at_mut(&parent);
                s.frac = clamp_frac(s.frac + delta);
                true
            }
            None => false,
        }
    }

    pub fn rotate(&mut self) {
        if let Frame::Split(s) = self {
            match s.align {
                Align::Vertical => s.align = Align::Horizontal,
                Align::Horizontal => {
                    s.align = Align::Vertical;
                    s.sel = 1 - s.sel;
                    std::mem::swap(&mut s.a, &mut s.b);
                    s.frac = 1.0 - s.frac;
                }
            }
            s.a.rotate();
            s.b.rotate();
        }
    }

    /// `cycle DELTA`: change the selection within the focused frame.
    pub fn cycle(&mut self, delta: i32) {
        let l = self.focused_leaf_mut();
        if !l.wins.is_empty() {
            let n = l.wins.len() as i32;
            l.sel = (l.sel as i32 + delta).rem_euclid(n) as usize;
        }
    }

    /// `cycle_frame DELTA`: focus the next frame in traversal order.
    pub fn cycle_frame(&mut self, delta: i32) {
        let focus = self.focus_path();
        let mut paths = Vec::new();
        self.for_each_leaf(&mut Vec::new(), &mut |p, _| paths.push(p.to_vec()));
        let idx = paths.iter().position(|p| *p == focus).unwrap() as i32;
        let next = (idx + delta).rem_euclid(paths.len() as i32) as usize;
        let target = paths[next].clone();
        self.focus_frame(&target);
    }

    /// `cycle_layout [DELTA [LAYOUTS...]]`
    pub fn cycle_layout(&mut self, delta: i32, list: &[Layout]) {
        let l = self.focused_leaf_mut();
        // cycle from what is shown, so leaving Auto always changes something
        let cur = l.effective();
        l.layout = if list.is_empty() {
            Layout::ALL[(cur.index() as i32 + delta).rem_euclid(4) as usize]
        } else {
            match list.iter().position(|&x| x == cur) {
                Some(i) => list[(i as i32 + delta).rem_euclid(list.len() as i32) as usize],
                None => list[0],
            }
        };
    }

    /// Compute window and frame geometry. `rect` is the monitor area with
    /// the outer frame gap already applied (see `apply_monitor_gap`).
    pub fn compute(&self, rect: Rect, g: &Geometry) -> (Vec<Tile>, Vec<FrameGeom>) {
        let mut tiles = Vec::new();
        let mut frames = Vec::new();
        let focus = self.focus_path();
        let mut rect = rect;
        if g.smart_gaps && !self.borderless(g) {
            // the top/left gap of every window touching the monitor edge; the
            // bottom/right gap comes off each window
            rect.x += g.window_gap;
            rect.y += g.window_gap;
            rect.w -= g.window_gap;
            rect.h -= g.window_gap;
        }
        self.compute_rec(rect, g, &mut Vec::new(), &focus, &mut tiles, &mut frames);
        (tiles, frames)
    }

    /// smart_window_surroundings drops gaps for a lone window (or max frame)
    fn borderless(&self, g: &Geometry) -> bool {
        match self {
            Frame::Leaf(l) => {
                g.smart_window_surroundings && (l.layout == Layout::Max || l.wins.len() == 1)
            }
            Frame::Split(_) => false,
        }
    }

    fn compute_rec(
        &self,
        rect: Rect,
        g: &Geometry,
        path: &mut Vec<usize>,
        focus: &[usize],
        tiles: &mut Vec<Tile>,
        frames: &mut Vec<FrameGeom>,
    ) {
        match self {
            Frame::Split(s) => {
                let (mut first, mut second) = (rect, rect);
                match s.align {
                    Align::Vertical => {
                        first.h = (rect.h as f64 * s.frac) as i32;
                        second.y += first.h;
                        second.h -= first.h;
                    }
                    Align::Horizontal => {
                        first.w = (rect.w as f64 * s.frac) as i32;
                        second.x += first.w;
                        second.w -= first.w;
                    }
                }
                path.push(0);
                s.a.compute_rec(first, g, path, focus, tiles, frames);
                path.pop();
                path.push(1);
                s.b.compute_rec(second, g, path, focus, tiles, frames);
                path.pop();
            }
            Frame::Leaf(l) => {
                let is_root = path.is_empty();
                let mut r = rect;
                if !g.smart_frame_surroundings || !is_root {
                    r.w -= g.frame_gap;
                    r.h -= g.frame_gap;
                    r = r.inset(g.frame_border_width);
                }
                r.w = r.w.max(1);
                r.h = r.h.max(1);
                frames.push(FrameGeom {
                    path: path.clone(),
                    content: r,
                    has_clients: !l.wins.is_empty(),
                    is_root,
                    focused: path.as_slice() == focus,
                });
                if l.wins.is_empty() {
                    return;
                }
                let smart = if g.smart_gaps {
                    // only a lone frame can drop its gaps without making
                    // the spacing between frames uneven
                    is_root && self.borderless(g)
                } else {
                    g.smart_window_surroundings && (l.layout == Layout::Max || l.wins.len() == 1)
                };
                let wg = if smart { 0 } else { g.window_gap };
                if !smart {
                    if !g.smart_gaps {
                        r.x += wg;
                        r.y += wg;
                        r.w -= wg;
                        r.h -= wg;
                    }
                    r = r.inset(g.frame_padding);
                }
                for (win, mut wr, visible) in layout_leaf(l, r) {
                    wr.w = (wr.w - wg).max(1);
                    wr.h = (wr.h - wg).max(1);
                    tiles.push(Tile {
                        win,
                        rect: wr,
                        visible,
                    });
                }
            }
        }
    }
}

/// The left/top part of the frame gap is applied once per monitor.
pub fn apply_monitor_gap(r: Rect, g: &Geometry) -> Rect {
    Rect::new(
        r.x + g.frame_gap,
        r.y + g.frame_gap,
        r.w - g.frame_gap,
        r.h - g.frame_gap,
    )
}

fn grid_size(count: usize) -> (usize, usize) {
    let mut cols = 0;
    while cols * cols < count {
        cols += 1;
    }
    let rows = if cols == 0 { 0 } else { count.div_ceil(cols) };
    (rows, cols)
}

fn inner_neighbour(l: &Leaf, dir: Dir) -> Option<usize> {
    let i = l.sel as i64;
    let n = l.wins.len() as i64;
    let r = match l.effective() {
        Layout::Vertical => match dir {
            Dir::Up => i - 1,
            Dir::Down => i + 1,
            _ => -1,
        },
        Layout::Horizontal => match dir {
            Dir::Left => i - 1,
            Dir::Right => i + 1,
            _ => -1,
        },
        // herbstluftwm only moves within max frames at the "all" level,
        // which plain focus/shift don't use
        Layout::Max => -1,
        Layout::Auto => unreachable!("effective() resolves Auto"),
        Layout::Grid => {
            let (_, cols) = grid_size(n as usize);
            if cols == 0 {
                return None;
            }
            let cols = cols as i64;
            let c = i % cols;
            match dir {
                Dir::Down => i + cols,
                Dir::Up => i - cols,
                Dir::Right if c < cols - 1 => i + 1,
                Dir::Left if c > 0 => i - 1,
                _ => -1,
            }
        }
    };
    (r >= 0 && r < n).then_some(r as usize)
}

fn layout_linear(wins: &[WinId], rect: Rect, vertical: bool) -> Vec<(WinId, Rect, bool)> {
    let n = wins.len() as i32;
    let mut cur = rect;
    let (last_x, last_y, step_x, step_y);
    if vertical {
        last_y = cur.h % n;
        last_x = 0;
        cur.h /= n;
        step_y = cur.h;
        step_x = 0;
    } else {
        last_y = 0;
        last_x = cur.w % n;
        cur.w /= n;
        step_y = 0;
        step_x = cur.w;
    }
    let mut out = Vec::new();
    for (i, &w) in wins.iter().enumerate() {
        let mut r = cur;
        if i as i32 == n - 1 {
            r.h += last_y;
            r.w += last_x;
        }
        out.push((w, r, true));
        cur.y += step_y;
        cur.x += step_x;
    }
    out
}

fn layout_leaf(l: &Leaf, rect: Rect) -> Vec<(WinId, Rect, bool)> {
    if l.layout == Layout::Auto {
        l.auto_horizontal.set(rect.w > rect.h);
    }
    match l.effective() {
        Layout::Vertical => layout_linear(&l.wins, rect, true),
        Layout::Horizontal => layout_linear(&l.wins, rect, false),
        Layout::Max => l
            .wins
            .iter()
            .enumerate()
            .map(|(i, &w)| (w, rect, i == l.sel))
            .collect(),
        Layout::Auto => unreachable!("effective() resolves Auto"),
        Layout::Grid => {
            let (rows, cols) = grid_size(l.wins.len());
            let (width, height) = (rect.w / cols as i32, rect.h / rows as i32);
            let count = l.wins.len();
            let mut out = Vec::new();
            let mut i = 0;
            let mut cur = rect;
            for r in 0..rows {
                cur.x = rect.x;
                cur.w = width;
                cur.h = height;
                if r == rows - 1 {
                    cur.h += rect.h % rows as i32;
                }
                for c in 0..cols {
                    if i >= count {
                        break;
                    }
                    let mut cell = cur;
                    // gapless_grid (on by default): the last window fills the row
                    if i == count - 1 && count % cols != 0 {
                        cell.w = rect.x + rect.w - cur.x;
                    } else if c == cols - 1 {
                        cell.w += rect.w % cols as i32;
                    }
                    out.push((l.wins[i], cell, true));
                    cur.x += width;
                    i += 1;
                }
                cur.y += height;
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geo() -> Geometry {
        Geometry {
            frame_gap: 0,
            frame_border_width: 0,
            frame_padding: 0,
            window_gap: 0,
            smart_frame_surroundings: false,
            smart_window_surroundings: false,
            smart_gaps: false,
        }
    }

    fn tree_with(wins: &[WinId]) -> Frame {
        let mut t = Frame::new_leaf(Layout::Vertical);
        for &w in wins {
            t.insert(w, true);
        }
        t
    }

    #[test]
    fn split_keeps_focus_on_old_frame() {
        let mut t = tree_with(&[1]);
        t.split("bottom", Some(0.82), Rect::new(0, 0, 100, 100))
            .unwrap();
        assert_eq!(t.focus_path(), vec![0]);
        assert_eq!(t.focused_window(), Some(1));
        t.split("left", Some(0.38), Rect::new(0, 0, 100, 100))
            .unwrap();
        // old frame is now the second child and stays selected
        assert_eq!(t.focus_path(), vec![0, 1]);
        assert_eq!(t.focused_window(), Some(1));
    }

    #[test]
    fn split_then_shift_moves_window_into_new_frame() {
        // the user's `split` script: split bottom 0.82 , shift -e down
        let mut t = tree_with(&[1, 2]);
        t.split("bottom", Some(0.82), Rect::new(0, 0, 100, 100))
            .unwrap();
        assert!(t.shift_dir(Dir::Down, true));
        assert_eq!(t.focus_path(), vec![1]);
        assert_eq!(t.focused_window(), Some(2));
        assert_eq!(t.at(&[0]).as_leaf().unwrap().wins, vec![1]);
    }

    #[test]
    fn focus_moves_between_frames_and_within_layout() {
        let mut t = tree_with(&[1, 2]);
        t.split("right", None, Rect::default()).unwrap();
        t.shift_dir(Dir::Right, true);
        assert_eq!(t.focused_window(), Some(2));
        assert!(t.focus_dir(Dir::Left, false));
        assert_eq!(t.focused_window(), Some(1));
        assert!(!t.focus_dir(Dir::Left, false));
        assert!(t.focus_dir(Dir::Right, false));
        assert_eq!(t.focused_window(), Some(2));
    }

    #[test]
    fn remove_merges_windows_into_sibling() {
        let mut t = tree_with(&[1, 2]);
        t.split("right", None, Rect::default()).unwrap();
        t.shift_dir(Dir::Right, true);
        t.remove_frame();
        let l = t.as_leaf().unwrap();
        assert_eq!(l.wins, vec![1, 2]);
        assert_eq!(t.focused_window(), Some(2));
    }

    #[test]
    fn remove_window_fixes_selection() {
        let mut t = tree_with(&[1, 2, 3]);
        t.focus_window(2);
        t.remove_window(2);
        assert_eq!(t.focused_window(), Some(1));
        t.remove_window(1);
        assert_eq!(t.focused_window(), Some(3));
        t.remove_window(3);
        assert_eq!(t.focused_window(), None);
    }

    #[test]
    fn resize_adjusts_nearest_split() {
        let mut t = tree_with(&[]);
        t.split("right", Some(0.5), Rect::default()).unwrap();
        assert!(t.resize(Dir::Right, 0.05));
        let Frame::Split(s) = &t else { panic!() };
        assert!((s.frac - 0.55).abs() < 1e-9);
        // nothing to the left: falls back to the right border
        assert!(t.resize(Dir::Left, 0.05));
        let Frame::Split(s) = &t else { panic!() };
        assert!((s.frac - 0.5).abs() < 1e-9);
    }

    #[test]
    fn rotate_turns_horizontal_into_vertical() {
        let mut t = tree_with(&[1]);
        t.split("right", Some(0.3), Rect::default()).unwrap();
        t.rotate();
        let Frame::Split(s) = &t else { panic!() };
        assert_eq!(s.align, Align::Vertical);
        assert!((s.frac - 0.7).abs() < 1e-9);
        assert_eq!(t.focused_window(), Some(1));
    }

    #[test]
    fn smart_gaps_are_even_across_frames() {
        // the user's layout: split bottom, one window in each frame
        let mut g = geo();
        g.window_gap = 6;
        let mut t = tree_with(&[1, 2]);
        t.split("bottom", Some(0.5), Rect::default()).unwrap();
        t.shift_dir(Dir::Down, true);
        let area = Rect::new(0, 0, 100, 100);
        let (tiles, _) = t.compute(apply_monitor_gap(area, &g), &g);
        // herbstluftwm: 12px between the frames
        assert_eq!(tiles[1].rect.y - (tiles[0].rect.y + tiles[0].rect.h), 12);
        g.smart_gaps = true;
        let (tiles, _) = t.compute(apply_monitor_gap(area, &g), &g);
        let (top, bottom) = (tiles[0].rect, tiles[1].rect);
        assert_eq!(top, Rect::new(6, 6, 88, 41));
        assert_eq!(bottom, Rect::new(6, 53, 88, 41));
        assert_eq!(bottom.y - (top.y + top.h), 6);
        assert_eq!(100 - (bottom.y + bottom.h), 6);
    }

    #[test]
    fn auto_layout_follows_the_long_axis() {
        let mut t = tree_with(&[1, 2]);
        t.focused_leaf_mut().layout = Layout::Auto;
        let (tiles, _) = t.compute(Rect::new(0, 0, 200, 100), &geo());
        assert_eq!(tiles[1].rect, Rect::new(100, 0, 100, 100));
        // focus follows the resolved orientation
        assert!(t.focus_dir(Dir::Left, false));
        let (tiles, _) = t.compute(Rect::new(0, 0, 100, 200), &geo());
        assert_eq!(tiles[1].rect, Rect::new(0, 100, 100, 100));
        assert!(t.focus_dir(Dir::Down, false));
        // cycling leaves Auto for the other orientation
        t.cycle_layout(1, &[Layout::Vertical, Layout::Horizontal]);
        assert_eq!(t.focused_leaf().layout, Layout::Horizontal);
        // new frames from a split inherit Auto
        let mut t = tree_with(&[1]);
        t.focused_leaf_mut().layout = Layout::Auto;
        t.split("right", None, Rect::default()).unwrap();
        assert_eq!(t.at(&[1]).as_leaf().unwrap().layout, Layout::Auto);
    }

    #[test]
    fn grid_layout_matches_herbstluftwm() {
        let mut t = tree_with(&[1, 2, 3]);
        t.focused_leaf_mut().layout = Layout::Grid;
        let (tiles, _) = t.compute(Rect::new(0, 0, 100, 100), &geo());
        let rects: Vec<Rect> = tiles.iter().map(|t| t.rect).collect();
        assert_eq!(
            rects,
            vec![
                Rect::new(0, 0, 50, 50),
                Rect::new(50, 0, 50, 50),
                Rect::new(0, 50, 100, 50)
            ]
        );
    }

    #[test]
    fn gaps_are_applied_like_herbstluftwm() {
        let mut g = geo();
        g.window_gap = 6;
        let t = tree_with(&[1]);
        let (tiles, _) = t.compute(apply_monitor_gap(Rect::new(0, 0, 100, 100), &g), &g);
        assert_eq!(tiles[0].rect, Rect::new(6, 6, 88, 88));
    }

    #[test]
    fn swap_exchanges_windows_across_frames() {
        let mut t = tree_with(&[1, 2]);
        t.split("right", None, Rect::default()).unwrap();
        t.shift_dir(Dir::Right, true);
        t.focus_dir(Dir::Left, false);
        assert!(t.swap_dir(Dir::Right));
        assert_eq!(t.at(&[0]).as_leaf().unwrap().wins, vec![2]);
        assert_eq!(t.at(&[1]).as_leaf().unwrap().wins, vec![1]);
        assert_eq!(t.focused_window(), Some(1));
    }

    #[test]
    fn swap_moves_like_focus_and_keeps_the_window_focused() {
        // three frames side by side; the middle one is selected in the right
        // half, so `focus left` from the right lands in the middle
        let mut t = tree_with(&[1]);
        t.split("right", None, Rect::default()).unwrap();
        t.split("right", None, Rect::default()).unwrap();
        t.focus_frame(&[0, 1]);
        t.insert(2, true);
        t.focus_frame(&[1]);
        t.insert(3, true);
        t.insert(4, true);
        t.focused_leaf_mut().layout = Layout::Vertical;
        // inside the frame: 4 swaps with 3 above it
        assert!(t.swap_dir(Dir::Up));
        assert_eq!(t.at(&[1]).as_leaf().unwrap().wins, vec![4, 3]);
        assert_eq!(t.focused_window(), Some(4));
        // across frames: 4 swaps with 2, which `focus left` would reach
        assert!(t.swap_dir(Dir::Left));
        assert_eq!(t.at(&[0, 1]).as_leaf().unwrap().wins, vec![4]);
        assert_eq!(t.at(&[1]).as_leaf().unwrap().wins, vec![2, 3]);
        assert_eq!(t.focused_window(), Some(4));
        // again: into the leftmost frame
        assert!(t.swap_dir(Dir::Left));
        assert_eq!(t.at(&[0, 0]).as_leaf().unwrap().wins, vec![4]);
        assert_eq!(t.focused_window(), Some(4));
        assert!(!t.swap_dir(Dir::Left));
    }

    #[test]
    fn explode_splits_windows() {
        let mut t = tree_with(&[1, 2, 3, 4]);
        t.split("explode", None, Rect::new(0, 0, 100, 100)).unwrap();
        assert_eq!(t.at(&[0]).as_leaf().unwrap().wins, vec![1, 2]);
        assert_eq!(t.at(&[1]).as_leaf().unwrap().wins, vec![3, 4]);
        assert_eq!(t.focused_window(), Some(4));
    }
}
