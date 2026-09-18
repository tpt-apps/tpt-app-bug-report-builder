#!/usr/bin/env sh
# TPT Bug Report Builder - build pipeline (macOS/Linux).
#
# Free edition (default):  ./build.sh
# Pro desktop edition:     ./build.sh --pro
#   (adds trunk build --release --features pro + the native webview exe)
#
# Env:
#   TPT_WEB_REPO  path to the web repo; the bundle is copied to
#                 $TPT_WEB_REPO/public/apps/bug-report-builder/ (glue named
#                 bug-report-builder.js, wasm keeps its _bg.wasm name),
#                 otherwise dist/web in this repo.

set -eu
cd "$(dirname "$0")"

CRATE=tpt-app-bug-report-builder
TARGET=wasm32-unknown-unknown
OUT_NAME=bug-report-builder

PRO=0
for arg in "$@"; do
  case "$arg" in
    --pro) PRO=1 ;;
    *) echo "usage: build.sh [--pro]" >&2; exit 2 ;;
  esac
done

if [ -n "${TPT_WEB_REPO:-}" ]; then
  DEST="$TPT_WEB_REPO/public/apps/bug-report-builder"
else
  DEST="dist/web"
fi

echo "== TPT Bug Report Builder build =="
echo " edition: $([ "$PRO" = 1 ] && echo 'pro (multi-screenshot sequences + scrolling capture + Markdown/PDF + hotkeys)' || echo 'free (single screenshot)')"
echo " dest:    $DEST"

if [ "$PRO" = 1 ]; then
  cargo build --release --target "$TARGET" -p "$CRATE" --features pro
else
  cargo build --release --target "$TARGET" -p "$CRATE"
fi

WASM="target/$TARGET/release/tpt_app_bug_report_builder.wasm"
[ -f "$WASM" ] || { echo "missing $WASM" >&2; exit 1; }

mkdir -p "$DEST"
wasm-bindgen --target web --out-name "$OUT_NAME" --out-dir "$DEST" "$WASM"

GLUE="$DEST/$OUT_NAME.js"
BG_WASM="$DEST/${OUT_NAME}_bg.wasm"
[ -f "$GLUE" ] || { echo "wasm-bindgen did not produce $GLUE" >&2; exit 1; }
[ -f "$BG_WASM" ] || { echo "wasm-bindgen did not produce $BG_WASM" >&2; exit 1; }

# No hub-wide wasm size budget was found (see build.ps1's comment); 5 MB
# gzip here is a generous local sanity check only.
RAW=$(wc -c < "$BG_WASM" | tr -d ' ')
if command -v gzip >/dev/null 2>&1; then
  GZ=$(gzip -9 -c "$BG_WASM" | wc -c | tr -d ' ')
  echo "-- $(basename "$BG_WASM"): $RAW bytes raw, $GZ bytes gzipped"
  if [ "$GZ" -gt 5242880 ]; then
    echo "warning: payload exceeds the 5 MB local sanity check (not a known hub budget)" >&2
  fi
fi

if [ "$PRO" = 1 ]; then
  trunk build --release --features pro
  cargo build --release -p tpt-bugreport-desktop
  echo "   exe: target/release/tpt-bug-report-builder-pro (serves dist/)"
fi

echo "-- done"
[ -n "${TPT_WEB_REPO:-}" ] || echo "   (set TPT_WEB_REPO to copy the bundle into the hub web repo)"
