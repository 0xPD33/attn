# attn Architecture

`attn` is a local attention ledger. It measures where focus actually goes across applications, and for supported browsers which web domains receive that browser time. It is deliberately observational: no blocking, no notifications, no nudges. It records local evidence and exposes a small local status surface plus a configurable break reminder.

The single design principle is that **focused application time is the source of truth**. Browser history is enrichment, only counted while the corresponding browser app actually has focus.

## System shape

```
Niri focus events ────┐
Terminal-subprocess   │
   resolution ────────┤
Browser history ──────┤
Wayland idle-notify ──┼──► attn daemon ──► SQLite ledger
D-Bus PrepareForSleep ┘                    │
                                           └──► Unix socket ──► attn status --json ──► Quickshell
```

The daemon owns all measurement, persistence, attribution, and aggregation. Quickshell only displays already-computed status. Home Manager only deploys binary, config, and the user service.

## Components

### Daemon

Long-running user process. Started on graphical session login via `systemd.user.services.attn`.

Threads:

- **Main / focus loop**: subscribes to `niri msg -j event-stream`, handles `WindowFocusChanged` / `WorkspaceActiveWindowChanged` / `WorkspaceActivated` / `WorkspacesChanged` events. On each event, closes the previously open interval and opens a new one for the now-focused window.
- **Terminal poll** (`terminals.poll_secs`, default 10 s): re-runs focus resolution. Catches the case where the focused window is unchanged but a new program launched inside it (e.g. you started `claude` inside an already-focused terminal).
- **Browser import** (`poll_interval_secs`, default 60 s): runs the browser history pipeline (snapshot → read → clip → write).
- **Socket server**: per-connection threads serving `status`, `reload`, `break_start`, `break_end`, `set_breaks` requests on `$XDG_RUNTIME_DIR/attn.sock`.
- **Heartbeat** (30 s): writes `meta.last_heartbeat_at`. On startup, if last heartbeat is `>60 s` stale, any still-open interval is retroactively capped at the heartbeat timestamp. Catches ungraceful suspend / crash / kill.
- **D-Bus listener**: subscribes to `org.freedesktop.login1.Manager.PrepareForSleep`. On `(true)` (imminent suspend), closes the current open interval cleanly.
- **Wayland idle-notify**: binds `ext_idle_notifier_v1`, creates a notification at `breaks.min_break_secs * 1000` ms. On `idled` event, auto-pauses tracking with `reason = idle`. On `resumed`, clears the idle pause and reopens an interval for the current focus. Reconnects with backoff on Wayland connection loss.
- **Config watcher** (3 s mtime poll): on `~/.config/attn/config.toml` save, calls the same path as `attn reload`. Eliminates the manual reload step.
- **Signal handler**: on SIGINT/SIGTERM, closes the open interval cleanly so totals on next start are correct.

The daemon is resilient. Missing browsers, unreadable History files, malformed individual visits, or Wayland/D-Bus connection failures degrade specific features but do not stop app tracking. Only state-DB-init failure causes exit.

### CLI

Single binary, multiple subcommands:

```
attn daemon              run the long-running collector
attn status --json       print current-day + 7-day status from the socket
attn reload              ask the daemon to reload config without restarting
attn init [--force]      write the default config to ~/.config/attn/config.toml
attn doctor              check niri, config, state DB, wayland, dbus, browsers, socket
attn break-start         pause tracking manually
attn break-end           resume tracking
attn set-breaks [...]    update break-reminder settings without editing TOML
```

The CLI hides socket protocol details from Quickshell and shell scripts.

### State database

SQLite at `~/.local/state/attn/attn.sqlite` (overridable in config). WAL journal mode. Raw intervals are source-of-truth; daily totals and the meta table are derived / mutable state.

