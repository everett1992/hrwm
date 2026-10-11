//! herbstclient-compatible commands.

use std::ffi::{CString, c_char, c_int};
use std::os::unix::process::CommandExt;
use std::process::Command;

use crate::tree::{Dir, Frame, Layout};
use crate::wm::{KeyBind, MouseBind, Place, Wm};

#[link(name = "xkbcommon")]
unsafe extern "C" {
    fn xkb_keysym_from_name(name: *const c_char, flags: c_int) -> u32;
}

const XKB_KEYSYM_CASE_INSENSITIVE: c_int = 1;

fn keysym(name: &str) -> Option<u32> {
    // bindings match the unshifted keysym, so Mod4-Shift-Z means Mod4-Shift-z
    let name = if name.len() == 1 {
        name.to_ascii_lowercase()
    } else {
        name.to_string()
    };
    let c = CString::new(name).ok()?;
    let sym = unsafe { xkb_keysym_from_name(c.as_ptr(), 0) };
    if sym != 0 {
        return Some(sym);
    }
    let sym = unsafe { xkb_keysym_from_name(c.as_ptr(), XKB_KEYSYM_CASE_INSENSITIVE) };
    (sym != 0).then_some(sym)
}

fn modifier(name: &str) -> Option<u32> {
    match name.to_ascii_lowercase().as_str() {
        "shift" => Some(1),
        "control" | "ctrl" => Some(4),
        "mod1" | "alt" => Some(8),
        "mod2" => Some(0), // numlock: ignored
        "mod3" => Some(32),
        "mod4" | "super" => Some(64),
        "mod5" => Some(128),
        _ => None,
    }
}

/// "Mod4-Shift-z" or "Mod4+Shift+z" -> (modifiers, last token)
fn parse_combo(combo: &str) -> Result<(u32, String), String> {
    let parts: Vec<&str> = combo.split(['-', '+']).collect();
    // allow binding the minus key itself, e.g. Mod4-minus or Mod4--
    let (mods, key) = match parts.split_last() {
        Some((&"", rest)) if !rest.is_empty() => (&rest[..rest.len() - 1], "minus"),
        Some((key, rest)) => (rest, *key),
        None => return Err(format!("invalid key combination \"{combo}\"")),
    };
    let mut m = 0;
    for p in mods {
        m |= modifier(p).ok_or_else(|| format!("unknown modifier \"{p}\""))?;
    }
    Ok((m, key.to_string()))
}

fn button(name: &str) -> Option<u32> {
    // linux/input-event-codes.h
    match name {
        "Button1" => Some(0x110), // BTN_LEFT
        "Button2" => Some(0x112), // BTN_MIDDLE
        "Button3" => Some(0x111), // BTN_RIGHT
        "Button8" => Some(0x113), // BTN_SIDE
        "Button9" => Some(0x114), // BTN_EXTRA
        _ => None,
    }
}

fn parse_bool_arg(cur: bool, v: Option<&String>) -> Result<bool, String> {
    match v.map(String::as_str) {
        None | Some("toggle") => Ok(!cur),
        Some("on" | "true" | "1") => Ok(true),
        Some("off" | "false" | "0") => Ok(false),
        Some(x) => Err(format!("invalid argument \"{x}\"")),
    }
}

fn spawn(args: &[String]) -> Result<(), String> {
    let (prog, rest) = args.split_first().ok_or("spawn: missing command")?;
    let mut cmd = Command::new(prog);
    cmd.args(rest);
    unsafe {
        cmd.pre_exec(|| {
            // undo SIG_IGN for SIGCHLD and detach from our session
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("spawn {prog}: {e}"))
}

pub fn autostart() {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".config")))
        .unwrap_or_default();
    let path = dir.join("hrwm/autostart");
    if path.exists() {
        if let Err(e) = spawn(&[path.to_string_lossy().into_owned()]) {
            eprintln!("hrwm: {e}");
        }
    } else {
        eprintln!("hrwm: no autostart at {}", path.display());
    }
}

