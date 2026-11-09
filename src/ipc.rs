//! The command socket, shared by the `hrwm` client and the window manager.
//!
//! A request is the command's arguments, each terminated by a NUL byte,
//! after which the client shuts down its writing half. The response is the
//! exit status on the first line followed by the command's output.
//!
//! `hrwm --idle` instead keeps the connection open and receives every hook
//! as one tab-separated line.

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use regex::Regex;

pub fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    let display = display
        .rsplit('/')
        .next()
        .unwrap_or("wayland-0")
        .to_string();
    dir.join(format!("hrwm-{display}.sock"))
}

pub fn encode(args: &[String]) -> Vec<u8> {
    let mut v = Vec::new();
    for a in args {
        v.extend_from_slice(a.as_bytes());
        v.push(0);
    }
    v
}

pub fn decode(buf: &[u8]) -> Vec<String> {
    // every argument is NUL-terminated, so empty arguments survive
    match buf.strip_suffix(&[0]) {
        Some(body) => body
            .split(|&b| b == 0)
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect(),
        None => Vec::new(),
    }
}

fn connect() -> Result<UnixStream, i32> {
    let path = socket_path();
    UnixStream::connect(&path).map_err(|e| {
        eprintln!(
            "hrwm: cannot connect to {}: {e} (is hrwm running?)",
            path.display()
        );
        1
    })
}

/// Send one command; returns its exit status and output.
pub fn request(args: &[String]) -> Result<(i32, String), i32> {
    let mut s = connect()?;
    let mut resp = String::new();
    if s.write_all(&encode(args)).is_err()
        || s.shutdown(std::net::Shutdown::Write).is_err()
        || s.read_to_string(&mut resp).is_err()
    {
        eprintln!("hrwm: lost connection");
        return Err(1);
    }
    let (status, output) = resp.split_once('\n').unwrap_or((&resp, ""));
    Ok((status.trim().parse().unwrap_or(1), output.to_string()))
}

/// Send one command and print its result. Returns the exit status.
pub fn send(args: &[String]) -> i32 {
    let (code, output) = match request(args) {
        Ok(r) => r,
        Err(code) => return code,
    };
    if code == 0 {
        print!("{output}");
    } else {
        eprint!("{output}");
    }
    code
}

/// Print hooks as they happen. With a regex, only hooks whose name matches.
pub fn idle(filter: Option<&str>, mut each: impl FnMut(&[&str]) -> bool) -> i32 {
    let filter = match filter.map(|f| Regex::new(&format!("^(?:{f})$"))) {
        Some(Ok(re)) => Some(re),
        Some(Err(e)) => {
            eprintln!("hrwm: invalid regex: {e}");
            return 1;
        }
        None => None,
    };
    let mut s = match connect() {
        Ok(s) => s,
        Err(code) => return code,
    };
    if s.write_all(&encode(&["--idle".to_string()])).is_err()
        || s.shutdown(std::net::Shutdown::Write).is_err()
    {
        return 1;
    }
    for line in BufReader::new(s).lines() {
        let Ok(line) = line else { return 0 };
        let fields: Vec<&str> = line.split('\t').collect();
        if filter.as_ref().is_some_and(|re| !re.is_match(fields[0])) {
            continue;
        }
        if !each(&fields) {
            return 0;
        }
    }
    0
}

/// A connection that is still sending its request.
pub struct Pending {
    pub stream: UnixStream,
    buf: Vec<u8>,
}

impl Pending {
    pub fn new(stream: UnixStream) -> Pending {
        let _ = stream.set_nonblocking(true);
        Pending {
            stream,
            buf: Vec::new(),
        }
    }

    /// Read what's available. Returns Some(args) once the request is complete,
    /// Err on a broken connection.
    pub fn poll(&mut self) -> Result<Option<Vec<String>>, ()> {
        let mut chunk = [0u8; 4096];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Ok(Some(decode(&self.buf))),
                Ok(n) => {
                    self.buf.extend_from_slice(&chunk[..n]);
                    if self.buf.len() > 1 << 20 {
                        return Err(());
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(None),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Err(()),
            }
        }
    }
}

/// A command's response, written as the client reads it so a client that
/// stops reading never stalls the window manager (river waits on hrwm's
/// manage sequence, where commands run). Dropping it closes the connection.
pub struct Reply {
    pub stream: UnixStream,
    buf: Vec<u8>,
    sent: usize,
}

impl Reply {
    pub fn new(stream: UnixStream, code: i32, output: &str) -> Reply {
        let _ = stream.set_nonblocking(true);
        Reply {
            stream,
            buf: format!("{code}\n{output}").into_bytes(),
            sent: 0,
        }
    }

    /// Write what the socket takes. Ok(true) once it's all sent, Err if the
    /// client went away.
    pub fn poll(&mut self) -> Result<bool, ()> {
        while self.sent < self.buf.len() {
            match self.stream.write(&self.buf[self.sent..]) {
                Ok(0) => return Err(()),
                Ok(n) => self.sent += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(false),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Err(()),
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::os::unix::net::UnixStream;

    #[test]
    fn reply_does_not_wait_for_the_reader() {
        let (ours, mut theirs) = UnixStream::pair().unwrap();
        let output = "x".repeat(4 << 20);
        let mut reply = super::Reply::new(ours, 0, &output);
        // more than the socket buffer holds: returns instead of blocking
        assert_eq!(reply.poll(), Ok(false));
        let reader = std::thread::spawn(move || {
            let mut got = String::new();
            theirs.read_to_string(&mut got).unwrap();
            got
        });
        while reply.poll() == Ok(false) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        drop(reply);
        assert_eq!(reader.join().unwrap(), format!("0\n{output}"));
    }

    #[test]
    fn empty_arguments_survive() {
        let args: Vec<String> = ["keymap", "us", "", "caps:swapescape"]
            .map(String::from)
            .to_vec();
        assert_eq!(super::decode(&super::encode(&args)), args);
        assert!(super::decode(b"").is_empty());
    }
}