```sql
CREATE TABLE app_intervals (
  id            INTEGER PRIMARY KEY,
  started_at    TEXT NOT NULL,
  ended_at      TEXT,
  app_id        TEXT NOT NULL,
  window_title  TEXT NOT NULL DEFAULT '',
  window_id     INTEGER,
  idle_adjusted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE domain_intervals (
  id              INTEGER PRIMARY KEY,
  started_at      TEXT NOT NULL,
  ended_at        TEXT NOT NULL,
  browser_app_id  TEXT NOT NULL,
  browser_name    TEXT NOT NULL DEFAULT '',
  domain          TEXT NOT NULL,
  url             TEXT NOT NULL DEFAULT '',
  source_profile  TEXT NOT NULL DEFAULT ''
);

CREATE TABLE daily_app_totals (
  day             TEXT NOT NULL,
  app_id          TEXT NOT NULL,
  seconds         INTEGER NOT NULL,
  watch_category  TEXT,
  PRIMARY KEY (day, app_id)
);

CREATE TABLE daily_domain_totals (
  day             TEXT NOT NULL,
  domain          TEXT NOT NULL,
  seconds         INTEGER NOT NULL,
  watch_category  TEXT,
  PRIMARY KEY (day, domain)
);

CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE INDEX app_intervals_started_idx       ON app_intervals(started_at);
CREATE INDEX app_intervals_app_started_idx   ON app_intervals(app_id, started_at);
CREATE INDEX domain_intervals_started_idx    ON domain_intervals(started_at);
```

The `meta` table holds: `last_heartbeat_at`, `paused_at`, `paused_reason`, `session_reset_at`, `breaks_enabled`, `breaks_interval_secs`, `breaks_min_break_secs`. Schema is migrated on startup. Missing columns are added in place; existing data is preserved.

### Quickshell widget

Three QML files:

- `AttnIndicator.qml`: bar chip that polls `attn status --json`. Initial polls run every 800 ms (up to three) so the chip appears quickly on startup, then settles to 5 s. Dims while stale, brightens when fresh. Border + opacity pulse amber when `break_overdue` or `paused`.
- `AttnPopup.qml`: popup rendered when the chip is clicked. Two view modes:
  - **Today**: stats card (total + stacked category bar + legend), top-item pill row, category filter chips, side-by-side Apps / Domains lists.
  - **Week**: 7 day rows (today + previous 6), each with date label, total duration, stacked horizontal category bar, and per-day category legend.
- Settings sheet (gear icon → overlay) with break-reminder toggle, interval picker (30 m / 1 h / 90 m / 2 h / 3 h), and idle-threshold picker (2 m / 5 m / 10 m / 15 m). Changes apply on click via `attn set-breaks`.
- Inline overdue banner appears at the top when `break_overdue && !paused`, leaving the data visible.
- Full break overlay appears only when actually paused (manual or idle).
- `AttnRow.qml`: single row used inside the Today lists (icon, label, mini bar, duration).

The widget is pure presentation. It does not read SQLite, does not classify watch categories, does not run attribution logic. It receives a self-describing JSON blob and renders it.

## Focus tracking

### Layer 1: niri events

The daemon subscribes to `niri msg -j event-stream` and reacts to focus-change events. On each event it queries `niri msg -j focused-window` and extracts `app_id`, `title`, and `pid`.

### Layer 2: terminal subprocess resolution

If the focused window is a terminal emulator (matched against `apps.watch.terminal`), the daemon attempts to identify the active program running inside it. Three strategies, in order:

1. **Title match**. The niri-reported window title is scanned for any program name configured under `[terminals.apps]`. Bounded-word matching is used so `clauderyx` does not match `claude`. Modern terminals set the OSC 0/2 title to the foreground program's argv0; this path is fast and high-confidence.
2. **Tmux query**. If a tmux client process is found in the focused window's descendant tree, `tmux list-clients` is invoked. The matching client's `pane_current_command` is checked against `[terminals.apps]`. This handles the common case of running tmux inside a terminal. Without tmux running, this path is skipped.
3. **Descendant scan**. Walk `/proc/<pid>/task/*/children` recursively from the focused window's pid, collecting basenames and start-times. The most recently started subprocess matching `[terminals.apps]` wins.

