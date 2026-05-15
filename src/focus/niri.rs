use anyhow::{Context, Result};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration as StdDuration;

use super::{FocusEvent, FocusSink, FocusSource, FocusedWindow};

pub struct NiriSource;

impl FocusSource for NiriSource {
    fn name(&self) -> &'static str {
        "niri"
    }

    fn run(self: Box<Self>, sink: FocusSink) -> Result<()> {
        loop {
            match spawn_niri_event_stream() {
                Ok(mut child) => {
                    let stdout = match child.stdout.take() {
                        Some(s) => s,
                        None => {
                            eprintln!("attn niri event stream: stdout unavailable");
                            let _ = child.wait();
                            thread::sleep(StdDuration::from_secs(2));
                            continue;
                        }
                    };
                    let reader = BufReader::new(stdout);
                    for line in reader.lines() {
                        let line = match line {
                            Ok(l) => l,
                            Err(error) => {
                                eprintln!("attn niri event stream read error: {error:#}");
                                break;
                            }
                        };
                        if line.trim().is_empty() {
                            continue;
                        }
                        if is_focus_event(&line) {
                            let event = match poll_focused_window() {
                                Ok(Some(w)) => FocusEvent::Focused(w),
                                Ok(None) => FocusEvent::Unfocused,
                                Err(error) => {
                                    eprintln!("attn niri poll error: {error:#}");
                                    continue;
                                }
                            };
                            if sink.send(event).is_err() {
                                return Ok(());
                            }
                        }
                    }
                    let _ = child.wait();
                    thread::sleep(StdDuration::from_secs(1));
                }
                Err(error) => {
                    eprintln!("attn niri event stream unavailable: {error:#}");
                    thread::sleep(StdDuration::from_secs(5));
                    if let Ok(Some(w)) = poll_focused_window() {
                        if sink.send(FocusEvent::Focused(w)).is_err() {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    fn poll_current(&self) -> Result<Option<FocusedWindow>> {
        poll_focused_window()
    }
}

fn spawn_niri_event_stream() -> Result<Child> {
    Command::new("niri")
        .args(["msg", "-j", "event-stream"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to spawn niri event-stream")
}

pub fn is_focus_event(line: &str) -> bool {
    let Ok(Value::Object(event)) = serde_json::from_str::<Value>(line) else {
        return false;
    };

    event.keys().any(|key| {
        matches!(
            key.as_str(),
            "WindowFocusChanged"
                | "WorkspaceActiveWindowChanged"
                | "WorkspaceActivated"
                | "WorkspacesChanged"
        )
    })
}

fn poll_focused_window() -> Result<Option<FocusedWindow>> {
    let output = Command::new("niri")
        .args(["msg", "-j", "focused-window"])
        .output()
        .context("failed to run niri focused-window")?;
    if !output.status.success() {
        return Ok(None);
    }
    let value: Value = serde_json::from_slice(&output.stdout).context("invalid niri JSON")?;
    let Some(app_id) = json_find_string(&value, &["app_id", "app-id", "appId"]) else {
        return Ok(None);
    };
    let window_id = json_find_i64(&value, &["id", "window_id", "window-id", "windowId"]);
    let title =
        json_find_string(&value, &["title", "window_title", "name"]).unwrap_or_default();
    let pid = json_find_i64(&value, &["pid", "process_id", "processId"])
        .and_then(|v| i32::try_from(v).ok());
    Ok(Some(FocusedWindow {
        window_id,
        app_id: crate::normalize_id(&app_id),
        title,
        pid,
    }))
}

fn json_find_string(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = value.get(key).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
        // search one level deeper (e.g. niri wraps in {"FocusedWindow": {...}})
        if let Some(obj) = value.as_object() {
            for inner in obj.values() {
                if let Some(s) = inner.get(key).and_then(|v| v.as_str()) {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

fn json_find_i64(value: &Value, keys: &[&str]) -> Option<i64> {
    for key in keys {
        if let Some(n) = value.get(key).and_then(|v| v.as_i64()) {
            return Some(n);
        }
        if let Some(obj) = value.as_object() {
            for inner in obj.values() {
                if let Some(n) = inner.get(key).and_then(|v| v.as_i64()) {
                    return Some(n);
                }
            }
        }
    }
    None
}
