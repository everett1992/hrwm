//! herbstluftwm-compatible settings (`set NAME VALUE`, `get NAME`).

use crate::tree::{Geometry, Layout};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    pub fn parse(s: &str) -> Option<Color> {
        let named = match s.to_ascii_lowercase().as_str() {
            "black" => Some(Color::rgb(0, 0, 0)),
            "white" => Some(Color::rgb(255, 255, 255)),
            "red" => Some(Color::rgb(255, 0, 0)),
            "green" => Some(Color::rgb(0, 255, 0)),
            "blue" => Some(Color::rgb(0, 0, 255)),
            "yellow" => Some(Color::rgb(255, 255, 0)),
            "orange" => Some(Color::rgb(255, 165, 0)),
            "gray" | "grey" => Some(Color::rgb(190, 190, 190)),
            "transparent" => Some(Color { r: 0, g: 0, b: 0, a: 0 }),
            _ => None,
        };
        if named.is_some() {
            return named;
        }
        let hex = s.strip_prefix('#')?;
        let digits: Vec<u8> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<_>>()?;
        let pair = |i: usize| digits[i] * 16 + digits[i + 1];
        match digits.len() {
            3 => Some(Color::rgb(digits[0] * 17, digits[1] * 17, digits[2] * 17)),
            6 => Some(Color::rgb(pair(0), pair(2), pair(4))),
            8 => Some(Color { r: pair(0), g: pair(2), b: pair(4), a: pair(6) }),
            _ => None,
        }
    }

    pub fn with_opacity(self, percent: i32) -> Color {
        let a = (self.a as i32 * percent.clamp(0, 100) / 100) as u8;
        Color { a, ..self }
    }

    /// Pre-multiplied 32-bit channels as river and single-pixel buffers want them.
    pub fn premultiplied_u32(self) -> [u32; 4] {
        let a = self.a as u64;
        let ch = |c: u8| ((c as u64 * a / 255) * 0x0101_0101) as u32;
        [ch(self.r), ch(self.g), ch(self.b), (a * 0x0101_0101) as u32]
    }

    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowFrames {
    None,
    Nonempty,
    IfEmpty,
    IfMultipleEmpty,
    Focused,
    FocusedIfMultiple,
    IfMultiple,
    All,
}

const SHOW_FRAMES: [(ShowFrames, &str); 8] = [
    (ShowFrames::None, "none"),
    (ShowFrames::Nonempty, "nonempty"),
    (ShowFrames::IfEmpty, "if_empty"),
    (ShowFrames::IfMultipleEmpty, "if_multiple_empty"),
    (ShowFrames::Focused, "focused"),
    (ShowFrames::FocusedIfMultiple, "focused_if_multiple"),
    (ShowFrames::IfMultiple, "if_multiple"),
    (ShowFrames::All, "all"),
];

impl ShowFrames {
    /// Port of FrameDecoration::updateVisibility.
    pub fn shows(self, has_clients: bool, focused: bool, is_root: bool) -> bool {
        match self {
            ShowFrames::None => false,
            ShowFrames::Nonempty => has_clients,
            ShowFrames::IfEmpty => !has_clients,
            ShowFrames::IfMultipleEmpty => !has_clients && !is_root,
            ShowFrames::Focused => has_clients || focused,
            ShowFrames::FocusedIfMultiple => has_clients || (focused && !is_root),
            ShowFrames::IfMultiple => has_clients || !is_root,
            ShowFrames::All => true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub window_gap: i32,
    pub frame_gap: i32,
    pub frame_padding: i32,
    pub frame_border_width: i32,
    pub window_border_width: i32,
    pub window_border_active_color: Color,
    pub window_border_normal_color: Color,
    pub window_border_urgent_color: Color,
    pub frame_border_active_color: Color,
    pub frame_border_normal_color: Color,
    pub frame_bg_active_color: Color,
    pub frame_bg_normal_color: Color,
    pub frame_bg_transparent: bool,
    pub frame_active_opacity: i32,
    pub frame_normal_opacity: i32,
    pub show_frame_decorations: ShowFrames,
    pub default_frame_layout: Layout,
    pub focus_follows_mouse: bool,
    pub raise_on_click: bool,
    pub raise_on_focus: bool,
    pub focus_crosses_monitor_boundaries: bool,
    pub swap_monitors_to_get_tag: bool,
    pub smart_frame_surroundings: bool,
    pub smart_window_surroundings: bool,
    pub smart_gaps: bool,
    pub default_direction_external_only: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            window_gap: 0,
            frame_gap: 5,
            frame_padding: 0,
            frame_border_width: 2,
            window_border_width: 2,
            window_border_active_color: Color::rgb(255, 0, 0),
            window_border_normal_color: Color::rgb(0, 0, 255),
            window_border_urgent_color: Color::rgb(255, 165, 0),
            frame_border_active_color: Color::rgb(255, 0, 0),
            frame_border_normal_color: Color::rgb(0, 0, 255),
            frame_bg_active_color: Color::rgb(0, 0, 0),
            frame_bg_normal_color: Color::rgb(0, 0, 0),
            frame_bg_transparent: false,
            frame_active_opacity: 100,
            frame_normal_opacity: 100,
            show_frame_decorations: ShowFrames::FocusedIfMultiple,
            default_frame_layout: Layout::Vertical,
            focus_follows_mouse: false,
            raise_on_click: true,
            raise_on_focus: false,
            focus_crosses_monitor_boundaries: true,
            swap_monitors_to_get_tag: true,
            smart_frame_surroundings: false,
            smart_window_surroundings: false,
            smart_gaps: false,
            default_direction_external_only: false,
        }
    }
}