When any layer matches, the `app_id` of the resulting interval is replaced with the resolved subprocess name (e.g. `claude`, `codex`, `nvim`). If none match, the original terminal `app_id` is kept.

### Idle handling

Two layers, layered on each other:

- **Interval cap** (`idle_after_secs`, default 300 s): if the focused window does not change for the configured interval, the open interval is capped past its start. The capped row is marked `idle_adjusted = 1` so reports can distinguish observed focus from capped focus.
- **Compositor idle** (Wayland `ext-idle-notify-v1`, `breaks.min_break_secs`): real input idle (no mouse/keyboard). On `idled`, tracking is auto-paused (`paused_reason = idle`). On `resumed`, the pause clears automatically and a fresh interval opens for the current focus. The pause state is persisted in `meta` so it survives daemon restart.

## Browser readers

V1 supports Chromium-family browsers: Helium, Brave, Chrome, Chromium. Each is configured in `[browsers.<name>]` with `app_ids`, `history_paths` (glob), and `kind = "chromium"`.

Browser history is **never read live** from the user's profile. The pipeline is:

1. Discover History DBs via glob expansion.
2. Snapshot-copy the SQLite file (and `-wal` / `-shm` sidecars) to a temp path.
3. Query the snapshot's `urls` JOIN `visits` table.
4. Convert Chromium microsecond timestamps (epoch 1601-01-01 UTC) to UTC.
5. Cap individual visits at the next visit's start time (to bound long-duration outliers from tab pinning).
6. Delete the snapshot.

This avoids any lock contention with the running browser. On daemon startup, any stale `attn-*` snapshot files in `$TMPDIR` older than 1 hour are cleaned up.

### Domain clipping

For each browser visit, the daemon finds overlapping focused intervals for that browser's app and stores only the overlap as a `domain_interval`. Domain time can therefore never grow while the browser is unfocused.

For domain time that the browser reported but no focused interval covers, the daemon inserts an uncovered-segment row only when it overlaps a focused interval of *any* configured browser of the same name. (See `insert_uncovered_domain_segments` in `src/main.rs`.)

On import, all imported rows for the day are first deleted, then rewritten. This keeps the table consistent even when browser history is amended or reordered between polls.

## Watch lists & categories

Two independent namespaces in the config:

```toml
[apps.watch]
coding   = ["code", "cursor", ...]
terminal = ["com.mitchellh.ghostty", "wezterm", ...]
chat     = ["discord", "signal", ...]

[domains.watch]
coding = ["github.com", "stackoverflow.com", ...]
ai     = ["chatgpt.com", "claude.ai", ...]
video  = ["youtube.com", "youtu.be", ...]
```

Plus a terminal-subprocess namespace that doubles as a category source:

```toml
[terminals.apps]
ai     = ["claude", "codex", "aichat"]
editor = ["nvim", "vim", "hx", "helix", "emacs"]
```

When resolving the category for an app id, `apps.watch` is consulted first; if no hit, `terminals.apps` provides the fallback. This means a focused interval with `app_id = "claude"` lands in the `ai` category without needing a duplicate entry under `apps.watch.ai`.

Domain matching supports exact match and suffix match (`www.youtube.com` matches `youtube.com`), but never substring (`notyoutube.com` does not match `youtube.com`).

The shipped default config covers 22+ categories: coding, ai, design, media, productivity, chat, email, storage, meeting, video, music, scroll, news, shopping, finance, learning, search, reference, devops, travel, food, sports, health, read_later. Each category has a distinct hue in the popup palette.

The shipped defaults are split across the `config/` tree so contributors can edit a single category without touching unrelated config:

- `config/runtime.toml` — non-list config (paths, intervals, browsers, terminals, breaks)
- `config/apps/<category>.txt` — one app ID per line per category (`#` comments allowed)
- `config/domains/<category>.txt` — one domain per line per category
- `config/default.toml` — generated by `tools/sync-default-config.sh` from the above

