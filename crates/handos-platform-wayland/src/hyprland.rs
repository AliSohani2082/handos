//! The one Hyprland query M1 needs: which monitor has focus (the `active-output` mapping target).

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
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
    let result = (|| {
        let mut stream = UnixStream::connect(&path)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        stream.write_all(request.as_bytes())?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply)?;
        Ok::<_, std::io::Error>(reply)
    })();
    match result {
        Ok(reply) => Some(reply),
        Err(err) => {
            tracing::debug!(path = %path.display(), %err, "hyprland query failed");
            None
        }
    }
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
    fn anything_else_is_none() {
        assert_eq!(parse_focused("[]"), None);
        assert_eq!(parse_focused("not json"), None);
        assert_eq!(parse_focused(r#"[{"name":"DP-1","focused":false}]"#), None);
        assert_eq!(parse_focused(r#"{"name":"DP-1","focused":true}"#), None);
    }
}
