#![allow(clippy::expect_used, unsafe_code)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn daemon_records_valid_packets_and_releases_runtime_socket() {
    let runtime = tempfile::tempdir().expect("temporary runtime directory");
    let tracker = runtime.path().join("tracker");
    let trace = runtime.path().join("live.htrace");
    // Send a bad packet and a valid frame over the real tracker socket.
    fs::write(
        &tracker,
        r#"#!/usr/bin/env python3
import socket, struct, sys
sock = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
sock.connect(sys.argv[2])
sock.send(b"\xff")
sock.send(struct.pack("<BBBBxIQQ", 1, 1, 0, 0, 0, 10, 20))
sock.recv(1024)
"#,
    )
    .expect("write stub tracker");
    fs::set_permissions(&tracker, fs::Permissions::from_mode(0o700)).expect("make stub executable");

    let binary = env!("CARGO_BIN_EXE_handos");
    let mut daemon = Daemon(
        Command::new(binary)
            .arg("daemon")
            .arg("--headless")
            .arg("--tracker")
            .arg(&tracker)
            .arg("--record")
            .arg(&trace)
            .env("XDG_RUNTIME_DIR", runtime.path())
            .stdout(Stdio::null())
            .spawn()
            .expect("start daemon"),
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while fs::metadata(&trace).map_or(0, |meta| meta.len()) < 45 {
        assert!(
            Instant::now() < deadline,
            "tracker did not write a complete trace packet"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let second = Command::new(binary)
        .arg("daemon")
        .arg("--headless")
        .env("XDG_RUNTIME_DIR", runtime.path())
        .output()
        .expect("start second daemon");
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("daemon already running"));

    // SAFETY: the test owns this running child process.
    assert_eq!(
        unsafe { libc::kill(daemon.0.id() as libc::pid_t, libc::SIGTERM) },
        0
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = daemon.0.try_wait().expect("poll daemon") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "daemon did not exit after SIGTERM"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success());
    assert!(!runtime.path().join("handos/tracker.sock").exists());
    assert_eq!(fs::metadata(&trace).expect("trace recorded").len(), 45);
}