/// Settings herbstluftwm has that make no sense here (or aren't implemented
/// yet). Setting them succeeds silently so existing autostarts keep working.
const IGNORED: &[&str] = &[
    "window_border_inner_width",
    "window_border_inner_color",
    "frame_border_inner_width",
    "frame_border_inner_color",
    "frame_transparent_width",
    "raise_on_focus_temporarily",
    "focus_stealing_prevention",
    "tree_style",
    "always_show_frame",
    "gapless_grid",
    "tabbed_max",
    "mouse_recenter_gap",
    "pseudotile_center_threshold",
    "update_dragged_clients",
    "snap_distance",
    "snap_gap",
    "hide_covered_windows",
    "auto_detect_monitors",
    "auto_detect_panels",
    "monitors_locked",
    "wmname",
    "verbose",
    "ellipsis",
    "focus_follows_shift",
];

fn parse_bool(cur: bool, v: &str) -> Result<bool, String> {
    match v {
        "1" | "true" | "on" => Ok(true),
        "0" | "false" | "off" => Ok(false),
        "toggle" => Ok(!cur),
        _ => Err(format!("invalid boolean \"{v}\"")),
    }
}

fn parse_int(cur: i32, v: &str) -> Result<i32, String> {
    let n = |s: &str| s.parse::<i32>().map_err(|_| format!("invalid integer \"{v}\""));
    // herbstluftwm accepts relative values like +=1 / -=1
    if let Some(d) = v.strip_prefix("+=") {
        Ok(cur + n(d)?)
    } else if let Some(d) = v.strip_prefix("-=") {
        Ok(cur - n(d)?)
    } else {
        n(v)
    }
}

fn parse_color(v: &str) -> Result<Color, String> {
    Color::parse(v).ok_or_else(|| format!("invalid color \"{v}\""))
}

impl Settings {
    pub fn geometry(&self) -> Geometry {
        Geometry {
            frame_gap: self.frame_gap,
            frame_border_width: self.frame_border_width,
            frame_padding: self.frame_padding,
            window_gap: self.window_gap,
            smart_frame_surroundings: self.smart_frame_surroundings,
            smart_window_surroundings: self.smart_window_surroundings,
            smart_gaps: self.smart_gaps,
        }
    }

