use anyhow::{Context, Result};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::thread;
use std::time::Duration as StdDuration;

use super::{FocusEvent, FocusSink, FocusSource, FocusedWindow};

pub struct HyprlandSource;

impl FocusSource for HyprlandSource {
    fn name(&self) -> &'static str {
        "hyprland"
    }

    fn run(self: Box<Self>, sink: FocusSink) -> Result<()> {
        loop {
            match connect_socket2() {
                Ok(stream) => {
                    let reader = BufReader::new(stream);
                    for line in reader.lines() {
                        let line = match line {
                            Ok(l) => l,
                            Err(error) => {
                                eprintln!("attn hyprland event stream read error: {error:#}");
                                break;
                            }
                        };
                        if is_focus_event_line(&line) {
                            let event = match query_active_window() {
                                Ok(Some(w)) => FocusEvent::Focused(w),
                                Ok(None) => FocusEvent::Unfocused,
                                Err(error) => {
                                    eprintln!("attn hyprland activewindow error: {error:#}");
                                    continue;
                                }
                            };
                            if sink.send(event).is_err() {
                                return Ok(());
                            }
                        }
                    }
                    thread::sleep(StdDuration::from_secs(1));
                }
                Err(error) => {
                    eprintln!("attn hyprland socket unavailable: {error:#}");
                    thread::sleep(StdDuration::from_secs(5));
                    // fallback poll
                    if let Ok(Some(w)) = query_active_window() {
                        if sink.send(FocusEvent::Focused(w)).is_err() {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    fn poll_current(&self) -> Result<Option<FocusedWindow>> {
        query_active_window()
    }
}

fn socket2_path() -> Result<std::path::PathBuf> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .context("HYPRLAND_INSTANCE_SIGNATURE not set")?;
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".into());
    Ok(std::path::PathBuf::from(format!(
        "{runtime}/hypr/{sig}/.socket2.sock"
    )))
}

fn connect_socket2() -> Result<UnixStream> {
    let path = socket2_path()?;
    UnixStream::connect(&path)
        .with_context(|| format!("failed to connect to hyprland socket2 at {}", path.display()))
}

/// Hyprland emits `event>>data` lines on socket2.
/// We care about `activewindow` and `activewindowv2`.
pub fn is_focus_event_line(line: &str) -> bool {
    line.starts_with("activewindow>>") || line.starts_with("activewindowv2>>")
}

fn query_active_window() -> Result<Option<FocusedWindow>> {
    let output = Command::new("hyprctl")
        .args(["-j", "activewindow"])
        .output()
        .context("failed to run hyprctl -j activewindow")?;
    if !output.status.success() {
        return Ok(None);
    }
    let value: Value =
        serde_json::from_slice(&output.stdout).context("invalid hyprctl JSON")?;

    // hyprctl returns `{}` or a window object
    if !value.is_object() || value.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        return Ok(None);
    }

    // app_id: prefer "initialClass" / "class"; fallback to "title"
    let app_id = value
        .get("initialClass")
        .or_else(|| value.get("class"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            value
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
        })
        .to_string();

    if app_id.is_empty() {
        return Ok(None);
    }

    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let pid = value
        .get("pid")
        .and_then(|v| v.as_i64())
        .and_then(|n| i32::try_from(n).ok());

    // Hyprland window address is a hex string like "0x55f3a2b40c00"
    let window_id = value
        .get("address")
        .and_then(|v| v.as_str())
        .and_then(|s| {
            let hex = s.trim_start_matches("0x");
            i64::from_str_radix(hex, 16).ok()
        });

    Ok(Some(FocusedWindow {
        window_id,
        app_id: crate::normalize_id(&app_id),
        title,
        pid,
    }))
}