fn dump(f: &Frame, wm: &Wm, out: &mut String) {
    match f {
        Frame::Leaf(l) => {
            out.push_str(&format!("(clients {}:{}", l.layout.name(), l.sel));
            for w in &l.wins {
                out.push_str(&format!(" {}", wm.wins[w].hex_id()));
            }
            out.push(')');
        }
        Frame::Split(s) => {
            let align = match s.align {
                crate::tree::Align::Vertical => "vertical",
                crate::tree::Align::Horizontal => "horizontal",
            };
            out.push_str(&format!("(split {align}:{:.6}:{} ", s.frac, s.sel));
            dump(&s.a, wm, out);
            out.push(' ');
            dump(&s.b, wm, out);
            out.push(')');
        }
    }
}

impl Wm {
    /// Run one command. Output goes to `out`; errors are returned.
    pub fn run(&mut self, args: &[String], out: &mut String) -> Result<(), String> {
        let Some((cmd, a)) = args.split_first() else {
            return Ok(());
        };
        let a: Vec<String> = a.to_vec();
        let arg = |i: usize| {
            a.get(i)
                .ok_or_else(|| format!("{cmd}: not enough arguments"))
        };
        let ti = self.cur_tag();

        match cmd.as_str() {
            // --- meta commands ---
            "chain" | "and" | "or" => {
                let sep = arg(0)?.clone();
                let mut last = Ok(());
                for part in a[1..].split(|x| *x == sep) {
                    last = self.run(part, out);
                    match (cmd.as_str(), &last) {
                        ("and", Err(_)) | ("or", Ok(())) => break,
                        _ => {}
                    }
                }
                return last;
            }
            "silent" => {
                let mut sink = String::new();
                return self.run(&a, &mut sink);
            }
            "try" => {
                let _ = self.run(&a, out);
            }
            "!" | "negate" => {
                return match self.run(&a, out) {
                    Ok(()) => Err(String::new()),
                    Err(_) => Ok(()),
                };
            }
            "true" | "lock" | "unlock" | "detect_monitors" | "pad" => {}
            "false" => return Err(String::new()),
            "echo" => {
                out.push_str(&a.join(" "));
                out.push('\n');
            }
            "version" => out.push_str(concat!("hrwm ", env!("CARGO_PKG_VERSION"), "\n")),
            "quit" => {
                if let Some(r) = &self.river {
                    r.exit_session();
                }
            }
            "reload" => {
                self.emit(&["reload"]);
                crate::commands::autostart();
            }
            "spawn" => spawn(&a)?,
            "emit_hook" => {
                let f: Vec<&str> = a.iter().map(String::as_str).collect();
                self.emit(&f);
            }

            // --- bindings ---
            "keybind" => {
                let combo = arg(0)?;
                let (mods, key) = parse_combo(combo)?;
                let sym = keysym(&key).ok_or_else(|| format!("unknown key \"{key}\""))?;
                if a.len() < 2 {
                    return Err("keybind: missing command".into());
                }
                self.keybinds.retain(|k| (k.mods, k.sym) != (mods, sym));
                self.keybinds.push(KeyBind {
                    mods,
                    sym,
                    combo: combo.clone(),
                    cmd: a[1..].to_vec(),
                });
                self.binds_dirty = true;
            }
            "keyunbind" => {
                let combo = arg(0)?;
                if matches!(combo.as_str(), "--all" | "-F") {
                    self.keybinds.clear();
                } else {
                    let (mods, key) = parse_combo(combo)?;
                    let sym = keysym(&key).ok_or_else(|| format!("unknown key \"{key}\""))?;
                    self.keybinds.retain(|k| (k.mods, k.sym) != (mods, sym));
                }
                self.binds_dirty = true;
            }
            "list_keybinds" => {
                for k in &self.keybinds {
                    out.push_str(&format!("{}\t{}\n", k.combo, k.cmd.join("\t")));
                }
            }
            "mousebind" => {
                let combo = arg(0)?;
                let (mods, b) = parse_combo(combo)?;
                let button = button(&b).ok_or_else(|| format!("unknown button \"{b}\""))?;
                if a.len() < 2 {
                    return Err("mousebind: missing action".into());
                }
                self.mousebinds
                    .retain(|m| (m.mods, m.button) != (mods, button));
                self.mousebinds.push(MouseBind {
                    mods,
                    button,
                    combo: combo.clone(),
                    cmd: a[1..].to_vec(),
                });
                self.binds_dirty = true;
            }
            "list_mousebinds" => {
                for m in &self.mousebinds {
                    out.push_str(&format!("{}\t{}\n", m.combo, m.cmd.join("\t")));
                }
            }
            "mouseunbind" => {
                self.mousebinds.clear();
                self.binds_dirty = true;
            }

            "input" => self.input_command(&a)?,
            "list_inputs" => out.push_str(&self.inputs.list()),
            "keymap" => {
                if a.is_empty() {
                    out.push_str(&self.keymaps.description);
                    out.push_str(&format!("\t({})\n", self.keymaps.status()));
                } else {
                    let get = |i: usize| a.get(i).map(String::as_str).unwrap_or("");
                    self.set_keymap(get(0), get(1), get(2))?;
                }
            }

            // --- settings ---
            "set" => {
                self.settings.set(arg(0)?, arg(1)?)?;
                if a[0] == "default_frame_layout" {
                    // tags nobody has used yet (like the first one, created
                    // before the autostart ran) get the new default too
                    let layout = self.settings.default_frame_layout;
                    for t in &mut self.tags {
                        if let Frame::Leaf(l) = &mut t.tree
                            && l.wins.is_empty()
                        {
                            l.layout = layout;
                        }
                    }
                }
            }
            "get" => {
                out.push_str(&self.settings.get(arg(0)?)?);
                out.push('\n');
            }
            "toggle" => self.settings.toggle(arg(0)?)?,

            // --- rules ---
            "rule" => out.push_str(&self.rules.add(&a)?),
            "unrule" => self.rules.remove(arg(0)?)?,
            "list_rules" => out.push_str(&self.rules.list()),

            // --- tags ---
            "add" => {
                self.add_tag(arg(0)?);
            }
            "rename" => {
                let (old, new) = (arg(0)?, arg(1)?);
                if self.tag_index(new).is_some() {
                    return Err(format!("tag \"{new}\" already exists"));
                }
                let i = self
                    .tag_index(old)
                    .ok_or_else(|| format!("no tag \"{old}\""))?;
                self.tags[i].name = new.clone();
                self.emit(&["tag_renamed", old, new]);
            }
            "use" => {
                let t = self
                    .tag_index(arg(0)?)
                    .ok_or_else(|| format!("no tag \"{}\"", a[0]))?;
                self.use_tag(t);
            }
            "use_index" => {
                let t = self.resolve_index(arg(0)?, a.iter().any(|x| x == "--skip-visible"))?;
                self.use_tag(t);
            }
            "use_previous" => {
                let prev = self.monitors.get(self.focused_mon).and_then(|m| m.prev_tag);
                if let Some(p) = prev.filter(|&p| p < self.tags.len()) {
                    self.use_tag(p);
                }
            }
            "move" | "move_index" => {
                let t = if cmd == "move" {
                    self.tag_index(arg(0)?)
                        .ok_or_else(|| format!("no tag \"{}\"", a[0]))?
                } else {
                    self.resolve_index(arg(0)?, a.iter().any(|x| x == "--skip-visible"))?
                };
                if let Some(w) = self.focused_window()
                    && t != ti
                {
                    self.detach(w);
                    self.attach(w, t, false);
                    self.emit(&["tag_flags"]);
                }
            }
            "merge_tag" => {
                let victim = self
                    .tag_index(arg(0)?)
                    .ok_or_else(|| format!("no tag \"{}\"", a[0]))?;
                let target = match a.get(1) {
                    Some(n) => self.tag_index(n).ok_or_else(|| format!("no tag \"{n}\""))?,
                    None => ti,
                };
                if victim == target {
                    return Err("cannot merge a tag into itself".into());
                }
                if self.monitor_of_tag(victim).is_some() {
                    return Err("cannot merge a tag that is shown".into());
                }
                for w in self.tags[victim].windows() {
                    self.detach(w);
                    self.attach(w, target, false);
                }
                self.remove_tag(victim);
            }
            "floating" => {
                let t = &mut self.tags[ti];
                if a.first().map(String::as_str) == Some("status") {
                    out.push_str(if t.floating { "on\n" } else { "off\n" });
                } else {
                    t.floating = parse_bool_arg(t.floating, a.first())?;
                }
            }
            "tag_status" => {
                let mon = match a.first() {
                    Some(m) => self
                        .monitor_by_ref(m)
                        .ok_or_else(|| format!("no monitor \"{m}\""))?,
                    None => self.focused_mon,
                };
                out.push('\t');
                for (i, t) in self.tags.iter().enumerate() {
                    let mut c = if t.windows().is_empty() { '.' } else { ':' };
                    match self.monitor_of_tag(i) {
                        Some(m) if m == mon => c = if m == self.focused_mon { '#' } else { '+' },
                        Some(m) => c = if m == self.focused_mon { '%' } else { '-' },
                        None => {}
                    }
                    if t.windows()
                        .iter()
                        .any(|w| self.wins.get(w).is_some_and(|w| w.urgent))
                    {
                        c = '!';
                    }
                    out.push(c);
                    out.push_str(&t.name);
                    out.push('\t');
                }
            }

            // --- frames ---
            "split" => {
                let frac = match a.get(1) {
                    Some(f) => Some(
                        f.parse::<f64>()
                            .map_err(|_| format!("invalid fraction \"{f}\""))?,
                    ),
                    None => None,
                };
                let hint = self.reference_rect();
                self.tags[ti].tree.split(arg(0)?, frac, hint)?;
            }
            "remove" => self.tags[ti].tree.remove_frame(),
            "rotate" => self.tags[ti].tree.rotate(),
            "cycle_layout" => {
                let delta = match a.first() {
                    Some(d) => d.parse().map_err(|_| format!("invalid delta \"{d}\""))?,
                    None => 1,
                };
                let list = a
                    .iter()
                    .skip(1)
                    .map(|n| Layout::parse(n).ok_or_else(|| format!("invalid layout \"{n}\"")))
                    .collect::<Result<Vec<_>, _>>()?;
                self.tags[ti].tree.cycle_layout(delta, &list);
            }
            "set_layout" => {
                let l =
                    Layout::parse(arg(0)?).ok_or_else(|| format!("invalid layout \"{}\"", a[0]))?;
                self.tags[ti].tree.focused_leaf_mut().layout = l;
            }
            "cycle_frame" => {
                let d = a.first().and_then(|d| d.parse().ok()).unwrap_or(1);
                self.tags[ti].tree.cycle_frame(d);
                self.tags[ti].float_focus = None;
            }
            "resize" => {
                let dir =
                    Dir::parse(arg(0)?).ok_or_else(|| format!("invalid direction \"{}\"", a[0]))?;
                let delta = match a.get(1) {
                    Some(d) => d
                        .trim_start_matches('+')
                        .parse::<f64>()
                        .map_err(|_| format!("invalid delta \"{d}\""))?,
                    None => 0.02,
                };
                if !self.tags[ti].tree.resize(dir, delta) {
                    return Err("no neighbour found".into());
                }
            }

            // --- focus and movement ---
            "focus" | "shift" => {
                let mut external = self.settings.default_direction_external_only;
                let mut dir = None;
                for x in &a {
                    match x.as_str() {
                        "-i" => external = false,
                        "-e" => external = true,
                        d => dir = Dir::parse(d),
                    }
                }
                let dir = dir.ok_or_else(|| format!("{cmd}: missing direction"))?;
                self.focus_or_shift(cmd == "shift", dir, external)?;
            }
            "swap" => {
                let dir =
                    Dir::parse(arg(0)?).ok_or_else(|| format!("invalid direction \"{}\"", a[0]))?;
                self.swap(dir)?;
            }
            "cycle" => {
                let d = a.first().and_then(|d| d.parse().ok()).unwrap_or(1);
                let t = &mut self.tags[ti];
                if t.float_focus.is_some() && !t.floats.is_empty() {
                    let i = t
                        .floats
                        .iter()
                        .position(|&x| Some(x) == t.float_focus)
                        .unwrap_or(0) as i32;
                    let n = t.floats.len() as i32;
                    t.float_focus = Some(t.floats[(i + d).rem_euclid(n) as usize]);
                } else {
                    t.tree.cycle(d);
                }
            }
            "cycle_all" => {
                let d = a.iter().find_map(|x| x.parse::<i32>().ok()).unwrap_or(1);
                let all = self.tags[ti].windows();
                if !all.is_empty() {
                    let cur = self
                        .focused_window()
                        .and_then(|f| all.iter().position(|&x| x == f));
                    let i = cur
                        .map(|c| (c as i32 + d).rem_euclid(all.len() as i32) as usize)
                        .unwrap_or(0);
                    self.focus_win(all[i]);
                    self.raise(all[i]);
                }
            }
            "jumpto" | "bring" => {
                let id = match arg(0)?.as_str() {
                    "urgent" => self.next_urgent().ok_or("no urgent window")?,
                    s => self
                        .parse_winid(s)
                        .ok_or_else(|| format!("no window \"{s}\""))?,
                };
                if cmd == "bring" && self.wins[&id].tag != ti {
                    self.detach(id);
                    self.attach(id, ti, true);
                } else if self.monitor_of_tag(self.wins[&id].tag).is_none() {
                    self.use_tag(self.wins[&id].tag);
                }
                self.focus_win(id);
                self.raise(id);
            }
            "raise" => {
                let id = self.parse_winid(arg(0)?).ok_or("no such window")?;
                self.raise(id);
            }

            // --- monitors ---
            "cycle_monitor" => {
                let d = a.first().and_then(|d| d.parse().ok()).unwrap_or(1);
                let n = self.monitors.len() as i32;
                if n > 0 {
                    self.focus_monitor((self.focused_mon as i32 + d).rem_euclid(n) as usize);
                }
            }
            "focus_monitor" => {
                let m = arg(0)?
                    .parse::<usize>()
                    .map_err(|_| format!("invalid monitor \"{}\"", a[0]))?;
                if m >= self.monitors.len() {
                    return Err(format!("no monitor {m}"));
                }
                self.focus_monitor(m);
            }
            "list_monitors" => {
                for (i, m) in self.monitors.iter().enumerate() {
                    let r = m.rect;
                    let focus = if i == self.focused_mon {
                        " [FOCUS]"
                    } else {
                        ""
                    };
                    out.push_str(&format!(
                        "{i}: {}x{}{:+}{:+} with tag \"{}\"{focus} {}\n",
                        r.w, r.h, r.x, r.y, self.tags[m.tag].name, m.name
                    ));
                }
            }

            // --- clients ---
            "close" => {
                let id = match a.first() {
                    Some(s) => Some(
                        self.parse_winid(s)
                            .ok_or_else(|| format!("no window \"{s}\""))?,
                    ),
                    None => self.focused_window(),
                };
                if let Some(id) = id {
                    self.wins[&id].obj.close();
                }
            }
            "close_or_remove" | "close_and_remove" => {
                let t = &self.tags[ti];
                let focused = t.focused();
                if let Some(id) = focused {
                    self.wins[&id].obj.close();
                    if cmd == "close_and_remove"
                        && t.float_focus.is_none()
                        && t.tree.focused_leaf().wins.len() == 1
                    {
                        self.tags[ti].tree.remove_frame();
                    }
                } else {
                    self.tags[ti].tree.remove_frame();
                }
            }
            "fullscreen" | "pseudotile" => {
                let id = self.focused_window().ok_or("no focused window")?;
                self.set_client_flag(id, cmd, a.first())?;
            }
            "attr" | "get_attr" | "set_attr" => {
                let path = arg(0)?.as_str();
                match a.get(1) {
                    Some(v) if cmd != "get_attr" => self.set_attr(path, v)?,
                    _ => {
                        out.push_str(&self.get_attr(path)?);
                        out.push('\n');
                    }
                }
            }
            "list_clients" => {
                let mut ids: Vec<_> = self.wins.keys().copied().collect();
                ids.sort();
                for id in ids {
                    let w = &self.wins[&id];
                    let place = match w.place {
                        Place::Hidden => "hidden",
                        Place::Tiled { .. } => "tiled",
                        Place::Floating => "floating",
                        Place::Fullscreen => "fullscreen",
                    };
                    let geom = w
                        .geometry(self.settings.window_border_width)
                        .map(|r| format!("{}x{}{:+}{:+}", r.w, r.h, r.x, r.y))
                        .unwrap_or_else(|| "-".into());
                    out.push_str(&format!(
                        "{}\t{}\t{}\t{}\t{}\t{}\n",
                        w.hex_id(),
                        self.tags[w.tag].name,
                        place,
                        geom,
                        w.app_id,
                        w.title
                    ));
                }
            }
            "dump" | "layout" => {
                let t = match a.first() {
                    Some(n) => self.tag_index(n).ok_or_else(|| format!("no tag \"{n}\""))?,
                    None => ti,
                };
                dump(&self.tags[t].tree, self, out);
                out.push('\n');
            }

            _ => return Err(format!("unknown command \"{cmd}\"")),
        }
        Ok(())
    }

