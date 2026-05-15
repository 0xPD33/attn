/// River / Sway focus adapter via `zwlr_foreign_toplevel_manager_v1`.
///
/// Binds the wlr-foreign-toplevel-management protocol and listens for
/// `activated` state changes.  When a toplevel becomes activated we emit
/// `FocusEvent::Focused`; when no toplevel is activated we emit
/// `FocusEvent::Unfocused`.
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::thread;
use std::time::Duration as StdDuration;

use wayland_client::{
    backend::ObjectId,
    protocol::{wl_registry, wl_seat},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use super::{FocusEvent, FocusSink, FocusSource, FocusedWindow};

pub struct WlrToplevelSource;

impl FocusSource for WlrToplevelSource {
    fn name(&self) -> &'static str {
        "wlr_toplevel"
    }

    fn run(self: Box<Self>, sink: FocusSink) -> Result<()> {
        loop {
            if let Err(error) = run_toplevel_loop(&sink) {
                eprintln!("attn wlr_toplevel error: {error:#}");
            }
            thread::sleep(StdDuration::from_secs(5));
            if sink.send(FocusEvent::Unfocused).is_err() {
                return Ok(());
            }
        }
    }

    fn poll_current(&self) -> Result<Option<FocusedWindow>> {
        // There is no one-shot query for wlr-foreign-toplevel; return None
        // so the daemon skips the initial open_app_interval call.  The first
        // activate event will open an interval shortly after startup.
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Internal state shared across Wayland dispatch callbacks
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Debug)]
struct ToplevelInfo {
    app_id: Option<String>,
    title: Option<String>,
    pid: Option<i32>,
    activated: bool,
}

struct ToplevelState {
    manager: Option<ZwlrForeignToplevelManagerV1>,
    toplevels: HashMap<ObjectId, ToplevelInfo>,
    /// Proxy id of the currently activated toplevel (if any).
    active_id: Option<ObjectId>,
    sink: FocusSink,
}

impl ToplevelState {
    fn emit_current(&self) {
        let event = match self.active_id.as_ref().and_then(|id| self.toplevels.get(id)) {
            Some(info) => {
                let app_id = info
                    .app_id
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| {
                        // Fallback: read /proc/<pid>/comm if pid is known
                        info.pid
                            .and_then(|pid| {
                                std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()
                            })
                            .map(|s| s.trim().to_string())
                            .unwrap_or_default()
                    });
                if app_id.is_empty() {
                    FocusEvent::Unfocused
                } else {
                    FocusEvent::Focused(FocusedWindow {
                        window_id: None,
                        app_id: crate::normalize_id(&app_id),
                        title: info.title.clone().unwrap_or_default(),
                        pid: info.pid,
                    })
                }
            }
            None => FocusEvent::Unfocused,
        };
        let _ = self.sink.send(event);
    }
}

// ---------------------------------------------------------------------------
// Wayland dispatch implementations
// ---------------------------------------------------------------------------

impl Dispatch<wl_registry::WlRegistry, ()> for ToplevelState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "zwlr_foreign_toplevel_manager_v1" {
                let manager: ZwlrForeignToplevelManagerV1 =
                    registry.bind(name, version.min(3), qh, ());
                state.manager = Some(manager);
            }
        }
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for ToplevelState {
    fn event(
        state: &mut Self,
        _manager: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                let id = toplevel.id();
                state.toplevels.insert(id, ToplevelInfo::default());
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {}
            _ => {}
        }
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for ToplevelState {
    fn event(
        state: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let id = handle.id();
        match event {
            zwlr_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                if let Some(info) = state.toplevels.get_mut(&id) {
                    info.app_id = Some(app_id);
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Title { title } => {
                if let Some(info) = state.toplevels.get_mut(&id) {
                    info.title = Some(title);
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::State { state: raw } => {
                let activated = raw
                    .chunks_exact(4)
                    .any(|b| {
                        let v = u32::from_ne_bytes([b[0], b[1], b[2], b[3]]);
                        v == zwlr_foreign_toplevel_handle_v1::State::Activated as u32
                    });
                if let Some(info) = state.toplevels.get_mut(&id) {
                    info.activated = activated;
                }
                if activated {
                    let prev = state.active_id.clone();
                    state.active_id = Some(id.clone());
                    if prev.as_ref() != Some(&id) {
                        state.emit_current();
                    }
                } else if state.active_id.as_ref() == Some(&id) {
                    state.active_id = None;
                    let _ = state.sink.send(FocusEvent::Unfocused);
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Done => {}
            zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                state.toplevels.remove(&id);
                if state.active_id.as_ref() == Some(&id) {
                    state.active_id = None;
                    let _ = state.sink.send(FocusEvent::Unfocused);
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::OutputEnter { .. } => {}
            zwlr_foreign_toplevel_handle_v1::Event::OutputLeave { .. } => {}
            zwlr_foreign_toplevel_handle_v1::Event::Parent { .. } => {}
            _ => {}
        }
    }
}

// wl_seat is required by the manager binding but we don't use it for events
impl Dispatch<wl_seat::WlSeat, ()> for ToplevelState {
    fn event(
        _state: &mut Self,
        _seat: &wl_seat::WlSeat,
        _event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

fn run_toplevel_loop(sink: &FocusSink) -> Result<()> {
    let conn = Connection::connect_to_env().context("wayland connect")?;
    let mut event_queue = conn.new_event_queue::<ToplevelState>();
    let qh = event_queue.handle();
    let display = conn.display();
    display.get_registry(&qh, ());

    let mut state = ToplevelState {
        manager: None,
        toplevels: HashMap::new(),
        active_id: None,
        sink: sink.clone(),
    };

    event_queue.roundtrip(&mut state).context("wayland roundtrip")?;
    event_queue.roundtrip(&mut state).context("wayland roundtrip 2")?;

    if state.manager.is_none() {
        anyhow::bail!(
            "compositor does not support zwlr_foreign_toplevel_manager_v1"
        );
    }

    loop {
        event_queue
            .blocking_dispatch(&mut state)
            .context("wayland dispatch")?;
    }
}
