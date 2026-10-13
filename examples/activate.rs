//! Test helper: open a window that requests activation of itself through
//! xdg-activation-v1 after a delay, then stays open until killed.
//!
//!   activate SECONDS

use std::time::Duration;

use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_registry, wl_surface::WlSurface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1;
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use wayland_protocols::xdg::activation::v1::client::{
    xdg_activation_token_v1::{self, XdgActivationTokenV1},
    xdg_activation_v1::XdgActivationV1,
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

#[derive(Default)]
struct State {
    compositor: Option<WlCompositor>,
    wm_base: Option<XdgWmBase>,
    pixels: Option<WpSinglePixelBufferManagerV1>,
    viewporter: Option<WpViewporter>,
    activation: Option<XdgActivationV1>,
    surface: Option<WlSurface>,
    viewport: Option<WpViewport>,
    size: (i32, i32),
    mapped: bool,
    token: Option<String>,
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
                "wl_compositor" => s.compositor = Some(r.bind(name, 4, qh, ())),
                "xdg_wm_base" => s.wm_base = Some(r.bind(name, 1, qh, ())),
                "wp_single_pixel_buffer_manager_v1" => s.pixels = Some(r.bind(name, 1, qh, ())),
                "wp_viewporter" => s.viewporter = Some(r.bind(name, 1, qh, ())),
                "xdg_activation_v1" => s.activation = Some(r.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        wm_base: &XdgWmBase,
        e: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = e {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for State {
    fn event(
        s: &mut Self,
        xdg_surface: &XdgSurface,
        e: xdg_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let xdg_surface::Event::Configure { serial } = e else {
            return;
        };
        xdg_surface.ack_configure(serial);
        let surface = s.surface.as_ref().unwrap();
        let (w, h) = s.size;
        let (w, h) = (if w > 0 { w } else { 200 }, if h > 0 { h } else { 200 });
        let buffer = s
            .pixels
            .as_ref()
            .unwrap()
            .create_u32_rgba_buffer(0, 0, 0, u32::MAX, qh, ());
        surface.attach(Some(&buffer), 0, 0);
        s.viewport.as_ref().unwrap().set_destination(w, h);
        surface.commit();
        s.mapped = true;
    }
}

impl Dispatch<XdgToplevel, ()> for State {
    fn event(
        s: &mut Self,
        _: &XdgToplevel,
        e: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            xdg_toplevel::Event::Configure { width, height, .. } => s.size = (width, height),
            xdg_toplevel::Event::Close => std::process::exit(0),
            _ => {}
        }
    }
}

impl Dispatch<XdgActivationTokenV1, ()> for State {
    fn event(
        s: &mut Self,
        _: &XdgActivationTokenV1,
        e: xdg_activation_token_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = e {
            s.token = Some(token);
        }
    }
}

delegate_noop!(State: WlCompositor);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: WpSinglePixelBufferManagerV1);
delegate_noop!(State: WpViewporter);
delegate_noop!(State: WpViewport);
delegate_noop!(State: XdgActivationV1);

fn main() {
    let secs: f64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or_else(|| {
            eprintln!("usage: activate SECONDS");
            std::process::exit(2);
        });
    let conn = Connection::connect_to_env().expect("no Wayland display");
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut s = State::default();
    queue.roundtrip(&mut s).unwrap();
    let missing = |what: &str| -> ! {
        eprintln!("activate: compositor lacks {what}");
        std::process::exit(1);
    };
    let compositor = s
        .compositor
        .clone()
        .unwrap_or_else(|| missing("wl_compositor"));
    let wm_base = s.wm_base.clone().unwrap_or_else(|| missing("xdg_wm_base"));
    let viewporter = s
        .viewporter
        .clone()
        .unwrap_or_else(|| missing("wp_viewporter"));
    let activation = s
        .activation
        .clone()
        .unwrap_or_else(|| missing("xdg_activation_v1"));
    if s.pixels.is_none() {
        missing("wp_single_pixel_buffer_manager_v1");
    }

    let surface = compositor.create_surface(&qh, ());
    s.viewport = Some(viewporter.get_viewport(&surface, &qh, ()));
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_app_id("hrwm-activate".into());
    toplevel.set_title("activate".into());
    surface.commit();
    s.surface = Some(surface.clone());
    while !s.mapped {
        queue.blocking_dispatch(&mut s).unwrap();
    }
    queue.roundtrip(&mut s).unwrap();

    std::thread::sleep(Duration::from_secs_f64(secs));
    // a token without a serial: the compositor shouldn't treat this as user
    // input, so a window manager may refuse to hand over focus
    let token = activation.get_activation_token(&qh, ());
    token.set_surface(&surface);
    token.commit();
    while s.token.is_none() {
        queue.blocking_dispatch(&mut s).unwrap();
    }
    activation.activate(s.token.take().unwrap(), &surface);
    loop {
        queue.blocking_dispatch(&mut s).unwrap();
    }
}
