#![allow(unsafe_code)]

use std::{
    fs::{self, File, OpenOptions},
    io, mem,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::{
            ffi::OsStrExt,
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command},
};

use anyhow::{Context, Result, bail};

pub struct Listener {
    fd: OwnedFd,
    pub path: PathBuf,
    _lock: File,
}

impl Listener {
    pub fn bind() -> Result<Self> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").context("set XDG_RUNTIME_DIR")?;
        let dir = Path::new(&runtime).join("handos");
        match fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).context("create handos runtime directory"),
        }
        let meta = fs::symlink_metadata(&dir)?;
        // SAFETY: geteuid has no arguments and no memory preconditions.
        if !meta.is_dir()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.permissions().mode() & 0o777 != 0o700
        {
            bail!(
                "{} must be a private directory owned by this user (0700)",
                dir.display()
            );
        }

        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(dir.join("tracker.lock"))?;
        // SAFETY: lock owns a valid fd for the duration of the call.
        let locked = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if locked < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                bail!("daemon already running");
            }
            return Err(error).context("lock handos runtime directory");
        }
        let path = dir.join("tracker.sock");
        let bytes = path.as_os_str().as_bytes();
        // SAFETY: zero is a valid initial value for sockaddr_un.
        let mut address: libc::sockaddr_un = unsafe { mem::zeroed() };
        if bytes.len() >= address.sun_path.len() || bytes.contains(&0) {
            bail!("invalid Unix socket path {}", path.display());
        }
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        for (dst, src) in address.sun_path.iter_mut().zip(bytes) {
            *dst = *src as libc::c_char;
        }

        // SAFETY: the arguments select a nonblocking Linux Unix sequenced packet socket.
        let fd = owned(unsafe {
            libc::socket(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
            )
        })?;
        let len =
            (mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as libc::socklen_t;
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                use std::os::unix::fs::FileTypeExt;
                if !meta.file_type().is_socket() {
                    bail!("{} exists and is not a socket", path.display());
                }
                fs::remove_file(&path)?;
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        // SAFETY: address is initialized and len includes its family and NUL-terminated path.
        check(unsafe { libc::bind(fd.as_raw_fd(), (&raw const address).cast(), len) })?;
        let listener = Self {
            fd,
            path,
            _lock: lock,
        };
        // SAFETY: fd is a valid, bound socket.
        check(unsafe { libc::listen(listener.fd.as_raw_fd(), 4) })?;
        Ok(listener)
    }

    pub fn accept(&self) -> io::Result<OwnedFd> {
        // SAFETY: the listener owns its fd and null pointers decline the peer address.
        owned(unsafe {
            libc::accept4(
                self.fd.as_raw_fd(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            )
        })
    }
}

impl AsFd for Listener {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn peer_pid(fd: BorrowedFd<'_>) -> io::Result<u32> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut size = mem::size_of_val(&cred) as libc::socklen_t;
    // SAFETY: the socket is open and cred and size are writable for the syscall.
    check(unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut size,
        )
    })?;
    Ok(cred.pid as u32)
}

/// Sends one packet without blocking. `Ok(false)` means the socket buffer was full and the
/// packet was not sent (the caller retries on the next frame).
pub fn send(fd: BorrowedFd<'_>, packet: &[u8]) -> io::Result<bool> {
    // SAFETY: the socket is open and packet has packet.len() readable bytes.
    let sent = unsafe {
        libc::send(
            fd.as_raw_fd(),
            packet.as_ptr().cast(),
            packet.len(),
            libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
        )
    };
    if sent >= 0 {
        return Ok(sent as usize == packet.len());
    }
    let err = io::Error::last_os_error();
    if err.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(err)
    }
}

/// Whether `pid` is `ancestor` or one of its descendants. A tracker started through a wrapper
/// such as `uv run` is a grandchild of the process the daemon spawned.
pub fn descends_from(mut pid: u32, ancestor: u32) -> bool {
    for _ in 0..16 {
        if pid == ancestor {
            return true;
        }
        match parent_pid(pid) {
            Some(parent) if parent > 1 => pid = parent,
            _ => return false,
        }
    }
    false
}

fn parent_pid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

pub fn recv(fd: BorrowedFd<'_>, buf: &mut [u8]) -> io::Result<usize> {
    // SAFETY: the socket is open and buf has buf.len() writable bytes.
    let len = unsafe {
        libc::recv(
            fd.as_raw_fd(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            libc::MSG_TRUNC,
        )
    };
    if len < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(len as usize)
    }
}

fn check(result: i32) -> io::Result<i32> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

fn owned(fd: i32) -> io::Result<OwnedFd> {
    let fd = check(fd)?;
    // SAFETY: a successful socket or accept call returns a new fd owned by the caller.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub struct Tracker(Child);

impl Tracker {
    pub fn spawn(command: &Path, socket: &Path) -> io::Result<Self> {
        Command::new(command)
            .arg("--socket")
            .arg(socket)
            .spawn()
            .map(Self)
    }

    /// A pidfd that becomes readable when the tracker exits, so the event loop can watch it
    /// without blocking in `wait()`.
    pub fn exit_fd(&self) -> io::Result<OwnedFd> {
        // SAFETY: pidfd_open takes a pid and flags and returns a new fd or -1.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, self.0.id() as libc::pid_t, 0) };
        owned(i32::try_from(fd).map_err(io::Error::other)?)
    }

    pub fn id(&self) -> u32 {
        self.0.id()
    }

    pub fn exited(&mut self) -> io::Result<bool> {
        Ok(self.0.try_wait()?.is_some())
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_process_descends_from_itself_and_its_ancestors_only() {
        let me = std::process::id();
        let parent = parent_pid(me).expect("parent");
        assert!(descends_from(me, me));
        assert!(descends_from(me, parent));
        assert!(!descends_from(parent, me));
    }
}
