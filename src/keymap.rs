//! Keyboard layout configuration (`keymap LAYOUT [VARIANT [OPTIONS]]`).
//!
//! river 0.4 leaves keymaps to clients of river-xkb-config-v1: we compile a
//! keymap with libxkbcommon, hand it to river and apply it to every keyboard,
//! including ones plugged in later.

use std::ffi::{CStr, CString, c_char, c_void};
use std::io::Write;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};

use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};

use crate::protocol::{
    river_input_device_v1::RiverInputDeviceV1,
    river_input_manager_v1::{self, RiverInputManagerV1},
    river_xkb_config_v1::{self, KeymapFormat, RiverXkbConfigV1},
    river_xkb_keyboard_v1::{self, RiverXkbKeyboardV1},
    river_xkb_keymap_v1::{self, RiverXkbKeymapV1},
};
use crate::wm::Wm;

#[repr(C)]
struct RuleNames {
    rules: *const c_char,
    model: *const c_char,
    layout: *const c_char,
    variant: *const c_char,
    options: *const c_char,
}

#[link(name = "xkbcommon")]
unsafe extern "C" {
    fn xkb_context_new(flags: i32) -> *mut c_void;
    fn xkb_context_unref(ctx: *mut c_void);
    fn xkb_keymap_new_from_names(
        ctx: *mut c_void,
        names: *const RuleNames,
        flags: i32,
    ) -> *mut c_void;
    fn xkb_keymap_get_as_string(keymap: *mut c_void, format: i32) -> *mut c_char;
    fn xkb_keymap_unref(keymap: *mut c_void);
}

/// Compile a keymap to its text form.
fn compile(layout: &str, variant: &str, options: &str) -> Result<Vec<u8>, String> {
    let c = |s: &str| CString::new(s).map_err(|_| "invalid keymap argument".to_string());
    let (layout, variant, options) = (c(layout)?, c(variant)?, c(options)?);
    let names = RuleNames {
        rules: std::ptr::null(),
        model: std::ptr::null(),
        layout: layout.as_ptr(),
        variant: variant.as_ptr(),
        options: options.as_ptr(),
    };
    unsafe {
        let ctx = xkb_context_new(0);
        if ctx.is_null() {
            return Err("cannot create xkb context".into());
        }
        let km = xkb_keymap_new_from_names(ctx, &names, 0);
        if km.is_null() {
            xkb_context_unref(ctx);
            return Err("invalid layout, variant or options".into());
        }
        let text = xkb_keymap_get_as_string(km, 1);
        let bytes = CStr::from_ptr(text).to_bytes_with_nul().to_vec();
        libc::free(text as *mut c_void);
        xkb_keymap_unref(km);
        xkb_context_unref(ctx);
        Ok(bytes)
    }
}

fn sealed_memfd(data: &[u8]) -> Result<OwnedFd, String> {
    let fd = unsafe {
        libc::memfd_create(
            c"hrwm-keymap".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if fd < 0 {
        return Err("memfd_create failed".into());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let mut f = std::fs::File::from(fd.try_clone().map_err(|e| e.to_string())?);
    f.write_all(data).map_err(|e| e.to_string())?;
    unsafe {
        use std::os::fd::AsRawFd;
        libc::fcntl(
            fd.as_raw_fd(),
            libc::F_ADD_SEALS,
            libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE | libc::F_SEAL_SEAL,
        );
    }
    Ok(fd)
}

#[derive(Default)]
pub struct Keymaps {
    pub config: Option<RiverXkbConfigV1>,
    pub input: Option<RiverInputManagerV1>,
    keyboards: Vec<RiverXkbKeyboardV1>,
    /// the keymap applied to keyboards, once river accepted it
    active: Option<RiverXkbKeymapV1>,
    pending: Option<RiverXkbKeymapV1>,
    pub description: String,
}

impl Keymaps {
    /// e.g. "applied to 1 keyboard(s)"
    pub fn status(&self) -> String {
        let state = match (&self.pending, &self.active) {
            (Some(_), _) => "pending",
            (None, Some(_)) => "applied",
            (None, None) => "not set",
        };
        format!("{state}, {} keyboard(s)", self.keyboards.len())
    }
}

impl Wm {
    /// `keymap LAYOUT [VARIANT [OPTIONS]]`
    pub fn set_keymap(&mut self, layout: &str, variant: &str, options: &str) -> Result<(), String> {
        let config = self
            .keymaps
            .config
            .clone()
            .ok_or("river_xkb_config_v1 not available")?;
        let text = compile(layout, variant, options)?;
        let fd = sealed_memfd(&text)?;
        let km = config.create_keymap(fd.as_fd(), KeymapFormat::TextV1, &self.qh, ());
        if let Some(old) = self.keymaps.pending.replace(km) {
            old.destroy();
        }
        self.keymaps.description = format!("{layout}\t{variant}\t{options}");
        Ok(())
    }
}

impl Dispatch<RiverXkbConfigV1, ()> for Wm {
    fn event(
        state: &mut Self,
        _: &RiverXkbConfigV1,
        event: river_xkb_config_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let river_xkb_config_v1::Event::XkbKeyboard { id } = event {
            if let Some(km) = &state.keymaps.active {
                id.set_keymap(km);
            }
            state.keymaps.keyboards.push(id);
        }
    }

    wayland_client::event_created_child!(Wm, RiverXkbConfigV1, [
        river_xkb_config_v1::EVT_XKB_KEYBOARD_OPCODE => (RiverXkbKeyboardV1, ()),
    ]);
}

impl Dispatch<RiverXkbKeymapV1, ()> for Wm {
    fn event(
        state: &mut Self,
        proxy: &RiverXkbKeymapV1,
        event: river_xkb_keymap_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let k = &mut state.keymaps;
        if k.pending.as_ref() != Some(proxy) {
            return;
        }
        let km = k.pending.take().unwrap();
        match event {
            river_xkb_keymap_v1::Event::Success => {
                for kb in &k.keyboards {
                    kb.set_keymap(&km);
                }
                if let Some(old) = k.active.replace(km) {
                    old.destroy();
                }
            }
            river_xkb_keymap_v1::Event::Failure { error_msg } => {
                eprintln!("hrwm: river rejected the keymap: {error_msg}");
                km.destroy();
            }
        }
    }
}

impl Dispatch<RiverXkbKeyboardV1, ()> for Wm {
    fn event(
        state: &mut Self,
        proxy: &RiverXkbKeyboardV1,
        event: river_xkb_keyboard_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let river_xkb_keyboard_v1::Event::Removed = event {
            state.keymaps.keyboards.retain(|k| k != proxy);
            proxy.destroy();
        }
    }
}

impl Dispatch<RiverInputManagerV1, ()> for Wm {
    fn event(
        _: &mut Self,
        _: &RiverInputManagerV1,
        _: river_input_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    wayland_client::event_created_child!(Wm, RiverInputManagerV1, [
        river_input_manager_v1::EVT_INPUT_DEVICE_OPCODE => (RiverInputDeviceV1, ()),
    ]);
}

delegate_noop!(Wm: ignore RiverInputDeviceV1);

#[cfg(test)]
mod tests {
    #[test]
    fn compiles_keymaps() {
        assert!(super::compile("us", "", "caps:swapescape").is_ok());
        assert!(super::compile("us", "workman", "").is_ok());
    }
}