Both the Rust daemon (via `include_str!`) and the Nix flake (via `builtins.readFile`) embed `config/default.toml`. CI runs `tools/sync-default-config.sh --check` to ensure it stays in sync with the per-category sources.

## Break reminder

When the daemon's `[breaks]` config is enabled, status includes `active_session_seconds` and `break_overdue` (`active_session_seconds >= interval_secs && !paused`). The widget shows a subtle banner inviting a break; clicking the button (or running `attn break-start`) sets `paused_reason = manual`, closes the open interval, and stops opening new ones. `attn break-end` clears the pause, persists `session_reset_at = now` so the next session counter starts at zero, and reopens an interval for the current focus.

Auto-pause from Wayland idle uses the same state but with `paused_reason = idle` and auto-clears on the next `resumed` event.

`attn set-breaks --enabled --interval --min-break` (or the in-popup settings sheet) writes overrides into `meta`, which the daemon layers on top of the TOML config at load time.

### Active session computation

`active_session_seconds` walks `app_intervals` newest-first and accumulates duration until it hits a gap `>= min_break_secs` or crosses `meta.session_reset_at`. Two defenses make this robust even when Wayland idle events are missed:

- **Per-interval cap**: each interval's effective end is capped at `started_at + idle_after_secs`. A focused window left open while the user is away (no input, no focus change) creates an effective gap the walk then detects as a break, even without a Wayland `idled` event.
- **`session_reset_at` marker**: set on `break_end` and on Wayland `resumed` (after an idle pause), it acts as a hard boundary the walk cannot cross.

## Daily totals & rebuild cooldown

`daily_app_totals` and `daily_domain_totals` are derived from the interval tables. They are deleted and rebuilt on demand inside a single SQLite transaction, covering both today and yesterday so the cross-midnight slice of the still-open interval lands in yesterday's total.

A 3-second cooldown gates the rebuild. After the first rebuild, subsequent `status` requests within 3 s reuse the cached totals; a request older than 3 s triggers a fresh rebuild. With the indicator polling every 5 s, this means at most one rebuild per poll. Steady-state status responses are ~1 ms.

The status response also includes a 7-day summary (`days[]`) populated from `daily_*_totals` so the Week view never re-scans intervals.

## Socket API

Unix socket at `$XDG_RUNTIME_DIR/attn.sock`, mode 0700 directory + user-only socket.

Requests are newline-terminated command strings. Responses are JSON, newline-terminated.

### `status`

```json
{
  "date": "2026-05-14",
  "updated_at": "2026-05-14T10:00:00+02:00",
  "watch_seconds": 15563,
  "tracked_seconds": 16928,
  "apps":      [ { "id": "...", "seconds": 0, "watched": true, "category": "..." } ],
  "domains":   [ { "domain": "...", "seconds": 0, "watched": true, "category": "..." } ],
  "categories":[ { "name": "ai", "seconds": 6979 } ],
  "days": [
    { "date": "2026-05-14", "tracked_seconds": 0, "watch_seconds": 0,
      "categories": [ { "name": "ai", "seconds": 0 } ] }
    /* 7 entries, today + previous 6 */
  ],
  "active_session_seconds": 1820,
  "break_overdue": false,
  "paused": false,
  "paused_reason": null,
  "paused_since": null,
  "breaks_enabled": true,
  "breaks_interval_secs": 3600,
  "breaks_min_break_secs": 300
}
```

- `watch_seconds` = `(watched non-browser app seconds) + (watched domain seconds)`. The non-browser exclusion prevents double-counting browser focus when both the browser itself and its domains are in watch lists.
- `tracked_seconds` is the sum of all category seconds.
- `apps` and `domains` lists include all today's items, watched and unwatched. The widget filters.

### `reload`

Reload `~/.config/attn/config.toml` without restarting. Re-resolves state path and socket path; reopens the SQLite connection if the state path changed. Responds with `{ "ok": true, "state_reopened": bool, "socket_restart_required": bool }`. Also fires automatically when the config file's mtime changes.

