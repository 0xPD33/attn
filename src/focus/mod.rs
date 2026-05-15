use anyhow::Result;
use std::sync::mpsc;

pub mod hyprland;
pub mod niri;
pub mod wlr_toplevel;

/// A window that currently has compositor focus.
#[derive(Clone, Debug)]
pub struct FocusedWindow {
    pub window_id: Option<i64>,
    pub app_id: String,
    pub title: String,
    pub pid: Option<i32>,
}

/// Events emitted by a focus adapter.
pub enum FocusEvent {
    Focused(FocusedWindow),
    Unfocused,
}

pub type FocusSink = mpsc::Sender<FocusEvent>;

pub trait FocusSource: Send {
    fn name(&self) -> &'static str;
    /// Block in a loop, sending events to `sink` until an unrecoverable error.
    fn run(self: Box<Self>, sink: FocusSink) -> Result<()>;
    /// One-shot query of the currently focused window (used at startup).
    fn poll_current(&self) -> Result<Option<FocusedWindow>>;
}

/// Probe environment variables and return the best focus source name.
/// Priority: NIRI_SOCKET > HYPRLAND_INSTANCE_SIGNATURE > SWAYSOCK > WAYLAND_DISPLAY (river).
pub fn auto_detect() -> &'static str {
    if std::env::var_os("NIRI_SOCKET").is_some() {
        return "niri";
    }
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        return "hyprland";
    }
    if std::env::var_os("SWAYSOCK").is_some() {
        return "sway";
    }
    "river"
}

/// Build the focus adapter for `kind` ("auto" resolves via [`auto_detect`]).
pub fn build(kind: &str) -> Result<Box<dyn FocusSource>> {
    let resolved = if kind == "auto" { auto_detect() } else { kind };
    match resolved {
        "niri" => Ok(Box::new(niri::NiriSource)),
        "hyprland" => Ok(Box::new(hyprland::HyprlandSource)),
        "river" | "sway" => Ok(Box::new(wlr_toplevel::WlrToplevelSource)),
        other => anyhow::bail!("unknown focus_source.kind: {other}"),
    }
}
