# attn Architecture

`attn` is a local attention ledger. It measures where focus actually goes across applications, for supported browsers which web domains receive that browser time, and which video domains are actively playing while a browser is focused. It is deliberately observational: no blocking, no gating, no nudges, no scores, no streaks. The one thing it will do unprompted is emit an opt-in desktop notification when a break is overdue or a daily category budget is exceeded. It records local evidence and exposes a small local status surface; it never intervenes.

The single design principle is that **focused application time is the source of truth**. Browser history and MPRIS playback are enrichment, only counted while the corresponding browser app actually has focus.

## System shape

```
Compositor focus events ─┐    (niri / Hyprland / Sway / river)
Terminal-subprocess      │
   resolution ───────────┤
Browser history ─────────┤    (Chromium-family + Firefox-family)
D-Bus MPRIS players ─────┤
Wayland idle-notify ─────┼──► attn daemon ──► SQLite ledger
D-Bus PrepareForSleep ───┘                     │
                                               ├──► Unix socket ──► attn status --json ──► Quickshell
                                               ├──► D-Bus Notifications (break-overdue, budget-exceeded)
                                               └──► attn export ──► CSV on stdout (reads the DB directly)
```

The daemon owns all measurement, persistence, attribution, and aggregation. Quickshell only displays already-computed status. Home Manager only deploys binary, config, and the user service.

## Components

### Daemon

Long-running user process. Started on graphical session login via `systemd.user.services.attn`.

Threads:

