//! The one Hyprland query M1 needs: which monitor has focus (the `active-output` mapping target).
#![allow(unsafe_code)]

use std::{
    io::{self, Read, Write},
    mem,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Hyprland answers in well under a millisecond; this only bounds a wedged compositor, because
/// the daemon asks from its event-loop thread.
const TIMEOUT: Duration = Duration::from_millis(200);

/// The focused monitor's connector name, or `None` when not on Hyprland or the query fails
/// (the caller falls back to the first output).
pub fn focused_monitor() -> Option<String> {
    let reply = query("j/monitors")?;
    parse_focused(&reply)
}

fn socket_path() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let instance = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    Some(
        PathBuf::from(runtime)
            .join("hypr")
            .join(instance)
            .join(".socket.sock"),
    )
}

fn query(request: &str) -> Option<String> {
    let path = socket_path()?;
    // One deadline for the whole exchange: connect, write and every read.
    let deadline = Instant::now() + TIMEOUT;
    let result = (|| {
        let mut stream = connect(&path, deadline)?;
        stream.set_write_timeout(Some(remaining(deadline)?))?;
        stream.write_all(request.as_bytes())?;
        let mut reply = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            stream.set_read_timeout(Some(remaining(deadline)?))?;
            match stream.read(&mut chunk)? {
                0 => break,
                n => reply.extend_from_slice(&chunk[..n]),
            }
        }
        String::from_utf8(reply).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    })();
    match result {
        Ok(reply) => Some(reply),
        Err(err) => {
            tracing::debug!(path = %path.display(), %err, "hyprland query failed");
            None
        }
    }
}

/// Time left before `deadline`, or `TimedOut` (a zero timeout would mean "block forever").
fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "hyprland query timed out"))
}

/// Connects without blocking past `deadline`. `UnixStream::connect` has no timeout, and a
/// wedged compositor with a full accept backlog would block the daemon's event loop.
fn connect(path: &Path, deadline: Instant) -> io::Result<UnixStream> {
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: sockaddr_un is plain old data; all-zero is a valid value.
    let mut addr: libc::sockaddr_un = unsafe { mem::zeroed() };
    if bytes.len() >= addr.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path too long",
        ));
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (dst, &src) in addr.sun_path.iter_mut().zip(bytes) {
        *dst = src as libc::c_char;
    }
    let len = (mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t;
    // SAFETY: the arguments select a non-blocking Unix stream socket.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is a new socket that nothing else owns.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    loop {
        // SAFETY: addr is initialised and len covers its family and NUL-terminated path.
        let rc = unsafe { libc::connect(stream.as_raw_fd(), (&raw const addr).cast(), len) };
        if rc == 0 {
            break;
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EISCONN) => break,
            // Backlog full (EAGAIN) or still connecting: wait a little and try again.
            Some(libc::EAGAIN | libc::EINPROGRESS | libc::EALREADY | libc::EINTR) => {
                let wait = remaining(deadline)?.min(Duration::from_millis(5));
                let mut pfd = libc::pollfd {
                    fd: stream.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                };
                // SAFETY: one valid pollfd that lives for the duration of the call.
                unsafe { libc::poll(&mut pfd, 1, wait.as_millis().max(1) as i32) };
            }
            _ => return Err(err),
        }
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

/// The `name` of the monitor with `"focused": true` in `hyprctl -j monitors` output.
pub fn parse_focused(json: &str) -> Option<String> {
    let monitors: serde_json::Value = serde_json::from_str(json).ok()?;
    monitors
        .as_array()?
        .iter()
        .find(|m| m.get("focused").and_then(serde_json::Value::as_bool) == Some(true))?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../tests/fixtures/monitors.json");

    #[test]
    fn finds_the_focused_monitor_in_a_captured_reply() {
        assert_eq!(parse_focused(SAMPLE).as_deref(), Some("HDMI-A-1"));
    }

    #[test]
    fn a_missing_socket_fails_fast_instead_of_blocking() {
        let start = Instant::now();
        let dir = std::env::temp_dir().join("handos-no-such-hyprland.sock");
        assert!(connect(&dir, Instant::now() + TIMEOUT).is_err());
        assert!(start.elapsed() < TIMEOUT);
    }

    #[test]
    fn a_live_socket_answers_within_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut req = [0u8; 10];
            conn.read_exact(&mut req).unwrap();
            conn.write_all(b"[]").unwrap();
        });
        let mut stream = connect(&path, Instant::now() + TIMEOUT).unwrap();
        stream.write_all(b"j/monitors").unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        server.join().unwrap();
        assert_eq!(reply, "[]");
    }

    #[test]
    fn anything_else_is_none() {
        assert_eq!(parse_focused("[]"), None);
        assert_eq!(parse_focused("not json"), None);
        assert_eq!(parse_focused(r#"[{"name":"DP-1","focused":false}]"#), None);
        assert_eq!(parse_focused(r#"{"name":"DP-1","focused":true}"#), None);
    }
}