### `break_start` / `break_end`

Toggle manual pause. Both return `{ "ok": true, "paused": bool, "paused_reason": "manual"|null }`.

### `set_breaks <enabled> <interval_secs> <min_break_secs>`

Persist break-reminder overrides to `meta` and apply in-memory. Returns the new values. The Quickshell settings sheet is the primary consumer; `attn set-breaks` is a CLI wrapper.

## Avoiding double-counting

App time and domain time are different views over the same attention. A 30-minute browser focus interval can appear as both:

- app time: `brave = 30 min`
- domain time: `youtube.com = 20 min`, `github.com = 10 min`

For display purposes:

```
watch_seconds = watched non-browser app seconds + watched domain seconds
```

The browser's app total is still surfaced in the apps list (so you can see "30 min in Brave"), but it is excluded from the `watch_seconds` headline number. This is opinionated; revisit if the headline should mean something else.

## Performance

The daemon is intended to run continuously at near-zero cost.

Hot paths:

- Niri event handling is event-driven, not polled.
- Status responses are served from `daily_*_totals` and gated by the 3 s rebuild cooldown, typically ~1 ms after the first call.
- Browser history scanning runs on a timer (default 60 s), not per status request.
- Live browser DB connections are open only long enough to copy the file.
- Terminal subprocess resolution is at most a small `/proc` walk plus one `tmux list-clients` invocation per focus change or 10 s tick.
- Wayland idle and D-Bus listeners block on their respective event sources; they do not poll.

Steady-state memory is ~40 MB and CPU is ~0% between focus events.

## Privacy & safety

- No network access. No telemetry. No cloud sync.
- All state local. State DB and socket are mode 0600 / 0700 user-only.
- The browser reader records the URL host (domain) and the visit URL. Full URLs are stored to allow attribution debugging, but the user-facing surfaces (status JSON, popup) only show domains.
- No browser credential or content scraping.
- Window titles are recorded for app intervals because terminal-subprocess resolution depends on them; window titles are not surfaced in the JSON.

## Failure modes

| Failure | Behavior |
|---------|----------|
| Niri IPC unavailable at startup | Daemon retries after a short delay; app tracking pauses meanwhile. |
| Browser profile missing or History unreadable | Skipped for this poll cycle. Logged. App tracking continues. |
| Browser History schema differs from Chromium layout | Skipped. Logged. |
| Config has a bad watch-list entry | Default values fall back in. Logged. |
| Wayland `ext-idle-notify-v1` unavailable | Auto-pause-on-idle disabled. Manual break-start/break-end still work. `attn doctor` reports it. |
| D-Bus `org.freedesktop.login1` unreachable | Suspend/wake detection falls back to the heartbeat thread's retroactive-cap path. Logged. |
| Daemon hard-killed mid-session | On restart, heartbeat staleness retroactively caps any still-open interval. |
| System suspended | D-Bus signal closes interval cleanly; on resume, focus event reopens. If D-Bus missed it, heartbeat catches up. |
| Socket path already exists after unclean shutdown | Stale path is removed if it doesn't resolve to a live listener. |
| State DB init fails | Daemon exits. This is the only fatal error. |

`attn doctor` probes each surface and prints a `verdict: ok` / `verdict: errors found` summary.

## Time windows

V1 reports the current local day plus the previous 6 days. All stored timestamps are UTC ISO-8601; local-day grouping happens at aggregation time using the user's local timezone. Rolling windows beyond the 7-day summary are not implemented.

## Extension points

Deliberately deferred:

- Zen / Firefox `places.sqlite` reader.
- Per-tab active-URL tracking via a browser extension.
- Sway / Hyprland / KWin focus providers (V1 is niri-only).
- Monthly aggregates.
- Per-category budgets with explicit thresholds (config has a `[budgets]` table, but no UI consumes it yet).
- A small local TUI.
- `config.d/*.toml` drop-ins.
- In-popup add-to-watchlist gesture.

These should not complicate the V1 daemon.