- **Main / focus loop**: dispatched through the `FocusSource` trait, with one of four adapters (niri / Hyprland / Sway / river) chosen by `focus_source.kind` (default `auto`). The niri adapter consumes `niri msg -j event-stream` and handles `WindowFocusChanged` / `WorkspaceActiveWindowChanged` / `WorkspaceActivated` / `WorkspacesChanged` events. The Hyprland adapter reads the socket2 event stream and re-queries `hyprctl -j activewindow`. The wlr-foreign-toplevel adapter (Sway, river) tracks `activated` state per toplevel. On each focus change, the daemon closes the previously open interval and opens a new one for the now-focused window. The adapter itself runs on its own thread and feeds an `mpsc` channel the main loop drains.
- **Terminal poll** (`terminals.poll_secs`, default 10 s): re-runs focus resolution. Catches the case where the focused window is unchanged but a new program launched inside it (e.g. you started `claude` inside an already-focused terminal). It never ends a pause.
- **Browser import** (`poll_interval_secs`, default 60 s): runs the browser history pipeline (snapshot → read → clip → write).
- **MPRIS media poll** (2 s, not configurable): enumerates `org.mpris.MediaPlayer2.*` names on the session bus and opens/closes `media_intervals` rows for players that are actually playing in the focused browser.
- **Socket server**: per-connection threads serving `status`, `reload`, `break_start`, `break_end`, `set_breaks`, `categorize`, `set_budget`, `set_notifications` and `set_focus_source` requests on `$XDG_RUNTIME_DIR/attn.sock`.
- **Heartbeat** (30 s): writes `meta.last_heartbeat_at`. On startup, if last heartbeat is `>60 s` stale, any still-open interval is retroactively capped at the heartbeat timestamp. Catches ungraceful suspend / crash / kill.
- **Notification check** (60 s): evaluates break-overdue and per-category budget-exceeded conditions and pushes at most one notification per session / per category per day. See [Notifications](#notifications).
- **D-Bus sleep listener**: subscribes to `org.freedesktop.login1.Manager.PrepareForSleep` on the system bus. On `(true)` (imminent suspend), closes the current open interval cleanly. Reconnects after 10 s on error.
- **Wayland idle-notify**: binds `ext_idle_notifier_v1`, creates a notification at `breaks.min_break_secs * 1000` ms. At notifier version 2 the notification is input-only, so idle inhibitors cannot hold it back. On `idled` event, auto-pauses tracking with `reason = idle` and ends focus at the last input. If a tracked video plays at that moment, the pause waits until the video ends. On `resumed`, clears the pause of either reason, stamps `session_reset_at`, and reopens an interval for the current focus. See [Idle handling](#idle-handling). Retries every 5 s on Wayland connection loss, and re-subscribes the same way once `set-breaks` changes `min_break_secs`.
- **Config watcher** (3 s mtime poll): on `~/.config/attn/config.toml` save, calls the same path as `attn reload`. Eliminates the manual reload step. It compares the mtime of the file and of the path itself. A home-manager config is a symlink into the Nix store, where every file has mtime 1, so only the link's mtime changes on a switch.
- **Signal handler**: on SIGINT/SIGTERM, closes the open app interval and any open media intervals cleanly and unlinks the socket, so totals on next start are correct. A second signal exits immediately.

The daemon is resilient once running: missing browsers, unreadable History files, malformed individual visits, an absent session bus, a lost compositor event stream, or Wayland/D-Bus connection failures degrade specific features but never stop app tracking, and no subsystem error terminates the process. Only *startup* can be fatal — an unparseable or invalid config, a state DB that won't open, or a focus source that won't answer its first probe. In each of those cases the process exits non-zero and systemd (`Restart=on-failure`, `RestartSec=3`) retries.

### CLI

Single binary, multiple subcommands:

```
attn daemon              run the long-running collector
attn status --json       print current-day + 7-day status from the socket
attn reload              ask the daemon to reload config without restarting
attn init [--force|--merge]
                         write or merge the default config at ~/.config/attn/config.toml
attn doctor              check config, state DB, focus source, wayland idle, dbus login1,
                         dbus notifications, browser history DBs, socket
attn break-start         pause tracking manually
attn break-end           resume tracking
attn set-breaks [...]    update break-reminder settings without editing TOML
attn categorize          assign an uncategorized app/domain to a category (writes config.toml)
attn set-budget          set or clear a daily budget for a category (writes config.toml)
attn set-notifications   toggle notification flags without editing TOML (writes config.toml)
attn set-focus-source    set focus source kind (writes config.toml; restart required)
attn export [--since=YYYY-MM-DD] [--until=YYYY-MM-DD]
                         dump raw intervals as CSV on stdout
```

Every subcommand except `daemon`, `init`, `export` and `doctor` goes through the socket, so the CLI hides socket protocol details from Quickshell and shell scripts. Those four work without a running daemon: `init` writes the bundled default config, `export` and `doctor` open the state DB themselves.

### State database

SQLite at `~/.local/state/attn/attn.sqlite` (overridable in config). WAL journal mode, `foreign_keys = ON`. Raw intervals are source-of-truth; daily totals and the meta table are derived / mutable state.

```sql
CREATE TABLE app_intervals (
  id            INTEGER PRIMARY KEY,
  started_at    TEXT NOT NULL,
  ended_at      TEXT,
  window_id     INTEGER,
  app_id        TEXT NOT NULL,
  window_title  TEXT NOT NULL DEFAULT '',
  idle_adjusted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE domain_intervals (
  id              INTEGER PRIMARY KEY,
  started_at      TEXT NOT NULL,
  ended_at        TEXT NOT NULL,
  browser_name    TEXT NOT NULL DEFAULT '',
  browser_app_id  TEXT NOT NULL,
  domain          TEXT NOT NULL,
  url             TEXT NOT NULL DEFAULT '',
  source_profile  TEXT NOT NULL DEFAULT ''
);

CREATE TABLE media_intervals (
  id           INTEGER PRIMARY KEY,
  started_at   TEXT NOT NULL,
  ended_at     TEXT,
  playback_id  TEXT NOT NULL,   -- MPRIS bus name
  player_name  TEXT NOT NULL,
  title        TEXT NOT NULL DEFAULT '',
  artist       TEXT NOT NULL DEFAULT '',
  url          TEXT NOT NULL DEFAULT '',
  domain       TEXT NOT NULL DEFAULT '',
  source       TEXT NOT NULL DEFAULT 'mpris'
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

CREATE INDEX app_intervals_started_idx     ON app_intervals(started_at);
CREATE INDEX app_intervals_app_started_idx ON app_intervals(app_id, started_at);
CREATE INDEX app_intervals_open_idx        ON app_intervals(ended_at, id);
CREATE INDEX domain_intervals_started_idx  ON domain_intervals(started_at);
CREATE INDEX domain_intervals_browser_coverage_idx
  ON domain_intervals(browser_app_id, started_at, ended_at);
CREATE INDEX domain_intervals_import_idx
  ON domain_intervals(browser_name, source_profile, started_at, ended_at);
CREATE INDEX domain_intervals_legacy_import_idx
  ON domain_intervals(browser_app_id, source_profile, started_at, ended_at)
  WHERE browser_name = '';
CREATE INDEX media_intervals_started_idx   ON media_intervals(started_at);
CREATE INDEX media_intervals_open_idx      ON media_intervals(playback_id, ended_at);
CREATE INDEX media_intervals_ended_idx     ON media_intervals(ended_at, id);
```

There is deliberately no `daily_media_totals`. Media is low-volume, so status aggregates it straight from `media_intervals` on every read.

The `meta` table holds:

| key | meaning |
|-----|---------|
| `last_heartbeat_at` | 30 s liveness stamp; drives retroactive interval capping on restart |
| `paused_at` / `paused_reason` | current pause state (`manual` \| `idle`) |
| `session_reset_at` | hard boundary for the active-session walk |
| `breaks_enabled` / `breaks_interval_secs` / `breaks_min_break_secs` | `set_breaks` overrides, layered over the TOML at load |
| `notified_break_overdue` | start of the last session notified about, so one notification per session |
| `notified_budget:<day>:<category>` | one budget notification per category per local day |

Schema is migrated on startup: tables and indexes are `CREATE ... IF NOT EXISTS`, and missing columns (`app_intervals.window_id`, `domain_intervals.browser_name`) are added in place. Existing data is preserved.

### Quickshell widget

Four QML files:

- `AttnStatus.qml`: the widget's only data source and its only command runner. It polls `attn status --json`: initial polls run every 800 ms (up to three) so the chip appears quickly on startup, then settle to 5 s. It parses the status blob into properties, with defaults for the fields the daemon omits. Every command (`break-start`, `break-end`, `set-breaks`, `set-budget`, `set-notifications`, `set-focus-source`, `categorize`) goes through one queue, so commands run one at a time in click order and a fast second click is not dropped. Each command triggers a refresh when it exits.
- `AttnIndicator.qml`: bar chip. It owns an `AttnStatus` unless the host passes one. Dims while stale, brightens when fresh. The glyph turns amber and pulses when `break_overdue` or `paused`.
- `AttnPopup.qml`: popup rendered when the chip is clicked. It reads the same `AttnStatus` as the chip (`status: chip.status`), so a host wires one property instead of mirroring each field. Two view modes:
  - **Today**: stats card (total, stacked category bar with per-segment budget stripes, legend with `used / budget` labels), a focus-metrics line (`N switches · longest … · N deep blocks`) and an active-window line (`active HH:MM–HH:MM`), top-item pill row, category filter chips, side-by-side Apps / Domains lists, and an **Other** drawer.
  - **Week**: 7 day rows (today + previous 6), each with date label, total duration, stacked horizontal category bar, and per-day category legend.
- `AttnRow.qml`: single row used inside the Today lists (icon, label, mini bar, duration).

Beyond the two view modes:

- The **Other** drawer collapses `uncategorized_apps` / `uncategorized_domains`. Each row has a `+` button that opens a category picker and shells out to `attn categorize`, so a stray app ID can be filed without opening an editor.
- The **settings sheet** (gear icon → overlay) has four sections: break reminder (on/off, prompt-after 30 m / 1 h / 90 m / 2 h / 3 h, idle-is-a-break 2 m / 5 m / 10 m / 15 m → `attn set-breaks`), per-category budget presets (off / 15 m / 30 m / 1 h / 2 h / 4 h / 8 h → `attn set-budget`), notification toggles (enable / break-overdue / budget-exceeded → `attn set-notifications`), and focus source (`auto`/`niri`/`hyprland`/`river`/`sway` → `attn set-focus-source`, with a restart hint). Changes apply on click.
- An inline overdue banner appears at the top when `break_overdue && !paused`, leaving the data visible.
- A full break overlay appears only when actually paused (manual or idle).

The widget is pure presentation. It does not read SQLite, does not classify watch categories, does not run attribution logic. It receives a self-describing JSON blob and renders it. It talks to the daemon only through the CLI, never the socket, so the socket protocol stays private. One `attn status --json` costs about 10 ms of wall time and no measurable CPU, so polling through the CLI is cheap enough. It does not yet render the `media` array — media time is exposed in the status JSON but has no dedicated surface in the shipped popup.

## Focus tracking

### Layer 1: compositor focus events

The daemon dispatches to one of four `FocusSource` adapters based on `focus_source.kind`:

- **niri**: subscribes to `niri msg -j event-stream` and re-queries `niri msg -j focused-window` on each focus event.
- **Hyprland**: reads `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock` for `activewindow` / `activewindowv2` events and re-queries via `hyprctl -j activewindow`.
- **wlr-foreign-toplevel** (Sway, river): binds `zwlr_foreign_toplevel_manager_v1` and tracks the `activated` state per toplevel via `wayland-client`. It reports a focus change on each toplevel's `done` event, when the batch of `app_id`, title and state is complete. The protocol has no one-shot query, so the adapter keeps the last focus it reported and `poll_current` returns that.

`kind = "auto"` probes the environment in priority order `NIRI_SOCKET` → `HYPRLAND_INSTANCE_SIGNATURE` → `SWAYSOCK` → river. The resolved kind is fixed once at daemon startup; changing `focus_source.kind` needs a restart, which is why `set_focus_source` says so in its response. Note that `status.focus_source_kind` reports the *configured* value, so it can read `"auto"`.

Each adapter normalizes to a `FocusedWindow` struct with `app_id`, `title`, `pid`, and an optional `window_id`.

### Layer 2: terminal subprocess resolution

If the focused window is a terminal emulator (matched against `apps.watch.terminal`), the daemon attempts to identify the active program running inside it. Four steps, in order:

1. **Title match**. The compositor-reported window title is scanned for any program name configured under `[terminals.apps]`. Bounded-word matching is used so `clauderyx` does not match `claude`. A title containing a Claude Code spinner glyph resolves immediately. Titles beginning `tmux ` are skipped so the tmux query can answer instead. Modern terminals set the OSC 0/2 title to the foreground program's argv0; this path is fast and high-confidence, and its result is cached against the window id.
2. **Per-window cache**. A 256-entry FIFO map from window id → last resolved program. It answers the ambiguous-title case (e.g. the title flipped to a file path). An entry holds only while that program still runs under the terminal's pid. When the program exits, the entry is dropped. Session-scoped, never persisted.
3. **Tmux query**. If a tmux client process is found in the focused window's descendant tree, `tmux list-clients` is invoked. The matching client's `pane_current_command` is checked against `[terminals.apps]`. Without tmux running, this path is skipped. Each tmux call has a 1 s timeout.
4. **Descendant scan**. Walk `/proc/<pid>/task/*/children` recursively from the focused window's pid, collecting basenames and start-times. The most recently started subprocess matching `[terminals.apps]` wins.

One terminal process can serve several windows (Ghostty, kitty `--single-instance`). Its descendants then belong to all of its windows, so they cannot name the focused one. When the terminal pid has more than one child, steps 3 and 4 are skipped, except the tmux query for a title that starts with `tmux `. The tmux client with the most recent activity then wins.

The resolver runs before the daemon takes the pause or DB lock, because steps 3 and 4 can run subprocesses.

When any step matches, the `app_id` of the resulting interval is replaced with the resolved subprocess name (e.g. `claude`, `codex`, `nvim`). If none match, the original terminal `app_id` is kept.

### Idle handling

The compositor's idle signal decides. The interval cap is the fallback for when there is none.

- **Compositor idle** (Wayland `ext-idle-notify-v1`, `breaks.min_break_secs`): real input idle (no mouse/keyboard). At notifier version 2 the daemon uses `get_input_idle_notification`, which ignores idle inhibitors. The plain notification waits for every inhibitor: a video tab, a game, an SDL app, or a D-Bus `org.freedesktop.ScreenSaver.Inhibit` that an app never released. On one desktop, an inhibitor held it back for 6 days, and unattended nights counted as 10 h of focus. `attn doctor` warns on a compositor with version 1 only. `idled` fires `min_break_secs` after the last input. Tracking then auto-pauses (`paused_reason = idle`), and focus ends at the last input, not at the event: the open interval closes at `now - min_break_secs`, and any interval that ended later is trimmed back to that time. A screen locker that starts at the same timeout takes focus first and closes the interval late, so the trim matters. On `resumed`, the pause clears automatically and a fresh interval opens for the current focus. Only a `manual` pause is restored from `meta` at startup. An idle pause is dropped at startup and again on each new subscription, because a fresh notification never emits `resumed` without its own `idled` first. If the user is still away, the next `idled` trims the time since.
- **Interval cap** (`idle_after_secs`, default 300 s): applies only while no idle notification is live (compositor without `ext-idle-notify`, or the Wayland connection is down). The open interval then ends at most `idle_after_secs` after its start, both when it closes and in live totals. Closed rows keep their stored end. With a live notification the cap is off: 40 minutes of reading in one window is 40 minutes. Rows closed at less than their wall-clock duration are marked `idle_adjusted = 1` so reports can distinguish observed focus from capped focus.
- **Video without input**: a film needs no input, so the input-only `idled` alone would cut it at `min_break_secs`. If a media interval is open when `idled` fires, the daemon does not pause. It stores that interval's id. The media loop (2 s) checks the id. The pause starts when that interval closes, and focus ends at the interval's end. A media interval closes when playback stops or pauses, when the focus leaves the video, and when autoplay starts the next title. So, without input, only the video that plays at the last input counts. A single video counts for at most 3 h after the last input. `resumed` cancels the wait. The pause waits only for media that the daemon tracks: a focused browser on a `video` domain.

## Pause / break state machine

Tracking has exactly one pause state, held in memory as `Option<PauseInfo>` and mirrored into `meta.paused_at` / `meta.paused_reason`. `PauseReason` is `Manual` or `Idle`.

| Transition | Trigger | Effect |
|---|---|---|
| → paused (`manual`) | `break_start` socket command / `attn break-start` / popup button | close the open interval, persist pause |
| → paused (`idle`) | Wayland `idled` | end focus at the last input, persist pause |
| → paused (`idle`) | the media interval open at `idled` closes, or 3 h pass | end focus at that interval's end (at most the last input + 3 h), persist pause |
| → running | `break_end` / `attn break-end` | clear pause, stamp `session_reset_at = now`, reopen an interval for current focus |
| → running | Wayland `resumed`, either reason | same as above. `resumed` follows at least `min_break_secs` without input, so a manual "take a break" pause also ends once the user is back |
| → running | new idle subscription, reason `idle` only | same as above |

Focus events and the terminal poll never end a pause. Both also fire while the user is away: a terminal title that keeps changing, a window that closes itself.

While paused, focus changes close any open interval and open nothing. `active_session_seconds` reports 0 and `break_overdue` is false.

## Browser readers

Two reader kinds, selected per browser by `kind`:

- **`chromium`** — Helium, Brave, Chrome, Chromium. Reads `urls` JOIN `visits` from the `History` DB; timestamps are microseconds since 1601-01-01 UTC.
- **`firefox`** — Firefox, Zen, LibreWolf, Floorp. Reads `moz_historyvisits` JOIN `moz_places` from `places.sqlite`; `visit_date` is PRTime (microseconds since the Unix epoch). Firefox records no visit duration, so every visit arrives as a zero-length point and is extended by the shared normalizer.

Each browser is configured in `[browsers.<name>]` with `app_ids`, `history_paths` (glob) and `kind`.

Browser history is **never read live** from the user's profile. The pipeline is:

1. Discover History DBs via glob expansion.
2. Snapshot-copy the SQLite file (and `-journal` / `-wal` / `-shm` sidecars) to `$XDG_RUNTIME_DIR`, chmod 0600. Chromium uses a rollback journal, so a copy taken during a commit needs the journal to roll back. The runtime dir is a tmpfs: a copy on each poll does not write to a disk.
3. Query the snapshot.
4. Convert the browser's native timestamps to UTC.
5. Normalize visit ranges: sort by start, extend zero-length visits to the next visit's start (or the import window end), and clip any visit that overruns the next one. Bounds long-duration outliers from tab pinning.
6. Delete the snapshot.
7. Normalize again over the visits of all profiles of the browser. Without this step, one profile's last visit runs to the window end and takes the time of the other profile's later visits.

This avoids any lock contention with the running browser. On daemon startup, any stale `attn-*` snapshot files in `$XDG_RUNTIME_DIR` (else `$TMPDIR`) owned by the current uid and older than 1 hour are cleaned up.

The import runs every `poll_interval_secs` for today. After midnight, the first poll also imports the previous day once, because that day's last poll ran up to `poll_interval_secs` early.

### Domain clipping

For each browser visit, the daemon finds overlapping focused intervals for that browser's app and stores only the overlap as a `domain_interval`. Domain time can therefore never grow while the browser is unfocused.

Two refinements sit on top of the plain overlap:

- **Video title override**: if the focused browser window's title identifies a known video domain (YouTube, Twitch, Netflix, …) and that domain is in a `video` watch category, the visit is re-attributed to the title's domain. History alone cannot tell a 2-hour video apart from a 2-second navigation away and back.
- **Idle-cap exemption for video**: a focused interval covering a `video` domain is not clamped to `idle_after_secs`. Watching a film is genuinely low-input; capping it at 5 minutes would be wrong.

Before the daemon inserts an overlap, it subtracts the domain rows already stored for the same `browser_app_id`. So two visits never claim the same focused second, and the first visit in start order keeps the time. (See `insert_uncovered_domain_segments` in `src/main.rs`.)

On import, the imported rows for the day and profile are first deleted, then rewritten inside one `BEGIN IMMEDIATE` transaction. This keeps the table consistent even when browser history is amended or reordered between polls. The delete starts at the profile's earliest visit that is still in the history. A profile with no visits for the day keeps its rows. Thus a cleared or recreated history cannot erase time that the daemon already attributed.

All overlap queries on the interval tables have a lower `started_at` bound one day before the window, so they use the `started_at` indexes. Without the bound, each visit scanned the whole ledger: on a 9-day ledger that was 16.8 s of each 60 s poll, under the DB lock. An interval longer than one day would drop out of these queries. The longest one seen so far is 12 h.

## Media (MPRIS) tracking

Browser history says which page was open. It cannot say whether a video was actually playing. The MPRIS loop closes that gap.

Every 2 s the daemon lists session-bus names matching `org.mpris.MediaPlayer2.*`, reads `PlaybackStatus` and `Metadata` from each, and discards anything not `Playing`. Surviving snapshots carry `playback_id` (the bus name), `player_name`, `title`, `artist`, `url` and the `xesam:url` host as `domain`.

A snapshot is only recorded if **all** of the following hold:

1. There is an open `app_interval` — i.e. something has focus.
2. The player name maps to the focused window's browser (matched against that browser's config name and `app_ids`).
3. The focused window's title resolves to a known video domain.
4. That domain is in a `video` watch category, and the snapshot's own domain matches it.

If the player reports no `xesam:url` (common for browser players), the domain is inferred from the focused browser window title under the same conditions. The net effect: `media_intervals` only ever contains video actually playing in the browser you are actually looking at. Music players, background audio, and video in an unfocused window are all dropped.

Rows are opened when a player starts and closed when it stops, when its metadata changes (new track → new row), when the player disappears from the bus, and on daemon startup/shutdown. A session-bus failure closes all open media rows and retries after 10 s.

Status aggregates the day's `media_intervals` by domain (or by player + label when there is no domain), drops anything whose category cannot be resolved, and reports `media_seconds` plus `video_watch_seconds` (the `video`-category subset). Media seconds are **not** added to `tracked_seconds` or `watch_seconds` — they are a parallel measurement of the same wall clock, not extra time.

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

An `apps.watch` entry with a `*` is a glob pattern. Patterns are tried last, after both exact lookups, longest pattern first. The `games` defaults use them because game windows have no fixed app id: Proton games get `steam_app_<id>`, Wine windows get `<name>.exe`, and Minecraft puts its version in the window class.

Domain matching supports exact match and suffix match (`www.youtube.com` matches `youtube.com`), but never substring (`notyoutube.com` does not match `youtube.com`). Candidates are tried longest-first, so the most specific host wins (`drive.google.com` resolves to `storage`, not to `search` via `google.com`). App IDs and domains are normalized (lowercased, trimmed, leading `www.` stripped) on both sides of every comparison.

The shipped defaults cover 27 distinct category names: `ai`, `chat`, `coding`, `design`, `devops`, `editor`, `email`, `finance`, `food`, `games`, `health`, `learning`, `media`, `meeting`, `music`, `news`, `productivity`, `read_later`, `reference`, `scroll`, `search`, `shopping`, `sports`, `storage`, `terminal`, `travel`, `video`. Each has a distinct hue in the popup palette; anything else falls back to the neutral accent.

### Where the defaults live

`config/default.toml` is the single source of truth for the bundled watch lists and browser definitions. It is generated, never hand-edited:

- `config/runtime.toml` — non-list config (paths, intervals, display, browsers, terminals, breaks, focus source, notifications)
- `config/apps/<category>.txt` — one app ID per line per category (`#` comments allowed)
- `config/domains/<category>.txt` — one domain per line per category
- `config/default.toml` — generated by `tools/sync-default-config.sh` from the above

The Rust daemon embeds `config/default.toml` with `include_str!`, and the Nix flake reads the same file with `builtins.readFile`. There are no watch lists hardcoded in Rust: every serde field default for a bundled table (`apps.watch`, `domains.watch`, `browsers`) is resolved by looking that path up in the embedded document. CI runs `tools/sync-default-config.sh --check` to ensure `default.toml` stays in sync with the per-category sources.

### Config layering

`Config::load_from_path` starts from the bundled default and applies the user's `~/.config/attn/config.toml` on top:

- scalars and whole tables (`poll_interval_secs`, `idle_after_secs`, paths, `[display]`, `[budgets]`, `[breaks]`, `[focus_source]`, `[notifications]`, `terminals.poll_secs`) **replace** the default;
- watch lists (`apps.watch`, `domains.watch`, `terminals.apps`) **merge per category**, appending user entries the default does not already have;
- `[browsers.<name>]` entries replace the bundled browser of the same name and add new ones.

So a user config only needs to state its differences. `attn init --merge` performs the same merge and writes the result back out, which is how an existing config picks up newly-shipped categories.

### Filing uncategorized items

Anything focused but unmatched still shows up: `uncategorized_apps` (unwatched, non-browser, ≥ 60 s) and `uncategorized_domains` (unwatched, ≥ `display.domains_min_seconds`) ride along in the status JSON. Each list is capped at 25 entries with the remainder folded into a synthetic `(below threshold)` row so the totals stay honest. The popup's Other drawer renders these, and its `+` button calls the `categorize` command, which appends the ID to the chosen category in `config.toml` and reloads.

## Daily budgets

```toml
[budgets.scroll]
daily_budget_secs = 1800
```

A budget is per category per local day, measured against the same category totals the headline uses (watched app seconds excluding browsers, plus watched domain seconds). Budgeted categories always appear in `status.categories`, even at zero seconds, so the settings sheet can list them. Each category carries `budget_secs` and `over_budget`.

`set_budget` with `--secs=0` removes the entry. Note the two thresholds are deliberately not identical: the popup marks a category `over_budget` at `seconds >= budget`, while the notification only fires at `seconds > budget`, so the bar turns amber a moment before your desktop says anything.

## Break reminder

When the daemon's `[breaks]` config is enabled, status includes `active_session_seconds` and `break_overdue` (`active_session_seconds >= interval_secs && !paused`). The widget shows a subtle banner inviting a break. Its button, the "break in …" chip (any time), or `attn break-start` sets `paused_reason = manual`, closes the open interval, and stops opening new ones. The pause ends on `attn break-end` or on the next `resumed`, which means the user was away for at least `min_break_secs` and came back. Either way the daemon persists `session_reset_at = now` so the next session counter starts at zero, and reopens an interval for the current focus.

Auto-pause from Wayland idle uses the same state but with `paused_reason = idle` and auto-clears on the next `resumed` event.

`attn set-breaks --enabled --interval --min-break` (or the in-popup settings sheet) writes overrides into `meta`, which the daemon layers on top of the TOML config at load time. Values are clamped (`interval_secs >= 60`, `min_break_secs >= 30`). This is the one settings path that does **not** touch `config.toml`.

### Active session computation

`active_session_seconds` walks the newest 1000 `app_intervals` newest-first and accumulates duration until it hits a gap `>= min_break_secs` or crosses `meta.session_reset_at`. Closed intervals count with their stored end; the open interval counts up to now, so the live counter keeps moving. A gap in the ledger is real time away: `idled` ends focus at the last input, and suspend closes the interval. Without idle detection the cap from [Idle handling](#idle-handling) makes the gap instead. `session_reset_at` is set on `break_end` and on Wayland `resumed`. It is a hard boundary the walk cannot cross. The walk also returns the session's start, which the break notification uses as its once-per-session key.

## Focus metrics

Four numbers describing the shape of the day, derived from the existing ledger and recomputed on every status build (today only, never stored):

- `switch_count` — app intervals that *started* inside the local day. A proxy for context-switch churn.
- `longest_focus_secs` — the longest uninterrupted non-distracted stretch.
- `deep_work_blocks` — how many of those stretches reached 25 minutes.
- `active_since` / `active_until` — the day's work window: earliest interval start and latest interval end, clipped to the day.

Longest-focus and deep-work blocks come from sampling the activity envelope in fixed 60-second buckets between `active_since` and `active_until`. A bucket counts as deep when focus covers it and no distraction does, where a distraction is an app or domain in the `scroll` or `video` category (browser app rows themselves are excluded, so the browser's own focus doesn't count against you — the domain inside it does). A non-deep stretch shorter than `breaks.min_break_secs` does not end the current block, so a quick glance at a chat window doesn't reset the counter.

The 25-minute threshold and 60-second bucket are constants, not config. Precision below a minute is immaterial against a 25-minute threshold.

## Notifications

```toml
[notifications]
enabled         = true
break_overdue   = true
budget_exceeded = true
```

A 60-second thread evaluates two conditions, using a `NotificationSink` trait so tests can record instead of emitting. The desktop sink posts through `org.freedesktop.Notifications` via `notify-rust`.

- **Break overdue** (`Normal` urgency): fires when breaks are enabled, tracking is not paused, and `active_session_seconds >= interval_secs`. Deduped on the session's start (from the active-session walk) written to `meta.notified_break_overdue`, so you get exactly one notification per work session, not one per minute. A session that ends by a gap, such as a suspend, counts as a new one.
- **Budget exceeded** (`Low` urgency): fires for each category whose day total exceeds its `daily_budget_secs`. Deduped on `meta.notified_budget:<day>:<category>`, so once per category per local day.

The sink is chosen once at daemon startup from `notifications.enabled`. Turning notifications *off* takes effect immediately (the check re-reads live config and returns early); turning them *on* from a daemon that started with them off needs a restart. A missing notification daemon degrades to a logged error — the chip and banner still surface the state.

Nothing else in the daemon ever initiates an interaction.

## Daily totals & rebuild cooldown

`daily_app_totals` and `daily_domain_totals` are derived from the interval tables. They are deleted and rebuilt on demand inside a single SQLite transaction, covering both today and yesterday so the cross-midnight slice of the still-open interval lands in yesterday's total. A day runs from local midnight to the next local midnight, so a day with a clock change lasts 23 or 25 hours.

A 3-second cooldown gates the rebuild. After the first rebuild, subsequent `status` requests within 3 s reuse the cached totals; a request older than 3 s triggers a fresh rebuild. With the indicator polling every 5 s, this means at most one rebuild per poll. Steady-state status responses are ~1 ms.

The status response also includes a 7-day summary (`days[]`) whose app and domain figures come from `daily_*_totals`, so the Week view never re-scans app or domain intervals. Two things in a status response are always computed live rather than read from a totals table: media aggregates (per day, from `media_intervals`) and today's focus metrics (from `app_intervals` + `domain_intervals`).

`load_domain_status` also applies `[display]`: domains below `domains_min_seconds` (default 30) are dropped entirely, and beyond `domains_show_top` (default 12) only watched domains survive. This keeps a long tail of one-visit domains out of the popup without hiding anything you asked to track.

## Socket API

Unix socket at `$XDG_RUNTIME_DIR/attn.sock`, chmod 0600 inside the already user-private runtime directory.

Requests are newline-terminated command strings. Responses are JSON, newline-terminated. Read and write both carry a timeout, so a wedged client cannot hold a handler thread forever.

### `status`

```json
{
  "date": "2026-05-14",
  "updated_at": "2026-05-14T10:00:00+02:00",
  "watch_seconds": 15563,
  "tracked_seconds": 16928,
  "media_seconds": 1260,
  "video_watch_seconds": 1260,
  "apps":      [ { "id": "...", "seconds": 0, "watched": true, "category": "..." } ],
  "domains":   [ { "domain": "...", "seconds": 0, "watched": true, "category": "..." } ],
  "media":     [ { "player_name": "brave", "label": "youtube.com", "seconds": 1260,
                   "domain": "youtube.com", "watched": true, "category": "video" } ],
  "categories":[ { "name": "ai", "seconds": 6979, "budget_secs": 7200 },
                 { "name": "scroll", "seconds": 2100, "budget_secs": 1800, "over_budget": true } ],
  "days": [
    { "date": "2026-05-14", "tracked_seconds": 0, "watch_seconds": 0,
      "media_seconds": 0, "video_watch_seconds": 0,
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
  "breaks_min_break_secs": 300,
  "uncategorized_apps":    [ { "id": "...", "seconds": 0, "watched": false, "category": null } ],
  "uncategorized_domains": [ { "domain": "...", "seconds": 0, "watched": false, "category": null } ],
  "notifications_enabled": true,
  "notifications_break_overdue": true,
  "notifications_budget_exceeded": true,
  "focus_source_kind": "auto",
  "switch_count": 84,
  "longest_focus_secs": 4320,
  "deep_work_blocks": 3,
  "active_since": "2026-05-14T07:12:00+00:00",
  "active_until": "2026-05-14T10:00:00+00:00"
}
```

- `watch_seconds` = `(watched non-browser app seconds) + (watched domain seconds)`. The non-browser exclusion prevents double-counting browser focus when both the browser itself and its domains are in watch lists.
- `tracked_seconds` is the sum of all category seconds.
- `media_seconds` / `video_watch_seconds` are a parallel view of the same wall clock and are excluded from both totals.
- `apps` and `domains` lists include all today's items, watched and unwatched. The widget filters.
- `media`, `uncategorized_apps`, `uncategorized_domains` and the nullable pause / budget fields are omitted when empty, so consumers must default them.
- `days[]` entries carry no budget fields; budgets are a today-only concern.

### `reload`

Reload `~/.config/attn/config.toml` without restarting. Re-resolves state path and socket path; reopens the SQLite connection if the state path changed. Responds with `{ "ok": true, "state_reopened": bool, "socket_restart_required": bool }`. Also fires automatically when the config file's mtime changes, and after every config-writing command below.

### `break_start` / `break_end`

Toggle manual pause. Both return `{ "ok": true, "paused": bool, "paused_reason": "manual"|null }`.

### `set_breaks <enabled> <interval_secs> <min_break_secs>`

Persist break-reminder overrides to `meta` and apply in-memory. Returns the new values. The Quickshell settings sheet is the primary consumer; `attn set-breaks` is a CLI wrapper.

### Config-writing commands

`categorize`, `set_budget`, `set_notifications` and `set_focus_source` each take a single JSON argument on the command line:

```
categorize        {"kind":"app"|"domain","id":"<id>","category":"<name>"}
set_budget        {"category":"<name>","daily_budget_secs":<N>}         # 0 removes
set_notifications {"enabled":<bool?>,"break_overdue":<bool?>,"budget_exceeded":<bool?>}
set_focus_source  {"kind":"auto"|"niri"|"hyprland"|"river"|"sway"}
```

All four share one write protocol, because they are editing a file the user also edits by hand:

1. Validate against the running config (`categorize` and `set_budget` reject unknown categories and list the valid ones; `set_focus_source` rejects unknown kinds).
2. Parse `config.toml` with `toml_edit`, so comments, ordering and formatting survive.
3. Apply the edit in the document tree.
4. Re-stat the file: if its mtime changed since step 2, abort with `config changed on disk, retry` rather than clobber a concurrent edit.
5. Write to a temp file in the same directory and `persist` it over the original — atomic rename, no torn config.
6. Trigger an in-process reload so the change is live immediately.

All four respond `{ "ok": true }` or `{ "ok": false, "error": "..." }`. `set_focus_source` adds `"message"` reminding you a daemon restart is needed for a new kind to take effect.

## CSV export

```sh
attn export                              # everything
attn export --since=2026-05-01 --until=2026-05-14
```

`attn export` opens the state DB directly — no daemon, no socket — and streams CSV to stdout:

```
kind,started_at,ended_at,duration_secs,id,category,detail
app,2026-05-14T07:12:00+00:00,2026-05-14T07:31:00+00:00,1140,claude,ai,"claude — attn"
domain,2026-05-14T08:02:00+00:00,2026-05-14T08:09:00+00:00,420,github.com,coding,https://github.com/...
```

`id` is the app ID or domain; `detail` is the window title or the visit URL. Fields are RFC-4180 quoted only when they need it. `duration_secs` for app rows is the stored duration. Only a still-open row gets the `idle_after_secs` cap, because the export runs without the daemon's idle signal. `--since` / `--until` filter on interval start; `--until` is inclusive of the whole named day. Media intervals are not exported.

Both timestamps are the stored UTC ISO-8601 values, unconverted — grouping into local days is the consumer's job.

## Avoiding double-counting

App time, domain time and media time are different views over the same attention. A 30-minute browser focus interval can appear as all three:

- app time: `brave = 30 min`
- domain time: `youtube.com = 20 min`, `github.com = 10 min`
- media time: `youtube.com = 18 min` (actually playing)

For display purposes:

```
watch_seconds = watched non-browser app seconds + watched domain seconds
```

The browser's app total is still surfaced in the apps list (so you can see "30 min in Brave"), but it is excluded from the `watch_seconds` headline number, and media seconds are reported on their own axis. This is opinionated; revisit if the headline should mean something else.

## Performance

The daemon is intended to run continuously at near-zero cost.

Hot paths:

- Focus-source event handling is event-driven (niri, Hyprland, wlr-toplevel all surface focus changes as events), not polled.
- Status responses are served from `daily_*_totals` and gated by the 3 s rebuild cooldown, typically ~1 ms after the first call.
- Focus metrics sample in 60-second buckets across at most one day, so the loop is bounded at 1440 iterations over in-memory spans.
- Browser history scanning runs on a timer (default 60 s), not per status request.
- Live browser DB connections are open only long enough to copy the file.
- Terminal subprocess resolution is usually a title match or a cache hit; only an ambiguous, uncached title costs a `/proc` walk plus one `tmux list-clients` per focus change or 10 s tick.
- The MPRIS poll is two D-Bus property reads per playing player every 2 s, and does nothing at all when no player is running.
- Wayland idle and D-Bus sleep listeners block on their respective event sources; they do not poll.

Steady-state memory is ~40 MB and CPU is ~0% between focus events.

## Privacy & safety

- No network access. No telemetry. No cloud sync.
- All state local. State DB, its WAL/SHM sidecars, the socket, and every browser snapshot are chmod 0600.
- The browser reader records the URL host (domain) and the visit URL. Full URLs are stored to allow attribution debugging, but the user-facing surfaces (status JSON, popup) only show domains. `attn export` does emit them, because that's the point of an export.
- MPRIS metadata (title, artist, URL) is stored for media intervals; the status surface reduces it to a domain or a short label.
- No browser credential or content scraping.
- Window titles are recorded for app intervals because terminal-subprocess resolution and video attribution depend on them; window titles are not surfaced in the status JSON.

## Failure modes

| Failure | Behavior |
|---------|----------|
| Compositor IPC unavailable while running | The adapter retries its event stream every 5 s and falls back to a one-shot poll in the meantime. Logged. |
| Compositor IPC unavailable at startup | The startup focus probe fails and the daemon exits non-zero; systemd (`Restart=on-failure`, `RestartSec=3`) brings it back until the compositor answers. |
| Browser profile missing or History/places.sqlite unreadable | Skipped for this poll cycle. Logged. App tracking continues. |
| Browser History schema differs from the configured `kind` layout | Skipped. Logged. |
| Unknown `browsers.<name>.kind` | That browser is skipped with a log line; others still import. |
| Session D-Bus unavailable / no MPRIS players | Media loop closes any open media rows, logs, retries after 10 s. Everything else unaffected. |
| D-Bus notification daemon absent | Break-overdue and budget-exceeded notifications log an error and continue; popup chip + banner still surface the state. `attn doctor` reports the missing daemon. |
| Config omits a table, or has a watch-list entry that matches nothing | Missing tables fall back to the bundled defaults; an entry that matches nothing simply never matches. Unknown keys are ignored. |
| Config is malformed TOML, or `poll_interval_secs` / `idle_after_secs` is out of range | On reload: the error is logged and the previously loaded config stays live. At startup: the daemon refuses to start. |
| `config.toml` edited by hand while a `set_*` command is in flight | Command aborts with `config changed on disk, retry`; the file is left as the user saved it. |
| Wayland `ext-idle-notify-v1` unavailable | Auto-pause-on-idle disabled, and the `idle_after_secs` cap stays on. Manual break-start works; a manual pause ends only on break-end. `attn doctor` reports it. |
| D-Bus `org.freedesktop.login1` unreachable | Suspend/wake detection falls back to the heartbeat thread's retroactive-cap path. Logged. |
| Daemon hard-killed mid-session | On restart, heartbeat staleness retroactively caps any still-open app interval and closes open media intervals. |
| System suspended | D-Bus signal closes interval cleanly; on resume, focus event reopens. If D-Bus missed it, heartbeat catches up. |
| Socket path already exists after unclean shutdown | Stale path is removed if it doesn't resolve to a live listener. |
| State DB init fails | Daemon exits. Along with an invalid config and a failed startup focus probe, one of the only three fatal errors — all of them at startup. |

`attn doctor` probes config (plus sanity-warns on absurd break settings), the state DB, the focus source, Wayland idle-notify, D-Bus `login1`, a D-Bus notification daemon, every configured browser's history DBs, and the socket — then prints a `verdict: ok` / `verdict: errors found` summary.

## Time windows

V1 reports the current local day plus the previous 6 days. All stored timestamps are UTC ISO-8601; local-day grouping happens at aggregation time using the user's local timezone. Rolling windows beyond the 7-day summary are not implemented — `attn export` is the escape hatch for arbitrary ranges.

## Extension points

Deliberately deferred:

- Per-tab active-URL tracking via a browser extension.
- KDE Plasma / GNOME Shell focus providers (niri / Hyprland / Sway / river ship today).
- Monthly aggregates and a `daily_media_totals` table, if media volume ever justifies one.
- A media surface in the popup for the `media` array the daemon already emits.
- A small local TUI.
- `config.d/*.toml` drop-ins.
- Configurable deep-work threshold and distraction category list (currently constants).

These should not complicate the V1 daemon.
