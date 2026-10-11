//! Test helper: drive a nested/headless river session with a virtual
//! keyboard and pointer.
//!
//!   vinput key MODS KEYCODE              press and release an evdev key
//!   vinput drag MODS BUTTON X1 Y1 X2 Y2  press BUTTON at X1,Y1 and release at X2,Y2
//!   vinput move X Y                      move the pointer
//!
//! MODS is the river modifier mask (Shift=1 Ctrl=4 Alt=8 Super=64).
//! Coordinates are absolute within a 1280x720 output.

use std::ffi::{CStr, c_char, c_void};
use std::io::Write;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::time::Duration;

use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

#[link(name = "xkbcommon")]
unsafe extern "C" {
    fn xkb_context_new(flags: i32) -> *mut c_void;
    fn xkb_keymap_new_from_names(ctx: *mut c_void, names: *const c_void, flags: i32)
    -> *mut c_void;
    fn xkb_keymap_get_as_string(keymap: *mut c_void, format: i32) -> *mut c_char;
}

#[derive(Default)]
struct State {
    seat: Option<WlSeat>,
    kbd: Option<ZwpVirtualKeyboardManagerV1>,
    ptr: Option<ZwlrVirtualPointerManagerV1>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        s: &mut Self,
        r: &wl_registry::WlRegistry,
        e: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = e
        {
            match interface.as_str() {
                "wl_seat" => s.seat = Some(r.bind(name, 1, qh, ())),
                "zwp_virtual_keyboard_manager_v1" => s.kbd = Some(r.bind(name, 1, qh, ())),
                "zwlr_virtual_pointer_manager_v1" => s.ptr = Some(r.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}
delegate_noop!(State: ignore WlSeat);
delegate_noop!(State: ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ZwpVirtualKeyboardV1);
delegate_noop!(State: ZwlrVirtualPointerManagerV1);
delegate_noop!(State: ZwlrVirtualPointerV1);

fn keymap_fd() -> (OwnedFd, u32) {
    let s = unsafe {
        let ctx = xkb_context_new(0);
        let km = xkb_keymap_new_from_names(ctx, std::ptr::null(), 0);
        CStr::from_ptr(xkb_keymap_get_as_string(km, 1))
            .to_bytes_with_nul()
            .to_vec()
    };
    let fd = unsafe { OwnedFd::from_raw_fd(libc::memfd_create(c"keymap".as_ptr(), 0)) };
    let mut f = std::fs::File::from(fd.try_clone().unwrap());
    f.write_all(&s).unwrap();
    (fd, s.len() as u32)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n = |i: usize| args[i].parse::<u32>().unwrap();
    let conn = Connection::connect_to_env().unwrap();
    let mut q = conn.new_event_queue();
    let qh = q.handle();
    conn.display().get_registry(&qh, ());
    let mut st = State::default();
    q.roundtrip(&mut st).unwrap();
    let seat = st.seat.clone().unwrap();
    let kbd = st
        .kbd
        .as_ref()
        .unwrap()
        .create_virtual_keyboard(&seat, &qh, ());
    let (fd, size) = keymap_fd();
    kbd.keymap(1, fd.as_fd(), size);
    let ptr = st
        .ptr
        .as_ref()
        .unwrap()
        .create_virtual_pointer(Some(&seat), &qh, ());
    q.roundtrip(&mut st).unwrap();
    let sync = |q: &mut wayland_client::EventQueue<State>, st: &mut State| {
        q.roundtrip(st).unwrap();
        std::thread::sleep(Duration::from_millis(60));
    };
    let t = || 0u32;
    let mods_of = |m: u32| {
        // river modifier bits use the same values as xkb's default modifier indices masks
        m
    };
    match args[0].as_str() {
        "key" => {
            let (mods, key) = (n(1), n(2));
            kbd.modifiers(mods_of(mods), 0, 0, 0);
            sync(&mut q, &mut st);
            kbd.key(t(), key, 1);
            sync(&mut q, &mut st);
            kbd.key(t(), key, 0);
            kbd.modifiers(0, 0, 0, 0);
            sync(&mut q, &mut st);
        }
        "move" => {
            ptr.motion_absolute(t(), n(1), n(2), 1280, 720);
            ptr.frame();
            sync(&mut q, &mut st);
        }
        "drag" => {
            let (mods, button) = (n(1), n(2));
            ptr.motion_absolute(t(), n(3), n(4), 1280, 720);
            ptr.frame();
            sync(&mut q, &mut st);
            kbd.modifiers(mods_of(mods), 0, 0, 0);
            sync(&mut q, &mut st);
            ptr.button(
                t(),
                button,
                wayland_client::protocol::wl_pointer::ButtonState::Pressed,
            );
            ptr.frame();
            sync(&mut q, &mut st);
            for i in 1..=10 {
                let x = n(3) as i64 + (n(5) as i64 - n(3) as i64) * i / 10;
                let y = n(4) as i64 + (n(6) as i64 - n(4) as i64) * i / 10;
                ptr.motion_absolute(t(), x as u32, y as u32, 1280, 720);
                ptr.frame();
                sync(&mut q, &mut st);
            }
            ptr.button(
                t(),
                button,
                wayland_client::protocol::wl_pointer::ButtonState::Released,
            );
            ptr.frame();
            kbd.modifiers(0, 0, 0, 0);
            sync(&mut q, &mut st);
        }
        _ => eprintln!("unknown"),
    }
}