    /// A monitor by index or by output name (as waybar's WAYBAR_OUTPUT_NAME).
    fn monitor_by_ref(&self, r: &str) -> Option<usize> {
        match r.parse::<usize>() {
            Ok(i) => (i < self.monitors.len()).then_some(i),
            Err(_) => self.monitors.iter().position(|m| m.name == r),
        }
    }

    /// Move the focused window the way `focus DIR` moves focus, trading
    /// places with the window found there. Focus stays on the moved window.
    fn swap(&mut self, dir: Dir) -> Result<(), String> {
        let ti = self.cur_tag();
        if self.tags[ti].float_focus.is_some() || self.tags[ti].floating {
            return Err("swap only works on tiled windows".into());
        }
        if self.tags[ti].tree.swap_dir(dir) {
            return Ok(());
        }
        let mine = self.tags[ti]
            .tree
            .focused_window()
            .ok_or("no focused window")?;
        let m = self
            .monitor_in_dir(dir)
            .filter(|_| self.settings.focus_crosses_monitor_boundaries)
            .ok_or("no neighbour found")?;
        let other = self.monitors[m].tag;
        match self.tags[other].tree.focused_window() {
            Some(theirs) => {
                self.tags[ti].tree.replace_window(mine, theirs);
                self.tags[other].tree.replace_window(theirs, mine);
                self.wins.get_mut(&mine).unwrap().tag = other;
                self.wins.get_mut(&theirs).unwrap().tag = ti;
            }
            None => {
                self.detach(mine);
                self.attach(mine, other, true);
            }
        }
        self.focus_win(mine);
        self.emit(&["tag_flags"]);
        Ok(())
    }

