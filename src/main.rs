//! hrwm: a herbstluftwm-style frame tiling window manager for river 0.4.
//!
//! `hrwm` with no arguments runs the window manager (normally from
//! ~/.config/river/init). With arguments it sends them as a command to the
//! running window manager, like herbstclient.

mod commands;
mod ipc;
mod keymap;
mod protocol;
mod rules;
mod settings;
mod tree;
mod wm;

use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::UnixListener;

use wayland_client::Connection;

use crate::ipc::Pending;
use crate::wm::Wm;

const USAGE: &str = "usage: hrwm                      run the window manager
       hrwm COMMAND [ARGS...]    send a command (like herbstclient)
       hrwm --idle [REGEX]       print hooks as they happen
       hrwm --waybar-tag NAME    JSON for a waybar custom module showing one tag
       hrwm --waybar-title       JSON for a waybar custom module showing the focused title
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => run_wm(),
        Some("-h" | "--help") => print!("{USAGE}"),
        Some("--idle") => std::process::exit(ipc::idle(args.get(1).map(String::as_str), |f| {
            println!("{}", f.join("\t"));
            true
        })),
        Some("--waybar-tag") => match args.get(1) {
            Some(name) => std::process::exit(waybar_tag(name)),
            None => {
                eprint!("{USAGE}");
                std::process::exit(1);
            }
        },
        Some("--waybar-title") => std::process::exit(waybar_title()),
        Some(_) => std::process::exit(ipc::send(&args)),
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// One line of JSON per change for a waybar custom module (return-type json).
///
/// Status is relative to the monitor of the bar running this (waybar sets
/// WAYBAR_OUTPUT_NAME). Classes: `this-monitor` or `other-monitor` for tags
/// being shown, plus `focused` when that monitor has focus; otherwise
/// `occupied`, `empty` or `urgent`.
fn waybar_tag(name: &str) -> i32 {
    use std::io::Write;
    let mut query = vec!["tag_status".to_string()];
    if let Ok(output) = std::env::var("WAYBAR_OUTPUT_NAME") {
        query.push(output);
    }
    let print = || -> bool {
        let Ok((0, status)) = ipc::request(&query) else { return false };
        let Some(field) = status.split('\t').find(|f| f.get(1..) == Some(name)) else {
            return true;
        };
        let classes = match field.chars().next() {
            Some('#') => r#"["this-monitor","focused"]"#,
            Some('+') => r#"["this-monitor"]"#,
            Some('%') => r#"["other-monitor","focused"]"#,
            Some('-') => r#"["other-monitor"]"#,
            Some(':') => r#"["occupied"]"#,
            Some('!') => r#"["urgent"]"#,
            _ => r#"["empty"]"#,
        };
        let n = json_escape(name);
        println!("{{\"text\":\"{n}\",\"class\":{classes},\"tooltip\":\"{n}\"}}");
        std::io::stdout().flush().is_ok()
    };
    if !print() {
        return 1;
    }
    ipc::idle(Some("tag_.*|focus_changed|reload"), |_| print())
}

fn waybar_title() -> i32 {
    use std::io::Write;
    let print = |title: &str| -> bool {
        println!("{{\"text\":\"{}\"}}", json_escape(title));
        std::io::stdout().flush().is_ok()
    };
    let initial = match ipc::request(&["attr".into(), "clients.focus.title".into()]) {
        Ok((0, t)) => t.trim_end().to_string(),
        Ok(_) => String::new(),
        Err(_) => return 1,
    };
    print(&initial);
    ipc::idle(Some("focus_changed|window_title_changed"), |f| print(f.get(2).copied().unwrap_or("")))
}

fn run_wm() {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("hrwm: cannot connect to the Wayland display: {e}");
            std::process::exit(1);
        }
    };
    // children shouldn't inherit WAYLAND_DEBUG
    unsafe { std::env::remove_var("WAYLAND_DEBUG") };
    // reap children automatically; spawn() resets this for the child
    unsafe { libc::signal(libc::SIGCHLD, libc::SIG_IGN) };

    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    let mut wm = Wm::new(qh);
    if let Err(e) = queue.roundtrip(&mut wm) {
        eprintln!("hrwm: {e}");
        std::process::exit(1);
    }
    if let Err(e) = wm.has_globals() {
        eprintln!("hrwm: {e}");
        std::process::exit(1);
    }

    let path = ipc::socket_path();
    if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        eprintln!("hrwm: already running ({} is in use)", path.display());
        std::process::exit(1);
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("hrwm: cannot listen on {}: {e}", path.display());
            std::process::exit(1);
        }
    };
    listener.set_nonblocking(true).unwrap();

    commands::autostart();

    let mut pending: Vec<Pending> = Vec::new();
    loop {
        if let Err(e) = queue.dispatch_pending(&mut wm) {
            eprintln!("hrwm: {e}");
            break;
        }
        if let Err(e) = queue.flush() {
            eprintln!("hrwm: {e}");
            break;
        }
        let Some(guard) = queue.prepare_read() else { continue };

        let mut fds = vec![
            libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        for p in &pending {
            fds.push(libc::pollfd { fd: p.stream.as_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 });
        }
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if n < 0 {
            drop(guard);
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            eprintln!("hrwm: poll failed");
            break;
        }
        if fds[0].revents != 0 {
            if let Err(e) = guard.read() {
                let would_block = matches!(&e, wayland_client::backend::WaylandError::Io(io)
                    if io.kind() == std::io::ErrorKind::WouldBlock);
                if !would_block {
                    eprintln!("hrwm: {e}");
                    break;
                }
            }
        } else {
            drop(guard);
        }
        if fds[1].revents != 0 {
            while let Ok((stream, _)) = listener.accept() {
                pending.push(Pending::new(stream));
            }
        }
        let mut i = 0;
        let mut got_command = false;
        while i < pending.len() {
            match pending[i].poll() {
                Ok(None) => i += 1,
                Ok(Some(args)) => {
                    let p = pending.swap_remove(i);
                    if args.first().map(String::as_str) == Some("--idle") {
                        let _ = p.stream.set_nonblocking(true);
                        wm.idle.push(p.stream);
                    } else {
                        wm.ipc_queue.push((p.stream, args));
                        got_command = true;
                    }
                }
                Err(()) => {
                    pending.swap_remove(i);
                }
            }
        }
        if got_command {
            wm.dirty();
        }
    }
    let _ = std::fs::remove_file(&path);
    std::process::exit(1);
}
