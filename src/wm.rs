//! Window manager state and the river-window-management-v1 manage/render loop.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::net::UnixStream;

use wayland_backend::client::ObjectId;
use wayland_client::protocol::{
    wl_buffer::WlBuffer,
    wl_compositor::WlCompositor,
    wl_output::{self, WlOutput},
    wl_registry,
    wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1;
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};

use crate::gesture::{self, Dir, Gesture};
use crate::protocol::{
    hrwm_activation_v1::{self, HrwmActivationV1},
    river_layer_shell_output_v1::{self, RiverLayerShellOutputV1},
    river_layer_shell_seat_v1::{self, RiverLayerShellSeatV1},
    river_layer_shell_v1::RiverLayerShellV1,
    river_node_v1::RiverNodeV1,
    river_output_v1::{self, RiverOutputV1},
    river_pointer_binding_v1::{self, RiverPointerBindingV1},
    river_seat_v1::{self, RiverSeatV1},
    river_shell_surface_v1::RiverShellSurfaceV1,
    river_touch_gesture_v1::{self, RiverTouchGestureV1},
    river_touch_gestures_seat_v1::RiverTouchGesturesSeatV1,
    river_touch_gestures_v1::RiverTouchGesturesV1,
    river_window_manager_v1::{self, RiverWindowManagerV1},
    river_window_v1::{self, RiverWindowV1},
    river_xkb_binding_v1::{self, RiverXkbBindingV1},
    river_xkb_bindings_v1::RiverXkbBindingsV1,
};
use crate::rules::{Props, Rules};
use crate::settings::{Color, Settings};
use crate::tree::{Frame, Rect, WinId, apply_monitor_gap};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    Hidden,
    /// the slot includes the window border
    Tiled {
        slot: Rect,
    },
    Floating,
    Fullscreen,
}

pub struct Win {
    pub id: WinId,
    /// river's name for the window, kept across window manager restarts
    pub identifier: String,
    pub obj: RiverWindowV1,
    node: RiverNodeV1,
    pub app_id: String,
    pub title: String,
    parent: Option<ObjectId>,
    pid: i32,
    new: bool,
    closed: bool,
    pub tag: usize,
    pub floating: bool,
    pub pseudotile: bool,
    pub fullscreen: bool,
    /// wants attention; cleared when focused
    pub urgent: bool,
    /// output river currently shows the window fullscreen on
    fs_output: Option<ObjectId>,
    /// content geometry while floating
    pub float_rect: Option<Rect>,
    /// size the window picked itself when offered 0x0
    natural: Option<(i32, i32)>,
    dims: Option<(i32, i32)>,
    proposed: Option<(i32, i32)>,
    tiled_edges: Option<u32>,
    only_csd: bool,
    ssd_sent: bool,
    req_fullscreen: Option<bool>,
    req_move: Option<ObjectId>,
    req_resize: Option<(ObjectId, u32)>,
    req_touch_move: Option<(ObjectId, i32)>,
    req_touch_resize: Option<(ObjectId, i32, u32)>,
    pub place: Place,
}

impl Win {
    pub fn hex_id(&self) -> String {
        format!("0x{:x}", self.id)
    }

    /// Content geometry as last laid out (what render positions it at).
    pub fn geometry(&self, border: i32) -> Option<Rect> {
        let (dw, dh) = self.dims?;
        match self.place {
            Place::Tiled { slot } => {
                let c = slot.inset(border);
                let (x, y) = if self.pseudotile {
                    (c.x + (c.w - dw) / 2, c.y + (c.h - dh) / 2)
                } else {
                    (c.x, c.y)
                };
                Some(Rect::new(x, y, dw, dh))
            }
            Place::Floating => self.float_rect.map(|r| Rect::new(r.x, r.y, dw, dh)),
            Place::Fullscreen | Place::Hidden => None,
        }
    }
}

pub struct Tag {
    pub name: String,
    pub tree: Frame,
    /// floating windows, bottom to top
    pub floats: Vec<WinId>,
    /// the tag's floating mode (`floating toggle`)
    pub floating: bool,
    /// set when focus is on a floating window
    pub float_focus: Option<WinId>,
    /// created for a monitor that had no free tag; gives way to the next
    /// tag that is added
    pub placeholder: bool,
}

impl Tag {
    pub fn focused(&self) -> Option<WinId> {
        self.float_focus.or_else(|| self.tree.focused_window())
    }

    pub fn windows(&self) -> Vec<WinId> {
        let mut v = self.tree.windows();
        v.extend(&self.floats);
        v
    }
}

pub struct Monitor {
    obj: RiverOutputV1,
    pub oid: ObjectId,
    wl: Option<WlOutput>,
    /// connector name like eDP-1, from wl_output
    pub name: String,
    ls: Option<RiverLayerShellOutputV1>,
    pub rect: Rect,
    usable: Option<Rect>,
    pub tag: usize,
    pub prev_tag: Option<usize>,
    removed: bool,
    new: bool,
}