    fn resolve_index(&self, s: &str, skip_visible: bool) -> Result<usize, String> {
        let n = self.tags.len() as i32;
        if s.starts_with('+') || s.starts_with('-') {
            let d: i32 = s.parse().map_err(|_| format!("invalid index \"{s}\""))?;
            let mut i = self.cur_tag() as i32;
            for _ in 0..n {
                i = (i + d).rem_euclid(n);
                if !skip_visible || self.monitor_of_tag(i as usize).is_none() {
                    return Ok(i as usize);
                }
            }
            Ok(self.cur_tag())
        } else {
            let i: usize = s.parse().map_err(|_| format!("invalid index \"{s}\""))?;
            if i >= self.tags.len() {
                return Err(format!("no tag with index {i}"));
            }
            Ok(i)
        }
    }

    fn focus_or_shift(&mut self, shift: bool, dir: Dir, external: bool) -> Result<(), String> {
        let ti = self.cur_tag();
        let t = &mut self.tags[ti];
        let floating_focus = t.float_focus.is_some() || t.floating;
        if floating_focus {
            // pick the nearest floating window in that direction
            if !shift {
                if let Some(id) = self.nearest_float(dir) {
                    self.focus_win(id);
                    return Ok(());
                }
            } else if let Some(id) = self.focused_window() {
                let step = 32;
                if let Some(r) = self.wins.get_mut(&id).and_then(|w| w.float_rect.as_mut()) {
                    match dir {
                        Dir::Left => r.x -= step,
                        Dir::Right => r.x += step,
                        Dir::Up => r.y -= step,
                        Dir::Down => r.y += step,
                    }
                    return Ok(());
                }
            }
        } else {
            let done = if shift {
                t.tree.shift_dir(dir, external)
            } else {
                t.tree.focus_dir(dir, external)
            };
            if done {
                return Ok(());
            }
        }
        if self.settings.focus_crosses_monitor_boundaries
            && let Some(m) = self.monitor_in_dir(dir)
        {
            if shift && let Some(w) = self.focused_window() {
                let target = self.monitors[m].tag;
                self.detach(w);
                self.attach(w, target, true);
                self.emit(&["tag_flags"]);
            }
            self.focus_monitor(m);
            return Ok(());
        }
        Err("no neighbour found".into())
    }

