#!/bin/sh
# attn installer
#
# Quick install:
#   curl -fsSL https://raw.githubusercontent.com/0xPD33/attn/main/scripts/install.sh | sh
#
# Optional environment overrides:
#   ATTN_REPO       owner/repo on GitHub. Default: 0xPD33/attn
#   ATTN_VERSION    tag to install (e.g. v0.1.0). Default: latest release.
#   ATTN_PREFIX     binary install dir. Default: $HOME/.local/bin
#   ATTN_SYSTEMD    set to 0 to skip writing the user service. Default: 1
#   ATTN_START      set to 1 to enable + start the service. Default: 0
#   ATTN_SKIP_COMPOSITOR_CHECK   set to 1 to install even without a supported compositor (not recommended)
#   ATTN_SKIP_NIRI_CHECK         deprecated alias for ATTN_SKIP_COMPOSITOR_CHECK

set -eu

ATTN_REPO="${ATTN_REPO:-0xPD33/attn}"
ATTN_PREFIX="${ATTN_PREFIX:-$HOME/.local/bin}"
ATTN_SYSTEMD="${ATTN_SYSTEMD:-1}"
ATTN_START="${ATTN_START:-0}"
ATTN_SKIP_COMPOSITOR_CHECK="${ATTN_SKIP_COMPOSITOR_CHECK:-0}"
ATTN_SKIP_NIRI_CHECK="${ATTN_SKIP_NIRI_CHECK:-0}"

red()    { printf '\033[31m%s\033[0m\n' "$1" >&2; }
green()  { printf '\033[32m%s\033[0m\n' "$1"; }
yellow() { printf '\033[33m%s\033[0m\n' "$1"; }
info()   { printf '%s\n' "$1"; }

need() {
  command -v "$1" >/dev/null 2>&1 || { red "missing required tool: $1"; exit 1; }
}

need curl
need tar
need uname

case "$(uname -s)" in
  Linux) ;;
  *)     red "attn currently only supports Linux."; exit 1 ;;
esac

if [ "$ATTN_SKIP_COMPOSITOR_CHECK" != "1" ] && [ "$ATTN_SKIP_NIRI_CHECK" != "1" ]; then
  if ! command -v niri >/dev/null 2>&1 \
     && [ -z "$HYPRLAND_INSTANCE_SIGNATURE" ] \
     && [ -z "$SWAYSOCK" ] \
     && ! command -v river >/dev/null 2>&1; then
    red "attn needs a supported Wayland compositor (niri, Hyprland, Sway, or river)."
    yellow "Focus tracking subscribes to the compositor's IPC. Without one, app tracking does nothing."
    yellow "Set ATTN_SKIP_COMPOSITOR_CHECK=1 to install anyway (e.g. on a build host)."
    exit 1
  fi
fi

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64|amd64)  TARGET="x86_64-unknown-linux-musl" ;;
  aarch64|arm64) TARGET="aarch64-unknown-linux-musl" ;;
  *) red "unsupported arch: $ARCH"; exit 1 ;;
esac

API="https://api.github.com/repos/${ATTN_REPO}"
RELEASES_BASE="https://github.com/${ATTN_REPO}/releases/download"

if [ -z "${ATTN_VERSION:-}" ]; then
  info "resolving latest release from ${ATTN_REPO} ..."
  TAG="$(curl -fsSL "${API}/releases/latest" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
  if [ -z "$TAG" ]; then
    red "could not determine latest release. Set ATTN_VERSION=v0.1.0 to install a specific tag."
    exit 1
  fi
else
  TAG="$ATTN_VERSION"
fi

ASSET="attn-${TAG}-${TARGET}.tar.gz"
SHA_ASSET="${ASSET}.sha256"
URL="${RELEASES_BASE}/${TAG}/${ASSET}"
SHA_URL="${RELEASES_BASE}/${TAG}/${SHA_ASSET}"

info "tag:    ${TAG}"
info "target: ${TARGET}"
info "asset:  ${URL}"

TMP="$(mktemp -d -t attn-install.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT INT HUP TERM

info "downloading..."
curl -fsSL "$URL"     -o "$TMP/$ASSET"
curl -fsSL "$SHA_URL" -o "$TMP/$SHA_ASSET" || yellow "no checksum file at $SHA_URL, skipping verification"

if [ -f "$TMP/$SHA_ASSET" ]; then
  info "verifying checksum..."
  (cd "$TMP" && sha256sum -c "$SHA_ASSET")
fi

info "extracting..."
tar -C "$TMP" -xzf "$TMP/$ASSET"
SRC_DIR="$TMP/attn-${TAG}-${TARGET}"
if [ ! -x "$SRC_DIR/attn" ]; then
  red "expected $SRC_DIR/attn after extract"; exit 1
fi

mkdir -p "$ATTN_PREFIX"
install -m 0755 "$SRC_DIR/attn" "$ATTN_PREFIX/attn"
green "installed: ${ATTN_PREFIX}/attn"

case ":$PATH:" in
  *":${ATTN_PREFIX}:"*) : ;;
  *) yellow "warning: ${ATTN_PREFIX} is not on \$PATH. Add it to your shell rc:"
     yellow "  export PATH=\"${ATTN_PREFIX}:\$PATH\"" ;;
esac

CONFIG_PATH="$HOME/.config/attn/config.toml"
if [ ! -f "$CONFIG_PATH" ]; then
  info "writing default config..."
  "$ATTN_PREFIX/attn" init || yellow "could not write default config; run 'attn init' manually."
else
  info "merging new bundled defaults into existing config..."
  "$ATTN_PREFIX/attn" init --merge || yellow "could not merge default config; run 'attn init --merge' manually."
fi

UNIT_DIR="$HOME/.config/systemd/user"
UNIT_PATH="$UNIT_DIR/attn.service"

if [ "$ATTN_SYSTEMD" = "1" ]; then
  mkdir -p "$UNIT_DIR"
  cat > "$UNIT_PATH" <<UNIT
[Unit]
Description=attn local attention ledger
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=${ATTN_PREFIX}/attn daemon
Restart=on-failure
RestartSec=3

[Install]
WantedBy=graphical-session.target
UNIT
  green "wrote systemd user unit: $UNIT_PATH"

  if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload >/dev/null 2>&1 || true
    if [ "$ATTN_START" = "1" ]; then
      systemctl --user enable --now attn.service \
        && green "service enabled and started" \
        || yellow "could not enable/start the service automatically."
    else
      info "to enable on next login + start now:"
      info "  systemctl --user enable --now attn.service"
    fi
  fi
fi

info ""
green "done."
info ""
info "next steps:"
info "  1. make sure your compositor (niri / Hyprland / Sway / river) is running."
info "  2. edit your config:  \$EDITOR $CONFIG_PATH"
info "  3. check status:      attn status --json | jq ."
info "  4. inspect health:    attn doctor"
info ""
info "Quickshell users: copy quickshell/Attn*.qml from the release tarball"
info "(or the repository) into your quickshell config tree."
