#!/usr/bin/env bash
# Regenerate config/default.toml from config/runtime.toml plus the
# per-category watch-list files under config/apps/ and config/domains/.
#
# Contributors editing the shipped defaults edit a per-category file
# (e.g. config/domains/coding.txt) and run this script. CI verifies
# config/default.toml is in sync with the per-category files.
#
# Usage:
#   tools/sync-default-config.sh           regenerate config/default.toml
#   tools/sync-default-config.sh --check   exit non-zero if out of sync
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$REPO/config/default.toml"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

emit_list_section() {
  local section="$1"     # "apps.watch" or "domains.watch"
  local dir="$2"         # "config/apps" or "config/domains"
  local file
  printf '[%s]\n' "$section"
  for file in "$REPO/$dir"/*.txt; do
    [ -e "$file" ] || continue
    local category
    category="$(basename "$file" .txt)"
    printf '%s = [\n' "$category"
    # Skip blank lines and comments. Trim whitespace.
    while IFS= read -r line; do
      line="${line%%#*}"
      line="${line#"${line%%[![:space:]]*}"}"
      line="${line%"${line##*[![:space:]]}"}"
      [ -z "$line" ] && continue
      printf '  "%s",\n' "$line"
    done < "$file"
    printf ']\n'
  done
}

{
  # Non-watch-list runtime config (paths, intervals, browsers, terminals, breaks).
  cat "$REPO/config/runtime.toml"
  echo
  emit_list_section "apps.watch"    "config/apps"
  echo
  emit_list_section "domains.watch" "config/domains"
} > "$TMP"

if [ "${1:-}" = "--check" ]; then
  if ! diff -u "$OUT" "$TMP" > /dev/null; then
    echo "config/default.toml is out of sync with config/runtime.toml + config/apps/ + config/domains/" >&2
    echo "Run tools/sync-default-config.sh to regenerate it." >&2
    diff -u "$OUT" "$TMP" >&2 || true
    exit 1
  fi
  exit 0
fi

mv "$TMP" "$OUT"
trap - EXIT
echo "wrote $OUT"