    fn nearest_float(&self, dir: Dir) -> Option<u32> {
        let t = &self.tags[self.cur_tag()];
        let cur = self.wins.get(&t.float_focus?)?.float_rect?.center();
        t.floats
            .iter()
            .filter(|&&id| Some(id) != t.float_focus)
            .filter_map(|&id| {
                let (x, y) = self.wins.get(&id)?.float_rect?.center();
                let ok = match dir {
                    Dir::Left => x < cur.0,
                    Dir::Right => x > cur.0,
                    Dir::Up => y < cur.1,
                    Dir::Down => y > cur.1,
                };
                ok.then_some((id, (x - cur.0).abs() + (y - cur.1).abs()))
            })
            .min_by_key(|&(_, d)| d)
            .map(|(id, _)| id)
    }

    fn set_client_flag(&mut self, id: u32, flag: &str, v: Option<&String>) -> Result<(), String> {
        match flag {
            "floating" => {
                let cur = self.wins[&id].floating;
                let on = parse_bool_arg(cur, v)?;
                self.set_floating(id, on);
            }
            "fullscreen" => {
                let w = self.wins.get_mut(&id).unwrap();
                w.fullscreen = parse_bool_arg(w.fullscreen, v)?;
                let (hex, on) = (w.hex_id(), w.fullscreen);
                self.emit(&["fullscreen", if on { "on" } else { "off" }, &hex]);
            }
            "pseudotile" => {
                let w = self.wins.get_mut(&id).unwrap();
                w.pseudotile = parse_bool_arg(w.pseudotile, v)?;
            }
            "urgent" => {
                let on = parse_bool_arg(self.wins[&id].urgent, v)?;
                self.set_urgent(id, on);
            }
            _ => return Err(format!("attribute \"{flag}\" is read-only or unknown")),
        }
        Ok(())
    }

