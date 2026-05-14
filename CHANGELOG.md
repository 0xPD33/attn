# Changelog

All notable changes to attn will be documented here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/0xPD33/attn/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/0xPD33/attn/releases/tag/v0.1.0
