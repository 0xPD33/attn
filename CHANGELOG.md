# Changelog

All notable changes to attn will be documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.3] - 2026-05-14

### Added
- `attn init --merge` layers an existing user config over the bundled defaults so newly shipped apps, domains, and categories land automatically on upgrade without clobbering user edits. `scripts/install.sh` now runs `attn init --merge` against existing installs.
- Today panel surfaces overdue break duration (e.g. "over by 12m") once the interval is passed.
- App/domain lists in the Today panel are scrollable inside the popup instead of resizing it.
- Default watch lists pick up LibreOffice apps (productivity), `vercel.app` (coding), and `search.brave.com` (search).

### Fixed
- Manual break-start while the daemon is already idle-paused now swaps the pause reason to `manual` so the popup label and break-end behave predictably.
- `active_session_seconds` keeps the live (still-open) focus interval ticking past `idle_after_secs` so long focused sessions get an accurate break countdown. Closed historical intervals still use the cap so they can't bridge across a real break.
- Today list rows no longer swallow mouse buttons, so the new scroll views handle flick gestures correctly.

## [0.1.2] - 2026-05-14

### Changed
- Watch lists split out of `config/default.toml` into per-category files under `config/apps/<category>.txt` and `config/domains/<category>.txt` (one item per line, `#` comments allowed). Non-list runtime config moved to `config/runtime.toml`. `tools/sync-default-config.sh` regenerates `config/default.toml` from those sources; CI verifies they stay in sync.

### Fixed
- Release workflow: matrix uploaders no longer race against each other looking for a release that doesn't exist yet. A `create-release` job now runs first; both `upload` and `install-script` depend on it.

## [0.1.1] - 2026-05-14

### Fixed
- `active_session_seconds` now treats a focused-but-idle window as a break. Previously, leaving the PC with the same window focused kept the session counter ticking because no focus-change event fired and the open interval never had a gap. The session walk now caps each interval at `idle_after_secs` from its start, so a long-running open interval creates an effective gap that the existing break detection picks up.

## [0.1.0] - 2026-05-14

Initial public release.

### Added
- Daemon and CLI written in Rust.
  - Focus tracking from the Niri IPC event stream.
  - Terminal subprocess resolution via window title match, live tmux query, and `/proc` descendant walk with start-time tiebreak.
  - Chromium-family browser history readers (Helium, Brave, Chrome, Chromium) with snapshot-copy + domain clipping to focused intervals.
  - Local SQLite ledger with raw app/domain intervals, rebuildable daily totals, and a small `meta` key/value table.
  - Unix socket API: `status`, `reload`, `break_start`, `break_end`, `set_breaks`.
  - CLI subcommands: `daemon`, `status --json`, `reload`, `init [--force]`, `doctor`, `break-start`, `break-end`, `set-breaks`.
- Default watch lists covering 22 categories (coding, ai, design, media, productivity, chat, email, storage, meeting, video, music, scroll, news, shopping, finance, learning, search, reference, devops, travel, food, sports, health, read_later).
- Break reminder.
  - `active_session_seconds` and `break_overdue` in `status`.
  - Wayland `ext-idle-notify-v1` auto-pause / auto-resume.
  - Manual `attn break-start` / `attn break-end` with persistent pause state.
- Suspend handling.
  - D-Bus `org.freedesktop.login1.Manager.PrepareForSleep` listener.
  - Heartbeat-based retroactive interval cap on ungraceful suspend / crash.
- Live config reload via filesystem mtime watch on `~/.config/attn/config.toml`.
- Quickshell widget.
  - `AttnIndicator.qml` bar chip with pulsing amber alert state.
  - `AttnPopup.qml` with Today / Week views, stacked category bar, top-item pill row, filter chips, two-column Apps / Domains lists.
  - In-popup settings sheet (gear icon) for break-reminder configuration.
  - Inline overdue banner; full overlay only when actually paused.
- Nix flake with `homeManagerModules.default` (`programs.attn`).
- One-line installer script with sha256 verification and systemd user unit generation.
- GitHub Releases workflow producing static musl binaries for `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`.
- `attn doctor` probes for niri, state DB, wayland idle-notify, D-Bus login1, browser DBs, and socket path; prints a final `verdict: ok` / `verdict: errors found`.

[Unreleased]: https://github.com/0xPD33/attn/compare/v0.1.3...HEAD
[0.1.3]: https://github.com/0xPD33/attn/releases/tag/v0.1.3
[0.1.2]: https://github.com/0xPD33/attn/releases/tag/v0.1.2
[0.1.1]: https://github.com/0xPD33/attn/releases/tag/v0.1.1
[0.1.0]: https://github.com/0xPD33/attn/releases/tag/v0.1.0