    fn set_attr(&mut self, path: &str, v: &String) -> Result<(), String> {
        if let Some(name) = path.strip_prefix("settings.") {
            return self.settings.set(name, v);
        }
        if let Some(rest) = path.strip_prefix("clients.") {
            let (id, flag) = self.client_attr(rest)?;
            return self.set_client_flag(id, flag, Some(v));
        }
        if path == "tags.focus.floating" {
            let ti = self.cur_tag();
            self.tags[ti].floating = parse_bool_arg(self.tags[ti].floating, Some(v))?;
            return Ok(());
        }
        Err(format!("unsupported attribute \"{path}\""))
    }

    /// "focus.floating" or "0x3.urgent" -> (window, attribute)
    fn client_attr<'a>(&self, rest: &'a str) -> Result<(u32, &'a str), String> {
        let (who, attr) = rest
            .split_once('.')
            .ok_or_else(|| format!("invalid attribute \"clients.{rest}\""))?;
        let id = if who == "focus" {
            self.focused_window().ok_or("no focused window")?
        } else {
            self.parse_winid(who)
                .ok_or_else(|| format!("no window \"{who}\""))?
        };
        Ok((id, attr))
    }

    fn get_attr(&self, path: &str) -> Result<String, String> {
        if let Some(name) = path.strip_prefix("settings.") {
            return self.settings.get(name);
        }
        let b = |x: bool| if x { "true" } else { "false" }.to_string();
        if let Some(rest) = path.strip_prefix("clients.") {
            let (id, attr) = self.client_attr(rest)?;
            let w = &self.wins[&id];
            return Ok(match attr {
                "winid" => w.hex_id(),
                "title" => w.title.clone(),
                "class" | "instance" => w.app_id.clone(),
                "tag" => self.tags[w.tag].name.clone(),
                "floating" => b(w.floating),
                "fullscreen" => b(w.fullscreen),
                "pseudotile" => b(w.pseudotile),
                "urgent" => b(w.urgent),
                _ => return Err(format!("unknown attribute \"{attr}\"")),
            });
        }
        let ti = self.cur_tag();
        Ok(match path {
            "tags.focus.name" => self.tags[ti].name.clone(),
            "tags.focus.index" => ti.to_string(),
            "tags.focus.floating" => b(self.tags[ti].floating),
            "tags.focus.client_count" => self.tags[ti].windows().len().to_string(),
            "tags.focus.frame_count" => self.tags[ti].tree.leaf_count().to_string(),
            "tags.count" => self.tags.len().to_string(),
            "monitors.focus.index" => self.focused_mon.to_string(),
            "monitors.count" => self.monitors.len().to_string(),
            _ => return Err(format!("unsupported attribute \"{path}\"")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos() {
        assert_eq!(parse_combo("Mod4-Shift-z").unwrap(), (65, "z".to_string()));
        assert_eq!(
            parse_combo("Control-Alt-Delete").unwrap(),
            (12, "Delete".to_string())
        );
        assert_eq!(
            parse_combo("Mod4+period").unwrap(),
            (64, "period".to_string())
        );
        assert_eq!(parse_combo("Mod4--").unwrap(), (64, "minus".to_string()));
        assert!(parse_combo("Hyper-x").is_err());
    }

    #[test]
    fn keysyms() {
        assert_eq!(keysym("z"), Some(0x7a));
        assert_eq!(keysym("Z"), Some(0x7a));
        assert_eq!(keysym("Return"), Some(0xff0d));
        assert_eq!(keysym("BackSpace"), Some(0xff08));
        assert_eq!(keysym("XF86AudioMute"), Some(0x1008ff12));
        assert_eq!(keysym("slash"), Some(0x2f));
        assert_eq!(keysym("nonsense_key"), None);
    }
}
