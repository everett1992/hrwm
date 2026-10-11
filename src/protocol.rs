//! Client bindings for river's protocols, generated at compile time from the
//! XML files in `protocol/` (copied from /usr/share/river-protocols/stable).

#![allow(
    non_upper_case_globals,
    non_camel_case_types,
    unused_imports,
    dead_code
)]
#![allow(clippy::all)]

pub use wayland_client;
pub use wayland_client::protocol::*;

// Each XML file gets its own interfaces module, since the generated helper
// items would otherwise clash.
pub mod __interfaces {
    pub mod rwm {
        pub use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocol/river-window-management-v1.xml");
    }
    pub mod rxkb {
        use super::rwm::*;
        wayland_scanner::generate_interfaces!("protocol/river-xkb-bindings-v1.xml");
    }
    pub mod rls {
        use super::rwm::*;
        wayland_scanner::generate_interfaces!("protocol/river-layer-shell-v1.xml");
    }
    pub mod rim {
        use super::rwm::*;
        wayland_scanner::generate_interfaces!("protocol/river-input-management-v1.xml");
    }
    pub mod rxc {
        use super::rim::*;
        use super::rwm::*;
        wayland_scanner::generate_interfaces!("protocol/river-xkb-config-v1.xml");
    }
}
use self::__interfaces::rim::*;
use self::__interfaces::rls::*;
use self::__interfaces::rwm::*;
use self::__interfaces::rxc::*;
use self::__interfaces::rxkb::*;

wayland_scanner::generate_client_code!("protocol/river-window-management-v1.xml");
wayland_scanner::generate_client_code!("protocol/river-xkb-bindings-v1.xml");
wayland_scanner::generate_client_code!("protocol/river-layer-shell-v1.xml");
wayland_scanner::generate_client_code!("protocol/river-input-management-v1.xml");
wayland_scanner::generate_client_code!("protocol/river-xkb-config-v1.xml");