impl Monitor {
    pub fn usable(&self) -> Rect {
        self.usable.unwrap_or(self.rect)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum OpKind {
    Move,
    Resize { left: bool, top: bool },
    Zoom { sx: i32, sy: i32 },
}

#[derive(Clone, Debug)]
pub struct Op {
    pub kind: OpKind,
    pub win: WinId,
    pub start: Rect,
    /// the touch point driving the op, or None for the pointer
    touch: Option<i32>,
    dx: i32,
    dy: i32,
    released: bool,
    /// a touch op whose touch point turned out to be a mistake (a palm)
    canceled: bool,
}

pub struct Seat {
    pub obj: RiverSeatV1,
    pub oid: ObjectId,
    new: bool,
    removed: bool,
    ls: Option<RiverLayerShellSeatV1>,
    keys: HashMap<(u32, u32), RiverXkbBindingV1>,
    buttons: HashMap<(u32, u32), RiverPointerBindingV1>,
    tg: Option<RiverTouchGesturesSeatV1>,
    gestures: HashMap<Gesture, RiverTouchGestureV1>,
    pub hovered: Option<WinId>,
    entered: Option<WinId>,
    /// the pointer moved since the last manage sequence
    moved: bool,
    interacted: Option<WinId>,
    shell_interacted: Option<ObjectId>,
    pub pointer: (i32, i32),
    pub op: Option<Op>,
    /// focus last requested from river; None means unknown
    focus_sent: Option<Option<WinId>>,
}

pub struct KeyBind {
    pub mods: u32,
    pub sym: u32,
    pub combo: String,
    pub cmd: Vec<String>,
}

pub struct MouseBind {
    pub mods: u32,
    pub button: u32,
    pub combo: String,
    pub cmd: Vec<String>,
}

pub struct GestureBind {
    pub gesture: Gesture,
    pub cmd: Vec<String>,
}

pub enum Action {
    Cmd(Vec<String>),
    Mouse { seat: ObjectId, cmd: Vec<String> },
}

/// A solid rectangle drawn below the windows: a frame's background or border.
struct Deco {
    surface: WlSurface,
    viewport: WpViewport,
    shell: RiverShellSurfaceV1,
    node: RiverNodeV1,
    shown: Option<(Rect, Color)>,
    frame: Option<(usize, Vec<usize>)>,
}

#[derive(Clone)]
struct DecoPlan {
    rect: Rect,
    color: Color,
    tag: usize,
    path: Vec<usize>,
}

pub struct Wm {
    pub qh: QueueHandle<Wm>,
    pub river: Option<RiverWindowManagerV1>,
    xkb: Option<RiverXkbBindingsV1>,
    layer_shell: Option<RiverLayerShellV1>,
    compositor: Option<WlCompositor>,
    viewporter: Option<WpViewporter>,
    pixels: Option<WpSinglePixelBufferManagerV1>,
    activation: Option<HrwmActivationV1>,
    touch_gestures: Option<RiverTouchGesturesV1>,

    pub settings: Settings,
    pub rules: Rules,
    pub tags: Vec<Tag>,
    pub monitors: Vec<Monitor>,
    pub focused_mon: usize,
    pub wins: HashMap<WinId, Win>,
    by_obj: HashMap<ObjectId, WinId>,
    next_id: WinId,
    pub seats: Vec<Seat>,
    pub keybinds: Vec<KeyBind>,
    pub mousebinds: Vec<MouseBind>,
    pub gesturebinds: Vec<GestureBind>,
    pub binds_dirty: bool,
    pub actions: Vec<Action>,
    pub ipc_queue: Vec<(UnixStream, Vec<String>)>,
    /// responses the main loop finishes writing as their clients read
    pub replies: Vec<crate::ipc::Reply>,
    pub idle: Vec<UnixStream>,
    pub locked: bool,
    last_focus: Option<Option<WinId>>,
    last_mon: Option<usize>,
    registry: Option<wl_registry::WlRegistry>,
    /// global name -> version, for wl_outputs bound on demand
    output_globals: HashMap<u32, u32>,
    default_ls_output: Option<ObjectId>,
    stack: Vec<WinId>,
    decos: Vec<Deco>,
    deco_plan: Vec<DecoPlan>,
    buffers: HashMap<[u32; 4], WlBuffer>,
    pub keymaps: crate::keymap::Keymaps,
    pub inputs: crate::input::Inputs,
    /// state saved by the previous hrwm, applied in the first manage sequence
    restore: Option<crate::state::Saved>,
    /// saved output name -> tag name, applied once the output's name arrives
    restore_monitors: Vec<(String, String)>,
    restore_focus: Option<String>,
    /// what the state file holds
    saved_text: String,
    /// `wmexec`'s command, run once river has let go of this window manager
    pub exec: Option<Vec<String>>,
}

impl Wm {
    pub fn new(qh: QueueHandle<Wm>) -> Wm {
        let settings = Settings::default();
        let tree = Frame::new_leaf(settings.default_frame_layout);
        Wm {
            qh,
            river: None,
            xkb: None,
            layer_shell: None,
            compositor: None,
            viewporter: None,
            pixels: None,
            activation: None,
            touch_gestures: None,
            settings,
            rules: Rules::default(),
            tags: vec![Tag {
                name: "default".into(),
                tree,
                floats: Vec::new(),
                floating: false,
                float_focus: None,
                placeholder: false,
            }],
            monitors: Vec::new(),
            focused_mon: 0,
            wins: HashMap::new(),
            by_obj: HashMap::new(),
            next_id: 1,
            seats: Vec::new(),
            keybinds: Vec::new(),
            mousebinds: Vec::new(),
            gesturebinds: Vec::new(),
            binds_dirty: false,
            actions: Vec::new(),
            ipc_queue: Vec::new(),
            replies: Vec::new(),
            idle: Vec::new(),
            locked: false,
            last_focus: None,
            last_mon: None,
            registry: None,
            output_globals: HashMap::new(),
            default_ls_output: None,
            stack: Vec::new(),
            decos: Vec::new(),
            deco_plan: Vec::new(),
            buffers: HashMap::new(),
            keymaps: Default::default(),
            inputs: Default::default(),
            restore: crate::state::load(),
            restore_monitors: Vec::new(),
            restore_focus: None,
            saved_text: String::new(),
            exec: None,
        }
    }

    pub fn has_globals(&self) -> Result<(), String> {
        if self.river.is_none() {
            return Err(
                "river_window_manager_v1 (version 4+) not found; is this river 0.4 or later?"
                    .into(),
            );
        }
        if self.xkb.is_none() {
            return Err("river_xkb_bindings_v1 not found".into());
        }
        Ok(())
    }

    /// Ask river for a manage sequence, e.g. because a command arrived.
    pub fn dirty(&self) {
        if let Some(r) = &self.river {
            r.manage_dirty();
        }
    }

    // ----- hooks ---------------------------------------------------------

    pub fn emit(&mut self, fields: &[&str]) {
        let mut line = fields.join("\t");
        line.push('\n');
        self.idle
            .retain_mut(|s| s.write_all(line.as_bytes()).is_ok());
    }

    // ----- lookups -------------------------------------------------------

    pub fn cur_tag(&self) -> usize {
        self.monitors
            .get(self.focused_mon)
            .map(|m| m.tag)
            .unwrap_or(0)
    }

    pub fn tag_index(&self, name: &str) -> Option<usize> {
        self.tags.iter().position(|t| t.name == name)
    }

    pub fn monitor_of_tag(&self, tag: usize) -> Option<usize> {
        self.monitors.iter().position(|m| m.tag == tag)
    }

    pub fn focused_window(&self) -> Option<WinId> {
        self.tags.get(self.cur_tag()).and_then(|t| t.focused())
    }

    /// Area used to lay out the focused tag (or a hidden one).
    pub fn reference_rect(&self) -> Rect {
        self.monitors
            .get(self.focused_mon)
            .map(|m| m.usable())
            .unwrap_or(Rect::new(0, 0, 1280, 720))
    }

    pub fn parse_winid(&self, s: &str) -> Option<WinId> {
        let id = match s.strip_prefix("0x") {
            Some(h) => u32::from_str_radix(h, 16).ok()?,
            None => s.parse().ok()?,
        };
        self.wins.contains_key(&id).then_some(id)
    }

    // ----- window/tag bookkeeping ----------------------------------------

    pub fn add_tag(&mut self, name: &str) -> usize {
        if let Some(i) = self.tag_index(name) {
            return i;
        }
        self.tags.push(Tag {
            name: name.to_string(),
            tree: Frame::new_leaf(self.settings.default_frame_layout),
            floats: Vec::new(),
            floating: false,
            float_focus: None,
            placeholder: false,
        });
        self.emit(&["tag_added", name]);
        let mut idx = self.tags.len() - 1;
        // a monitor showing an empty placeholder tag takes the new tag instead
        let holder = self.monitors.iter().position(|m| {
            let t = &self.tags[m.tag];
            t.placeholder && t.windows().is_empty()
        });
        if let Some(m) = holder {
            let old = self.monitors[m].tag;
            self.monitors[m].tag = idx;
            self.remove_tag(old);
            idx -= 1;
            self.emit(&["tag_changed", name, &m.to_string()]);
        }
        idx
    }

    /// A tag for a new monitor when all tags are already shown.
    fn add_placeholder_tag(&mut self) -> usize {
        let mut n = self.tags.len() + 1;
        while self.tag_index(&n.to_string()).is_some() {
            n += 1;
        }
        let name = n.to_string();
        self.tags.push(Tag {
            name: name.clone(),
            tree: Frame::new_leaf(self.settings.default_frame_layout),
            floats: Vec::new(),
            floating: false,
            float_focus: None,
            placeholder: true,
        });
        self.emit(&["tag_added", &name]);
        self.tags.len() - 1
    }

    /// Delete a tag that holds no windows and isn't shown.
    pub fn remove_tag(&mut self, victim: usize) {
        let name = self.tags.remove(victim).name;
        for w in self.wins.values_mut() {
            if w.tag > victim {
                w.tag -= 1;
            }
        }
        for m in &mut self.monitors {
            if m.tag > victim {
                m.tag -= 1;
            }
            m.prev_tag = m
                .prev_tag
                .filter(|&p| p != victim)
                .map(|p| if p > victim { p - 1 } else { p });
        }
        self.emit(&["tag_removed", &name]);
    }

    /// Take a window out of its tag's tree or floating list.
    pub fn detach(&mut self, id: WinId) {
        let Some(w) = self.wins.get(&id) else { return };
        let tag = &mut self.tags[w.tag];
        tag.tree.remove_window(id);
        tag.floats.retain(|&x| x != id);
        if tag.float_focus == Some(id) {
            tag.float_focus = None;
        }
    }

    /// Put a window into a tag: floating windows go on top of the floating
    /// list, tiled ones into the tag's focused frame.
    pub fn attach(&mut self, id: WinId, tag: usize, focus: bool) {
        let floating = self.wins[&id].floating;
        self.wins.get_mut(&id).unwrap().tag = tag;
        let t = &mut self.tags[tag];
        if floating {
            t.floats.push(id);
            if focus {
                t.float_focus = Some(id);
            }
        } else {
            t.tree.insert(id, focus);
            if focus {
                t.float_focus = None;
            }
        }
    }

    /// Focus a window: switch to its monitor and select it in its frame.
    pub fn focus_win(&mut self, id: WinId) {
        let Some(w) = self.wins.get(&id) else { return };
        let (tag, floating) = (w.tag, w.floating);
        if let Some(m) = self.monitor_of_tag(tag) {
            self.focused_mon = m;
        }
        let t = &mut self.tags[tag];
        if floating {
            t.float_focus = Some(id);
            if self.settings.raise_on_focus {
                t.floats.retain(|&x| x != id);
                t.floats.push(id);
            }
        } else {
            t.tree.focus_window(id);
            t.float_focus = None;
        }
    }

    pub fn set_urgent(&mut self, id: WinId, on: bool) {
        let Some(w) = self.wins.get_mut(&id) else {
            return;
        };
        if w.urgent == on {
            return;
        }
        w.urgent = on;
        let hex = w.hex_id();
        self.emit(&["urgent", if on { "on" } else { "off" }, &hex]);
        self.emit(&["tag_flags"]);
    }

    /// The next urgent window after the focused one, in creation order.
    pub fn next_urgent(&self) -> Option<WinId> {
        let mut urgent: Vec<WinId> = self
            .wins
            .values()
            .filter(|w| w.urgent)
            .map(|w| w.id)
            .collect();
        urgent.sort();
        let after = self.focused_window().unwrap_or(0);
        urgent
            .iter()
            .find(|&&id| id > after)
            .or(urgent.first())
            .copied()
    }

    pub fn raise(&mut self, id: WinId) {
        let Some(w) = self.wins.get(&id) else { return };
        let t = &mut self.tags[w.tag];
        if t.floats.contains(&id) {
            t.floats.retain(|&x| x != id);
            t.floats.push(id);
        }
    }

    pub fn set_floating(&mut self, id: WinId, on: bool) {
        let Some(w) = self.wins.get(&id) else { return };
        if w.floating == on {
            return;
        }
        let (tag, place) = (w.tag, w.place);
        let focused = self.tags[tag].focused() == Some(id);
        self.detach(id);
        let w = self.wins.get_mut(&id).unwrap();
        w.floating = on;
        if on
            && w.float_rect.is_none()
            && let Place::Tiled { slot } = place
        {
            let content = slot.inset(self.settings.window_border_width);
            let (nw, nh) = w.natural.unwrap_or((content.w, content.h));
            let (cx, cy) = content.center();
            w.float_rect = Some(Rect::new(cx - nw / 2, cy - nh / 2, nw, nh));
        }
        self.attach(id, tag, focused);
    }

    /// Switch the focused monitor to a tag (`use`).
    pub fn use_tag(&mut self, tag: usize) {
        if self.monitors.is_empty() {
            return;
        }
        let m = self.focused_mon;
        if self.monitors[m].tag == tag {
            return;
        }
        if let Some(other) = self.monitor_of_tag(tag) {
            if self.settings.swap_monitors_to_get_tag {
                let mine = self.monitors[m].tag;
                self.monitors[other].tag = mine;
                self.monitors[other].prev_tag = Some(tag);
                let name = self.tags[mine].name.clone();
                self.emit(&["tag_changed", &name, &other.to_string()]);
            } else {
                self.focused_mon = other;
                let name = self.tags[tag].name.clone();
                self.emit(&["tag_changed", &name, &other.to_string()]);
                return;
            }
        }
        let mon = &mut self.monitors[m];
        mon.prev_tag = Some(mon.tag);
        mon.tag = tag;
        let name = self.tags[tag].name.clone();
        self.emit(&["tag_changed", &name, &m.to_string()]);
    }

    pub fn focus_monitor(&mut self, m: usize) {
        if m < self.monitors.len() && m != self.focused_mon {
            self.focused_mon = m;
            let name = self.tags[self.monitors[m].tag].name.clone();
            self.emit(&["tag_changed", &name, &m.to_string()]);
        }
    }

    /// The nearest monitor in a direction, by center position.
    pub fn monitor_in_dir(&self, dir: crate::tree::Dir) -> Option<usize> {
        use crate::tree::Dir;
        let (cx, cy) = self.monitors.get(self.focused_mon)?.rect.center();
        self.monitors
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.focused_mon)
            .filter_map(|(i, m)| {
                let (x, y) = m.rect.center();
                let ok = match dir {
                    Dir::Left => x < cx,
                    Dir::Right => x > cx,
                    Dir::Up => y < cy,
                    Dir::Down => y > cy,
                };
                ok.then_some((i, (x - cx).abs() + (y - cy).abs()))
            })
            .min_by_key(|&(_, d)| d)
            .map(|(i, _)| i)
    }

    pub fn start_op(&mut self, seat: usize, win: WinId, action: &str) {
        let Some(start) = self.op_start_rect(win) else {
            return;
        };
        let (px, py) = self.seats[seat].pointer;
        let (cx, cy) = start.center();
        let kind = match action {
            "move" => OpKind::Move,
            "resize" => OpKind::Resize {
                left: px < cx,
                top: py < cy,
            },
            "zoom" => OpKind::Zoom {
                sx: if px < cx { -1 } else { 1 },
                sy: if py < cy { -1 } else { 1 },
            },
            _ => return,
        };
        self.seats[seat].obj.op_start_pointer();
        self.begin_op(seat, win, kind, start, None);
    }

    /// A move (edges None) or resize the window asked for with a touch point.
    fn start_touch_op(&mut self, seat: usize, win: WinId, touch: i32, edges: Option<u32>) {
        if self.seats[seat].obj.version() < 6 {
            return;
        }
        let Some(start) = self.op_start_rect(win) else {
            return;
        };
        // no pointer to pick a corner from, so take the client's edges
        let kind = match edges {
            None => OpKind::Move,
            Some(e) => OpKind::Resize {
                left: e & river_window_v1::Edges::Left.bits() != 0,
                top: e & river_window_v1::Edges::Top.bits() != 0,
            },
        };
        self.seats[seat].obj.op_start_touch(touch);
        self.begin_op(seat, win, kind, start, Some(touch));
    }

    /// The floating geometry an op on `win` starts from, floating it first.
    fn op_start_rect(&mut self, win: WinId) -> Option<Rect> {
        let w = self.wins.get(&win)?;
        if w.fullscreen || !matches!(w.place, Place::Tiled { .. } | Place::Floating) {
            return None;
        }
        if !w.floating && !self.tags[w.tag].floating {
            self.set_floating(win, true);
        }
        let w = &self.wins[&win];
        match (w.float_rect, w.dims) {
            (Some(r), _) => Some(r),
            (None, Some((dw, dh))) => Some(Rect::new(0, 0, dw, dh)),
            _ => None,
        }
    }

    fn begin_op(&mut self, seat: usize, win: WinId, kind: OpKind, start: Rect, touch: Option<i32>) {
        if !matches!(kind, OpKind::Move) {
            self.wins[&win].obj.inform_resize_start();
        }
        self.seats[seat].op = Some(Op {
            kind,
            win,
            start,
            touch,
            dx: 0,
            dy: 0,
            released: false,
            canceled: false,
        });
        self.focus_win(win);
        self.raise(win);
    }

    /// End a seat's op early, e.g. because its window closed.
    fn end_op(seat: &mut Seat) {
        if let Some(op) = seat.op.take() {
            match op.touch {
                Some(t) => seat.obj.op_end_touch(t),
                None => seat.obj.op_end(),
            }
        }
    }

    // ----- the manage sequence -------------------------------------------

    fn manage(&mut self) {
        self.reap();
        if let Some(saved) = self.restore.take() {
            self.restore_state(saved);
        }
        self.setup_new_outputs();
        self.restore_monitor_tags();
        self.setup_new_seats();
        self.place_new_windows();
        self.handle_window_requests();
        self.handle_seat_events();

        let actions = std::mem::take(&mut self.actions);
        for a in actions {
            if self.locked {
                continue;
            }
            match a {
                Action::Cmd(cmd) => {
                    let mut out = String::new();
                    if let Err(e) = self.run(&cmd, &mut out) {
                        eprintln!("hrwm: {}: {e}", cmd.join(" "));
                    }
                }
                Action::Mouse { seat, cmd } => {
                    let Some(si) = self.seats.iter().position(|s| s.oid == seat) else {
                        continue;
                    };
                    if cmd.len() == 1 && matches!(cmd[0].as_str(), "move" | "resize" | "zoom") {
                        if self.seats[si].op.is_none()
                            && let Some(w) = self.seats[si].hovered
                        {
                            self.start_op(si, w, &cmd[0]);
                        }
                    } else {
                        let cmd = if cmd.first().map(String::as_str) == Some("call") {
                            cmd[1..].to_vec()
                        } else {
                            cmd
                        };
                        let mut out = String::new();
                        let _ = self.run(&cmd, &mut out);
                    }
                }
            }
        }

        for (stream, args) in std::mem::take(&mut self.ipc_queue) {
            let mut out = String::new();
            let code = match self.run(&args, &mut out) {
                Ok(()) => 0,
                Err(e) => {
                    out.push_str(&e);
                    if !e.ends_with('\n') {
                        out.push('\n');
                    }
                    1
                }
            };
            let mut reply = crate::ipc::Reply::new(stream, code, &out);
            if reply.poll() == Ok(false) {
                self.replies.push(reply);
            }
        }

        self.update_ops();
        if self.binds_dirty {
            self.sync_bindings();
        }
        self.arrange();
        self.update_focus();
        self.save_state();
    }

    // ----- state kept across restarts ------------------------------------

    /// Take back the previous hrwm's tags and windows. Only windows river
    /// still has are matched; a state file from an earlier river session
    /// matches none and is ignored.
    fn restore_state(&mut self, saved: crate::state::Saved) {
        let by_ident: HashMap<&str, WinId> = self
            .wins
            .values()
            .filter(|w| w.new && !w.identifier.is_empty())
            .map(|w| (w.identifier.as_str(), w.id))
            .collect();
        let ids: Vec<Option<WinId>> = saved
            .wins
            .iter()
            .map(|w| by_ident.get(w.identifier.as_str()).copied())
            .collect();
        if saved.tags.is_empty() || ids.iter().all(Option::is_none) {
            return;
        }
        let map = |n: WinId| ids.get(n as usize).copied().flatten();
        self.tags = saved
            .tags
            .into_iter()
            .map(|mut t| {
                crate::state::remap(&mut t.tree, &map);
                Tag {
                    name: t.name,
                    tree: t.tree,
                    floats: t.floats.into_iter().filter_map(map).collect(),
                    floating: t.floating,
                    float_focus: t.float_focus.and_then(map),
                    placeholder: false,
                }
            })
            .collect();
        for ti in 0..self.tags.len() {
            for id in self.tags[ti].windows() {
                let w = self.wins.get_mut(&id).unwrap();
                w.tag = ti;
                w.new = false;
                self.init_window(id);
            }
        }
        for (n, sw) in saved.wins.iter().enumerate() {
            if let Some(w) = ids[n].and_then(|id| self.wins.get_mut(&id))
                && !w.new
            {
                w.floating = sw.floating;
                w.pseudotile = sw.pseudotile;
                w.fullscreen = sw.fullscreen;
                w.float_rect = sw.float_rect;
            }
        }
        self.restore_monitors = saved.monitors;
        self.restore_focus = saved.focus;
        for t in 0..self.tags.len() {
            let name = self.tags[t].name.clone();
            self.emit(&["tag_added", &name]);
        }
        self.emit(&["tag_flags"]);
    }

    /// Show each output's saved tag on it once river has told us its name.
    fn restore_monitor_tags(&mut self) {
        for m in 0..self.monitors.len() {
            let name = self.monitors[m].name.clone();
            if name.is_empty() {
                continue;
            }
            if let Some(pos) = self.restore_monitors.iter().position(|(o, _)| *o == name) {
                let (_, tag) = self.restore_monitors.remove(pos);
                if let Some(t) = self.tag_index(&tag)
                    && self.monitors[m].tag != t
                {
                    // the monitor showing it takes this one's tag instead
                    if let Some(other) = self.monitor_of_tag(t) {
                        self.monitors[other].tag = self.monitors[m].tag;
                    }
                    self.monitors[m].tag = t;
                    self.emit(&["tag_changed", &tag, &m.to_string()]);
                }
            }
            if self.restore_focus.as_ref() == Some(&name) {
                self.restore_focus = None;
                self.focused_mon = m;
            }
        }
    }

    /// Write the state file if anything in it changed. Skipped while a
    /// window is being dragged, which changes it every frame.
    pub fn save_state(&mut self) {
        if self.seats.iter().any(|s| s.op.is_some()) {
            return;
        }
        let mut order: Vec<WinId> = self
            .wins
            .values()
            .filter(|w| !w.new && !w.identifier.is_empty())
            .map(|w| w.id)
            .collect();
        order.sort();
        let num: HashMap<WinId, WinId> = order
            .iter()
            .enumerate()
            .map(|(n, &id)| (id, n as WinId))
            .collect();
        let map = |id: WinId| num.get(&id).copied();
        let mut saved = crate::state::Saved {
            wins: order
                .iter()
                .map(|id| {
                    let w = &self.wins[id];
                    crate::state::SavedWin {
                        identifier: w.identifier.clone(),
                        floating: w.floating,
                        pseudotile: w.pseudotile,
                        fullscreen: w.fullscreen,
                        float_rect: w.float_rect,
                    }
                })
                .collect(),
            tags: self
                .tags
                .iter()
                .map(|t| {
                    let mut tree = t.tree.clone();
                    crate::state::remap(&mut tree, &map);
                    crate::state::SavedTag {
                        name: t.name.clone(),
                        floating: t.floating,
                        float_focus: t.float_focus.and_then(map),
                        floats: t.floats.iter().filter_map(|&w| map(w)).collect(),
                        tree,
                    }
                })
                .collect(),
            monitors: self
                .monitors
                .iter()
                .filter(|m| !m.name.is_empty())
                .map(|m| (m.name.clone(), self.tags[m.tag].name.clone()))
                .collect(),
            focus: self
                .monitors
                .get(self.focused_mon)
                .map(|m| m.name.clone())
                .filter(|n| !n.is_empty()),
        };
        // outputs not seen since the restart keep their saved tag
        for (o, t) in &self.restore_monitors {
            if !saved.monitors.iter().any(|(x, _)| x == o) {
                saved.monitors.push((o.clone(), t.clone()));
            }
        }
        let text = saved.to_text();
        if text != self.saved_text {
            crate::state::store(&text);
            self.saved_text = text;
        }
    }

    /// Destroy closed windows and removed outputs/seats.
    fn reap(&mut self) {
        let closed: Vec<WinId> = self
            .wins
            .values()
            .filter(|w| w.closed)
            .map(|w| w.id)
            .collect();
        for id in closed {
            self.detach(id);
            let w = self.wins.remove(&id).unwrap();
            self.by_obj.remove(&w.obj.id());
            for s in &mut self.seats {
                if s.op.as_ref().is_some_and(|op| op.win == id) {
                    Self::end_op(s);
                }
                for f in [&mut s.hovered, &mut s.entered, &mut s.interacted] {
                    if *f == Some(id) {
                        *f = None;
                    }
                }
                if s.focus_sent == Some(Some(id)) {
                    s.focus_sent = None;
                }
            }
            w.node.destroy();
            w.obj.destroy();
            self.emit(&["tag_flags"]);
        }

        let mut i = 0;
        while i < self.monitors.len() {
            if self.monitors[i].removed {
                let m = self.monitors.remove(i);
                if let Some(ls) = m.ls {
                    ls.destroy();
                }
                m.obj.destroy();
                if self.default_ls_output.as_ref() == Some(&m.oid) {
                    self.default_ls_output = None;
                }
                if self.focused_mon > i || self.focused_mon >= self.monitors.len() {
                    self.focused_mon = self.focused_mon.saturating_sub(1);
                }
            } else {
                i += 1;
            }
        }

        self.seats.retain(|s| {
            if s.removed {
                for b in s.keys.values() {
                    b.destroy();
                }
                for b in s.buttons.values() {
                    b.destroy();
                }
                if let Some(ls) = &s.ls {
                    ls.destroy();
                }
                for g in s.gestures.values() {
                    g.destroy();
                }
                if let Some(tg) = &s.tg {
                    tg.destroy();
                }
                s.obj.destroy();
            }
            !s.removed
        });
    }

    fn setup_new_outputs(&mut self) {
        for i in 0..self.monitors.len() {
            if !self.monitors[i].new {
                continue;
            }
            self.monitors[i].new = false;
            let used: Vec<usize> = self
                .monitors
                .iter()
                .enumerate()
                .filter(|(j, m)| *j != i && !m.new)
                .map(|(_, m)| m.tag)
                .collect();
            let tag = match (0..self.tags.len()).find(|t| !used.contains(t)) {
                Some(t) => t,
                None => self.add_placeholder_tag(),
            };
            let m = &mut self.monitors[i];
            m.tag = tag;
            if let Some(ls) = &self.layer_shell {
                m.ls = Some(ls.get_output(&m.obj, &self.qh, m.oid.clone()));
            }
            self.emit(&["tag_changed", &self.tags[tag].name.clone(), &i.to_string()]);
        }
    }

    fn setup_new_seats(&mut self) {
        for s in &mut self.seats {
            if s.new {
                s.new = false;
                if let Some(ls) = &self.layer_shell {
                    s.ls = Some(ls.get_seat(&s.obj, &self.qh, s.oid.clone()));
                }
                if let Some(tg) = &self.touch_gestures {
                    s.tg = Some(tg.get_seat(&s.obj, &self.qh, ()));
                }
                if let Ok(theme) = std::env::var("XCURSOR_THEME") {
                    let size = std::env::var("XCURSOR_SIZE")
                        .ok()
                        .and_then(|s| s.parse().ok());
                    s.obj.set_xcursor_theme(theme, size.unwrap_or(24));
                }
                self.binds_dirty = true;
            }
        }
    }

    /// What every window gets from us once, new or restored.
    fn init_window(&mut self, id: WinId) {
        let w = self.wins.get_mut(&id).unwrap();
        w.obj
            .set_capabilities(river_window_v1::Capabilities::Fullscreen);
        if !w.only_csd {
            w.obj.use_ssd();
            w.ssd_sent = true;
        }
    }

    fn place_new_windows(&mut self) {
        let mut new: Vec<WinId> = self.wins.values().filter(|w| w.new).map(|w| w.id).collect();
        new.sort();
        for id in new {
            self.init_window(id);
            let w = self.wins.get_mut(&id).unwrap();
            w.new = false;
            let props = Props {
                app_id: &w.app_id,
                title: &w.title,
                has_parent: w.parent.is_some(),
                pid: w.pid,
            };
            let out = self.rules.apply(&props);
            w.floating = out.floating.unwrap_or(false) || out.manage == Some(false);
            w.pseudotile = out.pseudotile.unwrap_or(false);
            w.fullscreen = out.fullscreen.unwrap_or(false);
            let mut tag = self.cur_tag();
            if let Some(t) = out.tag.as_deref().and_then(|n| self.tag_index(n)) {
                tag = t;
            } else if let Some(m) = out.monitor.as_deref().and_then(|m| m.parse::<usize>().ok())
                && let Some(mon) = self.monitors.get(m)
            {
                tag = mon.tag;
            }
            let focus = out.focus.unwrap_or(false);
            self.attach(id, tag, focus);
            if focus {
                if self.monitor_of_tag(tag).is_none() && out.switchtag == Some(true) {
                    self.use_tag(tag);
                }
                if let Some(m) = self.monitor_of_tag(tag) {
                    self.focused_mon = m;
                }
            }
            self.emit(&["tag_flags"]);
        }
    }

    fn handle_window_requests(&mut self) {
        let ids: Vec<WinId> = self.wins.keys().copied().collect();
        for id in ids {
            let w = self.wins.get_mut(&id).unwrap();
            if let Some(on) = w.req_fullscreen.take()
                && w.fullscreen != on
            {
                w.fullscreen = on;
                let hex = w.hex_id();
                self.emit(&["fullscreen", if on { "on" } else { "off" }, &hex]);
            }
            let w = self.wins.get_mut(&id).unwrap();
            let mv = w.req_move.take();
            let rs = w.req_resize.take();
            if let Some(seat) = mv
                && let Some(si) = self.seats.iter().position(|s| s.oid == seat)
                && self.seats[si].op.is_none()
            {
                self.start_op(si, id, "move");
            }
            if let Some((seat, _edges)) = rs
                && let Some(si) = self.seats.iter().position(|s| s.oid == seat)
                && self.seats[si].op.is_none()
            {
                self.start_op(si, id, "resize");
            }
            let w = self.wins.get_mut(&id).unwrap();
            let tmv = w.req_touch_move.take();
            let trs = w.req_touch_resize.take();
            if let Some((seat, touch)) = tmv
                && let Some(si) = self.seats.iter().position(|s| s.oid == seat)
                && self.seats[si].op.is_none()
            {
                self.start_touch_op(si, id, touch, None);
            }
            if let Some((seat, touch, edges)) = trs
                && let Some(si) = self.seats.iter().position(|s| s.oid == seat)
                && self.seats[si].op.is_none()
            {
                self.start_touch_op(si, id, touch, Some(edges));
            }
        }
    }

    fn handle_seat_events(&mut self) {
        for si in 0..self.seats.len() {
            let entered = self.seats[si].entered.take();
            // like herbstluftwm's drop_enternotify_events: a window ending up
            // under a resting pointer (after a relayout or tag switch) must
            // not steal focus
            let moved = std::mem::take(&mut self.seats[si].moved);
            let clicked = self.seats[si].interacted.take();
            let shell = self.seats[si].shell_interacted.take();
            let busy = self.seats[si].op.is_some();
            if let Some(id) = clicked {
                self.focus_win(id);
                if self.settings.raise_on_click {
                    self.raise(id);
                }
            } else if let (Some(id), false, true) = (entered, busy, moved)
                && self.settings.focus_follows_mouse
                && self.wins.get(&id).is_some_and(|w| w.place != Place::Hidden)
            {
                self.focus_win(id);
            }
            if let Some(oid) = shell
                && let Some((tag, path)) = self
                    .decos
                    .iter()
                    .find(|d| d.shell.id() == oid)
                    .and_then(|d| d.frame.clone())
            {
                if let Some(m) = self.monitor_of_tag(tag) {
                    self.focused_mon = m;
                }
                if tag < self.tags.len() {
                    let t = &mut self.tags[tag];
                    if t.tree.leaf_count() > 0 && path_exists(&t.tree, &path) {
                        t.tree.focus_frame(&path);
                        t.float_focus = None;
                    }
                }
            }
        }
    }

    fn update_ops(&mut self) {
        for si in 0..self.seats.len() {
            let Some(op) = self.seats[si].op.clone() else {
                continue;
            };
            let Some(w) = self.wins.get_mut(&op.win) else {
                self.seats[si].op = None;
                continue;
            };
            if op.released || op.canceled {
                // river ends a touch op itself when the touch point lifts
                if op.touch.is_none() {
                    self.seats[si].obj.op_end();
                }
                if !matches!(op.kind, OpKind::Move) {
                    w.obj.inform_resize_end();
                }
                if op.canceled {
                    w.float_rect = Some(op.start);
                }
                self.seats[si].op = None;
                continue;
            }
            let s = op.start;
            let r = match op.kind {
                OpKind::Move => Rect::new(s.x + op.dx, s.y + op.dy, s.w, s.h),
                OpKind::Resize { left, top } => {
                    let nw = (if left { s.w - op.dx } else { s.w + op.dx }).max(20);
                    let nh = (if top { s.h - op.dy } else { s.h + op.dy }).max(20);
                    let x = if left { s.x + s.w - nw } else { s.x };
                    let y = if top { s.y + s.h - nh } else { s.y };
                    Rect::new(x, y, nw, nh)
                }
                OpKind::Zoom { sx, sy } => {
                    let nw = (s.w + 2 * sx * op.dx).max(20);
                    let nh = (s.h + 2 * sy * op.dy).max(20);
                    let (cx, cy) = s.center();
                    Rect::new(cx - nw / 2, cy - nh / 2, nw, nh)
                }
            };
            w.float_rect = Some(r);
        }
    }

    /// Create and destroy river binding objects to match the keybind and
    /// mousebind lists.
    fn sync_bindings(&mut self) {
        self.binds_dirty = false;
        let (Some(xkb), qh) = (&self.xkb, &self.qh) else {
            return;
        };
        let keys: Vec<(u32, u32)> = self.keybinds.iter().map(|k| (k.mods, k.sym)).collect();
        let buttons: Vec<(u32, u32)> = self.mousebinds.iter().map(|b| (b.mods, b.button)).collect();
        let gestures: Vec<Gesture> = self.gesturebinds.iter().map(|g| g.gesture).collect();
        for s in &mut self.seats {
            s.gestures.retain(|g, o| {
                let keep = gestures.contains(g);
                if !keep {
                    o.destroy();
                }
                keep
            });
            if let Some(tg) = &s.tg {
                for &g in &gestures {
                    s.gestures
                        .entry(g)
                        .or_insert_with(|| new_gesture(tg, g, qh));
                }
            }
            s.keys.retain(|k, b| {
                let keep = keys.contains(k);
                if !keep {
                    b.destroy();
                }
                keep
            });
            for &(mods, sym) in &keys {
                s.keys.entry((mods, sym)).or_insert_with(|| {
                    let b = xkb.get_xkb_binding(
                        &s.obj,
                        sym,
                        river_seat_v1::Modifiers::from_bits_retain(mods),
                        qh,
                        (mods, sym),
                    );
                    b.enable();
                    b
                });
            }
            s.buttons.retain(|k, b| {
                let keep = buttons.contains(k);
                if !keep {
                    b.destroy();
                }
                keep
            });
            for &(mods, button) in &buttons {
                s.buttons.entry((mods, button)).or_insert_with(|| {
                    let b = s.obj.get_pointer_binding(
                        button,
                        river_seat_v1::Modifiers::from_bits_retain(mods),
                        qh,
                        (s.oid.clone(), mods, button),
                    );
                    b.enable();
                    b
                });
            }
        }
    }

    /// Compute where every window goes, send sizes and fullscreen state, and
    /// plan stacking order and frame decorations for the render sequence.
    fn arrange(&mut self) {
        let g = self.settings.geometry();
        let bw = self.settings.window_border_width;
        let reference = self.reference_rect();
        let mut stack = Vec::new();
        let mut plan = Vec::new();
        let mut fullscreen_top = Vec::new();

        for ti in 0..self.tags.len() {
            let mon = self.monitor_of_tag(ti);
            let area = mon.map(|m| self.monitors[m].usable()).unwrap_or(reference);
            let visible = mon.is_some();
            let tag = &self.tags[ti];
            let (tiles, frames) = tag.tree.compute(apply_monitor_gap(area, &g), &g);
            let tag_floating = tag.floating;
            let focused = tag.focused();

            for t in &tiles {
                let Some(w) = self.wins.get_mut(&t.win) else {
                    continue;
                };
                if tag_floating {
                    if w.float_rect.is_none() {
                        w.float_rect = Some(t.rect.inset(bw));
                    }
                    w.place = if visible {
                        Place::Floating
                    } else {
                        Place::Hidden
                    };
                } else {
                    w.place = if visible && t.visible {
                        Place::Tiled { slot: t.rect }
                    } else {
                        Place::Hidden
                    };
                }
                let size = if tag_floating {
                    w.float_rect.map(|r| (r.w, r.h)).unwrap_or((0, 0))
                } else {
                    let c = t.rect.inset(bw);
                    let (cw, ch) = (c.w.max(1), c.h.max(1));
                    if w.pseudotile {
                        match w.natural.or(w.float_rect.map(|r| (r.w, r.h))) {
                            Some((nw, nh)) => (nw.min(cw), nh.min(ch)),
                            None => (0, 0),
                        }
                    } else {
                        (cw, ch)
                    }
                };
                propose(w, size);
                set_tiled(w, if tag_floating { 0 } else { 15 });
            }
            // tiled windows in tree order; in a max frame only the selected
            // one is visible anyway
            let mut order: Vec<WinId> = tiles.iter().map(|t| t.win).collect();
            if let Some(f) = focused
                && order.contains(&f)
                && tag_floating
            {
                order.retain(|&x| x != f);
                order.push(f);
            }
            for &id in &tag.floats {
                let Some(w) = self.wins.get_mut(&id) else {
                    continue;
                };
                w.place = if visible {
                    Place::Floating
                } else {
                    Place::Hidden
                };
                let size = w.float_rect.map(|r| (r.w, r.h)).unwrap_or((0, 0));
                propose(w, size);
                set_tiled(w, 0);
                order.push(id);
            }

            // fullscreen
            let output = mon.map(|m| (self.monitors[m].obj.clone(), self.monitors[m].oid.clone()));
            for &id in &order {
                let w = self.wins.get_mut(&id).unwrap();
                let want = w.fullscreen && w.place != Place::Hidden;
                match (&output, want) {
                    (Some((obj, oid)), true) => {
                        if w.fs_output.as_ref() != Some(oid) {
                            w.obj.fullscreen(obj);
                            w.obj.inform_fullscreen();
                            w.fs_output = Some(oid.clone());
                        }
                        w.place = Place::Fullscreen;
                    }
                    _ => {
                        if w.fs_output.take().is_some() {
                            w.obj.exit_fullscreen();
                            w.obj.inform_not_fullscreen();
                            // the window needs a size again
                            let size = w.proposed.take().unwrap_or((0, 0));
                            propose(w, size);
                        }
                    }
                }
            }

            if visible {
                for &id in &order {
                    if self.wins[&id].place == Place::Fullscreen {
                        fullscreen_top.push(id);
                    } else {
                        stack.push(id);
                    }
                }
            }

            // frame decorations
            if visible && !tag_floating {
                let s = &self.settings;
                for f in &frames {
                    if !s
                        .show_frame_decorations
                        .shows(f.has_clients, f.focused, f.is_root)
                    {
                        continue;
                    }
                    let (border, bg, opacity) = if f.focused {
                        (
                            s.frame_border_active_color,
                            s.frame_bg_active_color,
                            s.frame_active_opacity,
                        )
                    } else {
                        (
                            s.frame_border_normal_color,
                            s.frame_bg_normal_color,
                            s.frame_normal_opacity,
                        )
                    };
                    let fbw = s.frame_border_width;
                    if fbw > 0 {
                        plan.push(DecoPlan {
                            rect: f.content.inset(-fbw),
                            color: border.with_opacity(opacity),
                            tag: ti,
                            path: f.path.clone(),
                        });
                    }
                    if !s.frame_bg_transparent {
                        plan.push(DecoPlan {
                            rect: f.content,
                            color: bg.with_opacity(opacity),
                            tag: ti,
                            path: f.path.clone(),
                        });
                    }
                }
            }
        }

        // focused window on top within its layer
        stack.extend(fullscreen_top);
        self.stack = stack;
        self.deco_plan = plan;

        // layer surfaces (fuzzel, notifications) open on the focused monitor
        if let Some(m) = self.monitors.get(self.focused_mon)
            && self.default_ls_output.as_ref() != Some(&m.oid)
            && let Some(ls) = &m.ls
        {
            ls.set_default();
            self.default_ls_output = Some(m.oid.clone());
        }
    }

    fn update_focus(&mut self) {
        let want = self
            .focused_window()
            .filter(|id| self.wins[id].place != Place::Hidden);
        for s in &mut self.seats {
            if s.focus_sent == Some(want) {
                continue;
            }
            match want {
                Some(id) => s.obj.focus_window(&self.wins[&id].obj),
                None => s.obj.clear_focus(),
            }
            s.focus_sent = Some(want);
        }
        // bars show which monitor has focus
        if self.last_mon != Some(self.focused_mon) && !self.monitors.is_empty() {
            self.last_mon = Some(self.focused_mon);
            let name = self.tags[self.cur_tag()].name.clone();
            self.emit(&["tag_changed", &name, &self.focused_mon.to_string()]);
        }
        if let Some(id) = want {
            self.set_urgent(id, false);
        }
        if self.last_focus != Some(want) {
            self.last_focus = Some(want);
            match want {
                Some(id) => {
                    let (hex, title) = (self.wins[&id].hex_id(), self.wins[&id].title.clone());
                    self.emit(&["focus_changed", &hex, &title]);
                }
                None => self.emit(&["focus_changed", "0x0", ""]),
            }
        }
    }

    // ----- the render sequence -------------------------------------------

    fn render(&mut self) {
        let bw = self.settings.window_border_width;
        let focused = self.focused_window();
        let mut new_floats = Vec::new();
        for w in self.wins.values_mut() {
            match w.place {
                Place::Hidden => w.obj.hide(),
                Place::Fullscreen => w.obj.show(),
                Place::Tiled { slot } => {
                    w.obj.show();
                    let c = slot.inset(bw);
                    let (dw, dh) = w.dims.unwrap_or((c.w, c.h));
                    let (x, y) = if w.pseudotile {
                        (c.x + (c.w - dw) / 2, c.y + (c.h - dh) / 2)
                    } else {
                        (c.x, c.y)
                    };
                    w.node.set_position(x, y);
                }
                Place::Floating => {
                    w.obj.show();
                    match w.float_rect {
                        Some(r) => w.node.set_position(r.x, r.y),
                        None => {
                            // first time shown: center on its monitor once the
                            // window has picked a size
                            if let Some((dw, dh)) = w.dims {
                                new_floats.push((w.id, dw, dh));
                            }
                        }
                    }
                }
            }
            let color = if Some(w.id) == focused {
                self.settings.window_border_active_color
            } else if w.urgent {
                self.settings.window_border_urgent_color
            } else {
                self.settings.window_border_normal_color
            };
            let [r, g, b, a] = color.premultiplied_u32();
            let edges = if bw > 0 {
                river_window_v1::Edges::all()
            } else {
                river_window_v1::Edges::empty()
            };
            w.obj.set_borders(edges, bw, r, g, b, a);
        }
        for (id, dw, dh) in new_floats {
            let area = self
                .monitor_of_tag(self.wins[&id].tag)
                .map(|m| self.monitors[m].usable())
                .unwrap_or(self.reference_rect());
            let (cx, cy) = area.center();
            let w = self.wins.get_mut(&id).unwrap();
            let r = Rect::new(cx - dw / 2, cy - dh / 2, dw, dh);
            w.float_rect = Some(r);
            w.node.set_position(r.x, r.y);
        }
        for id in &self.stack {
            self.wins[id].node.place_top();
        }
        self.render_decorations();
    }

    fn render_decorations(&mut self) {
        let (Some(comp), Some(vp), Some(px), Some(river)) = (
            &self.compositor,
            &self.viewporter,
            &self.pixels,
            &self.river,
        ) else {
            return;
        };
        while self.decos.len() < self.deco_plan.len() {
            let surface = comp.create_surface(&self.qh, ());
            let viewport = vp.get_viewport(&surface, &self.qh, ());
            let shell = river.get_shell_surface(&surface, &self.qh, ());
            let node = shell.get_node(&self.qh, ());
            self.decos.push(Deco {
                surface,
                viewport,
                shell,
                node,
                shown: None,
                frame: None,
            });
        }
        for (i, d) in self.decos.iter_mut().enumerate() {
            let want = self.deco_plan.get(i);
            d.frame = want.map(|p| (p.tag, p.path.clone()));
            let state = want.map(|p| (p.rect, p.color));
            if d.shown == state {
                continue;
            }
            d.shell.sync_next_commit();
            match want {
                Some(p) => {
                    let key = p.color.premultiplied_u32();
                    let buf = self.buffers.entry(key).or_insert_with(|| {
                        px.create_u32_rgba_buffer(key[0], key[1], key[2], key[3], &self.qh, ())
                    });
                    d.surface.attach(Some(buf), 0, 0);
                    d.viewport.set_destination(p.rect.w.max(1), p.rect.h.max(1));
                    d.node.set_position(p.rect.x, p.rect.y);
                    d.surface.damage_buffer(0, 0, 1, 1);
                }
                None => d.surface.attach(None, 0, 0),
            }
            d.surface.commit();
            d.shown = state;
        }
        // first planned decoration at the very bottom
        for d in self.decos.iter().take(self.deco_plan.len()).rev() {
            d.node.place_bottom();
        }
    }
}

fn path_exists(tree: &Frame, path: &[usize]) -> bool {
    let mut f = tree;
    for &i in path {
        match f {
            Frame::Split(s) => f = if i == 0 { &s.a } else { &s.b },
            Frame::Leaf(_) => return false,
        }
    }
    matches!(f, Frame::Leaf(_))
}

fn propose(w: &mut Win, size: (i32, i32)) {
    if w.fs_output.is_some() || w.proposed == Some(size) {
        return;
    }
    w.obj.propose_dimensions(size.0, size.1);
    w.proposed = Some(size);
}

fn set_tiled(w: &mut Win, edges: u32) {
    if w.tiled_edges != Some(edges) {
        w.obj
            .set_tiled(river_window_v1::Edges::from_bits_retain(edges));
        w.tiled_edges = Some(edges);
    }
}

// ----- Wayland dispatch --------------------------------------------------

/// Create and enable river's recogniser for a gesture.
fn new_gesture(
    tg: &RiverTouchGesturesSeatV1,
    g: Gesture,
    qh: &QueueHandle<Wm>,
) -> RiverTouchGestureV1 {
    use river_touch_gesture_v1::{Direction, Edge};
    let dir = |d: Dir| match d {
        Dir::Up => Direction::Up,
        Dir::Down => Direction::Down,
        Dir::Left => Direction::Left,
        Dir::Right => Direction::Right,
    };
    let o = tg.get_gesture(g.fingers(), qh, g);
    match g {
        Gesture::Swipe { dir: d, .. } => o.set_direction(dir(d), gesture::SWIPE_DISTANCE),
        // river starts a pinch once the scale is at or past either bound
        Gesture::Pinch { out: false, .. } => o.set_threshold_scale(gesture::PINCH_IN, f64::MAX),
        Gesture::Pinch { out: true, .. } => o.set_threshold_scale(0.0, gesture::PINCH_OUT),
        Gesture::Edge(e) => {
            let edge = match e {
                Dir::Up => Edge::Top,
                Dir::Down => Edge::Bottom,
                Dir::Left => Edge::Left,
                Dir::Right => Edge::Right,
            };
            o.set_edge(edge, gesture::EDGE_DISTANCE);
            o.set_direction(dir(e.inward()), gesture::SWIPE_DISTANCE);
        }
    }
    o.enable();
    o
}

impl Dispatch<RiverTouchGestureV1, Gesture> for Wm {
    fn event(
        state: &mut Self,
        _: &RiverTouchGestureV1,
        event: river_touch_gesture_v1::Event,
        g: &Gesture,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let river_touch_gesture_v1::Event::Start = event
            && let Some(b) = state.gesturebinds.iter().find(|b| b.gesture == *g)
        {
            state.actions.push(Action::Cmd(b.cmd.clone()));
        }
    }
}

fn edge_bits(edges: wayland_client::WEnum<river_window_v1::Edges>) -> u32 {
    match edges {
        wayland_client::WEnum::Value(e) => e.bits(),
        wayland_client::WEnum::Unknown(u) => u,
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Wm {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        state.registry.get_or_insert_with(|| registry.clone());
        // river's globals at the newest version both river and protocol/
        // know, so one build works with river 0.4.x and 0.5
        macro_rules! river {
            ($i:ty) => {
                Some(registry.bind::<$i, _, _>(
                    name,
                    version.min(<$i>::interface().version),
                    qh,
                    (),
                ))
            };
        }
        match interface.as_str() {
            "wl_output" => {
                state.output_globals.insert(name, version);
            }
            "river_window_manager_v1" if version >= 4 => {
                state.river = river!(RiverWindowManagerV1);
            }
            "river_xkb_bindings_v1" => state.xkb = river!(RiverXkbBindingsV1),
            "river_layer_shell_v1" => state.layer_shell = river!(RiverLayerShellV1),
            "wl_compositor" if version >= 4 => {
                state.compositor = Some(registry.bind(name, 4, qh, ()))
            }
            "wp_viewporter" => state.viewporter = Some(registry.bind(name, 1, qh, ())),
            "river_input_manager_v1" => {
                state.keymaps.input =
                    river!(crate::protocol::river_input_manager_v1::RiverInputManagerV1)
            }
            "river_xkb_config_v1" => {
                state.keymaps.config =
                    river!(crate::protocol::river_xkb_config_v1::RiverXkbConfigV1)
            }
            "river_libinput_config_v1" => {
                state.inputs.config =
                    river!(crate::protocol::river_libinput_config_v1::RiverLibinputConfigV1)
            }
            "wp_single_pixel_buffer_manager_v1" => {
                state.pixels = Some(registry.bind(name, 1, qh, ()))
            }
            "hrwm_activation_v1" => state.activation = river!(HrwmActivationV1),
            "river_touch_gestures_v1" => state.touch_gestures = river!(RiverTouchGesturesV1),
            _ => {}
        }
    }
}

impl Dispatch<RiverWindowManagerV1, ()> for Wm {
    fn event(
        state: &mut Self,
        proxy: &RiverWindowManagerV1,
        event: river_window_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use river_window_manager_v1::Event;
        match event {
            Event::Unavailable => {
                eprintln!("hrwm: another window manager is already running");
                std::process::exit(1);
            }
            Event::Finished => {
                if let Some(cmd) = state.exec.take() {
                    state.save_state();
                    crate::commands::wmexec(&cmd);
                }
                std::process::exit(0)
            }
            Event::ManageStart => {
                state.manage();
                proxy.manage_finish();
            }
            Event::RenderStart => {
                state.render();
                proxy.render_finish();
            }
            Event::SessionLocked => state.locked = true,
            Event::SessionUnlocked => {
                state.locked = false;
                for s in &mut state.seats {
                    s.focus_sent = None;
                }
            }
            Event::Window { id } => {
                let wid = state.next_id;
                state.next_id += 1;
                let node = id.get_node(&state.qh, ());
                state.by_obj.insert(id.id(), wid);
                state.wins.insert(
                    wid,
                    Win {
                        id: wid,
                        identifier: String::new(),
                        obj: id,
                        node,
                        app_id: String::new(),
                        title: String::new(),
                        parent: None,
                        pid: 0,
                        new: true,
                        closed: false,
                        tag: 0,
                        floating: false,
                        pseudotile: false,
                        fullscreen: false,
                        urgent: false,
                        fs_output: None,
                        float_rect: None,
                        natural: None,
                        dims: None,
                        proposed: None,
                        tiled_edges: None,
                        only_csd: false,
                        ssd_sent: false,
                        req_fullscreen: None,
                        req_move: None,
                        req_resize: None,
                        req_touch_move: None,
                        req_touch_resize: None,
                        place: Place::Hidden,
                    },
                );
            }
            Event::Output { id } => state.monitors.push(Monitor {
                oid: id.id(),
                wl: None,
                name: String::new(),
                obj: id,
                ls: None,
                rect: Rect::default(),
                usable: None,
                tag: 0,
                prev_tag: None,
                removed: false,
                new: true,
            }),
            Event::Seat { id } => state.seats.push(Seat {
                oid: id.id(),
                obj: id,
                new: true,
                removed: false,
                ls: None,
                keys: HashMap::new(),
                buttons: HashMap::new(),
                tg: None,
                gestures: HashMap::new(),
                hovered: None,
                entered: None,
                moved: false,
                interacted: None,
                shell_interacted: None,
                pointer: (0, 0),
                op: None,
                focus_sent: None,
            }),
        }
    }

    wayland_client::event_created_child!(Wm, RiverWindowManagerV1, [
        river_window_manager_v1::EVT_WINDOW_OPCODE => (RiverWindowV1, ()),
        river_window_manager_v1::EVT_OUTPUT_OPCODE => (RiverOutputV1, ()),
        river_window_manager_v1::EVT_SEAT_OPCODE => (RiverSeatV1, ()),
    ]);
}

impl Dispatch<RiverWindowV1, ()> for Wm {
    fn event(
        state: &mut Self,
        proxy: &RiverWindowV1,
        event: river_window_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use river_window_v1::Event;
        let Some(&id) = state.by_obj.get(&proxy.id()) else {
            return;
        };
        let parent_id = |p: &Option<RiverWindowV1>| p.as_ref().map(|p| p.id());
        let mut title_changed = false;
        let mut natural_found = false;
        {
            let w = state.wins.get_mut(&id).unwrap();
            match event {
                Event::Closed => w.closed = true,
                Event::Dimensions { width, height } => {
                    w.dims = Some((width, height));
                    if w.natural.is_none() && w.proposed == Some((0, 0)) {
                        w.natural = Some((width, height));
                        natural_found = w.pseudotile;
                    }
                }
                Event::AppId { app_id } => w.app_id = app_id.unwrap_or_default(),
                Event::Title { title } => {
                    w.title = title.unwrap_or_default();
                    title_changed = true;
                }
                Event::Parent { parent } => w.parent = parent_id(&parent),
                Event::DecorationHint { hint } => {
                    w.only_csd = matches!(
                        hint,
                        wayland_client::WEnum::Value(
                            river_window_v1::DecorationHint::OnlySupportsCsd
                        )
                    );
                }
                Event::PointerMoveRequested { seat } => w.req_move = Some(seat.id()),
                Event::PointerResizeRequested { seat, edges } => {
                    w.req_resize = Some((seat.id(), edge_bits(edges)));
                }
                Event::TouchMoveRequested { seat, touch_point } => {
                    w.req_touch_move = Some((seat.id(), touch_point));
                }
                Event::TouchResizeRequested {
                    seat,
                    touch_point,
                    edges,
                } => w.req_touch_resize = Some((seat.id(), touch_point, edge_bits(edges))),
                Event::FullscreenRequested { .. } => w.req_fullscreen = Some(true),
                Event::ExitFullscreenRequested => w.req_fullscreen = Some(false),
                Event::UnreliablePid { unreliable_pid } => w.pid = unreliable_pid,
                Event::Identifier { identifier } => w.identifier = identifier,
                _ => {}
            }
        }
        if natural_found {
            state.dirty();
        }
        if title_changed {
            let w = &state.wins[&id];
            let (hex, title) = (w.hex_id(), w.title.clone());
            if state.focused_window() == Some(id) {
                state.emit(&["window_title_changed", &hex, &title]);
            }
        }
    }
}

/// Windows asking for activation become urgent rather than taking focus.
impl Dispatch<HrwmActivationV1, ()> for Wm {
    fn event(
        state: &mut Self,
        _: &HrwmActivationV1,
        event: hrwm_activation_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let hrwm_activation_v1::Event::ActivationRequested { window } = event;
        if let Some(&id) = state.by_obj.get(&window.id())
            && state.focused_window() != Some(id)
        {
            state.set_urgent(id, true);
        }
    }
}

impl Dispatch<RiverOutputV1, ()> for Wm {
    fn event(
        state: &mut Self,
        proxy: &RiverOutputV1,
        event: river_output_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use river_output_v1::Event;
        let Some(m) = state.monitors.iter_mut().find(|m| m.oid == proxy.id()) else {
            return;
        };
        match event {
            Event::Removed => {
                m.removed = true;
                if let Some(wl) = m.wl.take()
                    && wl.version() >= 3
                {
                    wl.release();
                }
            }
            Event::WlOutput { name } => {
                let version = state.output_globals.get(&name).copied().unwrap_or(1);
                if let Some(reg) = &state.registry {
                    m.wl = Some(reg.bind(name, version.min(4), &state.qh, m.oid.clone()));
                }
            }
            Event::Position { x, y } => {
                m.rect.x = x;
                m.rect.y = y;
            }
            Event::Dimensions { width, height } => {
                m.rect.w = width;
                m.rect.h = height;
            }
            _ => {}
        }
    }
}

impl Dispatch<RiverSeatV1, ()> for Wm {
    fn event(
        state: &mut Self,
        proxy: &RiverSeatV1,
        event: river_seat_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use river_seat_v1::Event;
        let win_of = |w: &RiverWindowV1| state.by_obj.get(&w.id()).copied();
        let Some(si) = state.seats.iter().position(|s| s.oid == proxy.id()) else {
            return;
        };
        match event {
            Event::Removed => state.seats[si].removed = true,
            Event::PointerEnter { window } => {
                let id = win_of(&window);
                let s = &mut state.seats[si];
                s.hovered = id;
                s.entered = id;
            }
            Event::PointerLeave => state.seats[si].hovered = None,
            Event::WindowInteraction { window } => {
                let id = win_of(&window);
                state.seats[si].interacted = id;
            }
            Event::ShellSurfaceInteraction { shell_surface } => {
                state.seats[si].shell_interacted = Some(shell_surface.id());
            }
            Event::OpDelta { dx, dy } => {
                if let Some(op) = &mut state.seats[si].op
                    && op.touch.is_none()
                {
                    op.dx = dx;
                    op.dy = dy;
                }
            }
            Event::OpRelease => {
                if let Some(op) = &mut state.seats[si].op
                    && op.touch.is_none()
                {
                    op.released = true;
                }
            }
            Event::OpDeltaTouch {
                touch_point,
                dx,
                dy,
            } => {
                if let Some(op) = &mut state.seats[si].op
                    && op.touch == Some(touch_point)
                {
                    op.dx = dx;
                    op.dy = dy;
                }
            }
            Event::OpReleaseTouch { touch_point } => {
                if let Some(op) = &mut state.seats[si].op
                    && op.touch == Some(touch_point)
                {
                    op.released = true;
                }
            }
            Event::OpCancelTouch { touch_point } => {
                if let Some(op) = &mut state.seats[si].op
                    && op.touch == Some(touch_point)
                {
                    op.canceled = true;
                }
            }
            Event::PointerPosition { x, y } => {
                let s = &mut state.seats[si];
                s.moved |= s.pointer != (x, y);
                s.pointer = (x, y);
            }
            _ => {}
        }
    }
}

impl Dispatch<RiverXkbBindingV1, (u32, u32)> for Wm {
    fn event(
        state: &mut Self,
        _: &RiverXkbBindingV1,
        event: river_xkb_binding_v1::Event,
        key: &(u32, u32),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let river_xkb_binding_v1::Event::Pressed = event
            && let Some(k) = state.keybinds.iter().find(|k| (k.mods, k.sym) == *key)
        {
            state.actions.push(Action::Cmd(k.cmd.clone()));
        }
    }
}

impl Dispatch<RiverPointerBindingV1, (ObjectId, u32, u32)> for Wm {
    fn event(
        state: &mut Self,
        _: &RiverPointerBindingV1,
        event: river_pointer_binding_v1::Event,
        data: &(ObjectId, u32, u32),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let river_pointer_binding_v1::Event::Pressed = event {
            let (seat, mods, button) = data.clone();
            if let Some(b) = state
                .mousebinds
                .iter()
                .find(|b| (b.mods, b.button) == (mods, button))
            {
                state.actions.push(Action::Mouse {
                    seat,
                    cmd: b.cmd.clone(),
                });
            }
        }
    }
}

impl Dispatch<RiverLayerShellOutputV1, ObjectId> for Wm {
    fn event(
        state: &mut Self,
        _: &RiverLayerShellOutputV1,
        event: river_layer_shell_output_v1::Event,
        output: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let river_layer_shell_output_v1::Event::NonExclusiveArea {
            x,
            y,
            width,
            height,
        } = event;
        if let Some(m) = state.monitors.iter_mut().find(|m| m.oid == *output) {
            m.usable = Some(Rect::new(x, y, width, height));
        }
    }
}

impl Dispatch<RiverLayerShellSeatV1, ObjectId> for Wm {
    fn event(
        state: &mut Self,
        _: &RiverLayerShellSeatV1,
        event: river_layer_shell_seat_v1::Event,
        seat: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // once a layer surface lets go of the keyboard, give focus back
        if let river_layer_shell_seat_v1::Event::FocusNone = event
            && let Some(s) = state.seats.iter_mut().find(|s| s.oid == *seat)
        {
            s.focus_sent = None;
        }
    }
}

delegate_noop!(Wm: RiverXkbBindingsV1);
delegate_noop!(Wm: RiverNodeV1);
delegate_noop!(Wm: RiverLayerShellV1);
delegate_noop!(Wm: RiverTouchGesturesV1);
delegate_noop!(Wm: RiverTouchGesturesSeatV1);
delegate_noop!(Wm: RiverShellSurfaceV1);
delegate_noop!(Wm: WlCompositor);
delegate_noop!(Wm: ignore WlSurface);
delegate_noop!(Wm: ignore WlBuffer);
delegate_noop!(Wm: WpViewporter);
delegate_noop!(Wm: WpViewport);
delegate_noop!(Wm: WpSinglePixelBufferManagerV1);

impl Dispatch<WlOutput, ObjectId> for Wm {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        output: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some(m) = state.monitors.iter_mut().find(|m| m.oid == *output)
        {
            m.name = name;
        }
    }
}