    pub fn set(&mut self, name: &str, v: &str) -> Result<(), String> {
        macro_rules! int { ($f:ident) => { self.$f = parse_int(self.$f, v)? } }
        macro_rules! boolean { ($f:ident) => { self.$f = parse_bool(self.$f, v)? } }
        macro_rules! color { ($f:ident) => { self.$f = parse_color(v)? } }
        match name {
            "window_gap" => int!(window_gap),
            "frame_gap" => int!(frame_gap),
            "frame_padding" => int!(frame_padding),
            "frame_border_width" => int!(frame_border_width),
            "window_border_width" => int!(window_border_width),
            "frame_active_opacity" => int!(frame_active_opacity),
            "frame_normal_opacity" => int!(frame_normal_opacity),
            "window_border_active_color" => color!(window_border_active_color),
            "window_border_normal_color" => color!(window_border_normal_color),
            "window_border_urgent_color" => color!(window_border_urgent_color),
            "frame_border_active_color" => color!(frame_border_active_color),
            "frame_border_normal_color" => color!(frame_border_normal_color),
            "frame_bg_active_color" => color!(frame_bg_active_color),
            "frame_bg_normal_color" => color!(frame_bg_normal_color),
            "frame_bg_transparent" => boolean!(frame_bg_transparent),
            "focus_follows_mouse" => boolean!(focus_follows_mouse),
            "raise_on_click" => boolean!(raise_on_click),
            "raise_on_focus" => boolean!(raise_on_focus),
            "focus_crosses_monitor_boundaries" => boolean!(focus_crosses_monitor_boundaries),
            "swap_monitors_to_get_tag" => boolean!(swap_monitors_to_get_tag),
            "smart_window_surroundings" => boolean!(smart_window_surroundings),
            "smart_gaps" => boolean!(smart_gaps),
            "default_direction_external_only" => boolean!(default_direction_external_only),
            "smart_frame_surroundings" => {
                // newer herbstluftwm uses off/hide_gaps/hide_all
                self.smart_frame_surroundings = match v {
                    "hide_gaps" | "hide_all" => true,
                    _ => parse_bool(self.smart_frame_surroundings, v)?,
                }
            }
            "default_frame_layout" => {
                self.default_frame_layout =
                    Layout::parse(v).ok_or_else(|| format!("invalid layout \"{v}\""))?
            }
            "show_frame_decorations" => {
                self.show_frame_decorations = SHOW_FRAMES
                    .iter()
                    .find(|(_, n)| *n == v)
                    .map(|(s, _)| *s)
                    .ok_or_else(|| format!("invalid value \"{v}\""))?
            }
            _ if IGNORED.contains(&name) => {}
            _ => return Err(format!("unknown setting \"{name}\"")),
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<String, String> {
        let b = |x: bool| if x { "true" } else { "false" }.to_string();
        Ok(match name {
            "window_gap" => self.window_gap.to_string(),
            "frame_gap" => self.frame_gap.to_string(),
            "frame_padding" => self.frame_padding.to_string(),
            "frame_border_width" => self.frame_border_width.to_string(),
            "window_border_width" => self.window_border_width.to_string(),
            "frame_active_opacity" => self.frame_active_opacity.to_string(),
            "frame_normal_opacity" => self.frame_normal_opacity.to_string(),
            "window_border_active_color" => self.window_border_active_color.to_hex(),
            "window_border_normal_color" => self.window_border_normal_color.to_hex(),
            "window_border_urgent_color" => self.window_border_urgent_color.to_hex(),
            "frame_border_active_color" => self.frame_border_active_color.to_hex(),
            "frame_border_normal_color" => self.frame_border_normal_color.to_hex(),
            "frame_bg_active_color" => self.frame_bg_active_color.to_hex(),
            "frame_bg_normal_color" => self.frame_bg_normal_color.to_hex(),
            "frame_bg_transparent" => b(self.frame_bg_transparent),
            "focus_follows_mouse" => b(self.focus_follows_mouse),
            "raise_on_click" => b(self.raise_on_click),
            "raise_on_focus" => b(self.raise_on_focus),
            "focus_crosses_monitor_boundaries" => b(self.focus_crosses_monitor_boundaries),
            "swap_monitors_to_get_tag" => b(self.swap_monitors_to_get_tag),
            "smart_frame_surroundings" => b(self.smart_frame_surroundings),
            "smart_window_surroundings" => b(self.smart_window_surroundings),
            "smart_gaps" => b(self.smart_gaps),
            "default_direction_external_only" => b(self.default_direction_external_only),
            "default_frame_layout" => self.default_frame_layout.name().to_string(),
            "show_frame_decorations" => {
                let s = self.show_frame_decorations;
                SHOW_FRAMES.iter().find(|(x, _)| *x == s).unwrap().1.to_string()
            }
            _ => return Err(format!("unknown setting \"{name}\"")),
        })
    }

    /// For `toggle NAME`.
    pub fn toggle(&mut self, name: &str) -> Result<(), String> {
        self.set(name, "toggle")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(Color::parse("#c8bdb8"), Some(Color::rgb(0xc8, 0xbd, 0xb8)));
        assert_eq!(Color::parse("#fff"), Some(Color::rgb(255, 255, 255)));
        assert_eq!(Color::parse("red"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(Color::parse("#12"), None);
        assert_eq!(Color::rgb(255, 0, 0).premultiplied_u32(), [u32::MAX, 0, 0, u32::MAX]);
    }

    #[test]
    fn set_and_get() {
        let mut s = Settings::default();
        s.set("window_gap", "6").unwrap();
        s.set("window_gap", "+=2").unwrap();
        assert_eq!(s.get("window_gap").unwrap(), "8");
        s.set("default_frame_layout", "3").unwrap();
        assert_eq!(s.default_frame_layout, Layout::Grid);
        s.set("focus_follows_mouse", "1").unwrap();
        s.set("focus_follows_mouse", "toggle").unwrap();
        assert!(!s.focus_follows_mouse);
        assert!(s.set("tree_style", "x").is_ok());
        assert!(s.set("nonsense", "1").is_err());
    }
}
