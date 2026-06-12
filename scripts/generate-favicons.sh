#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SVG="$ROOT/static/favicon.svg"
OUT="$ROOT/static"

if [[ ! -f "$SVG" ]]; then
  echo "missing $SVG" >&2
  exit 1
fi

render_png() {
  local size="$1"
  local dest="$2"
  if command -v sips >/dev/null 2>&1; then
    sips -s format png -z "$size" "$size" "$SVG" --out "$dest" >/dev/null
  else
    python3 "$ROOT/scripts/rasterize_favicon.py" "$SVG" "$dest" "$size"
  fi
}

render_png 16 "$OUT/favicon-16x16.png"
render_png 32 "$OUT/favicon-32x32.png"
render_png 180 "$OUT/apple-touch-icon.png"
python3 "$ROOT/scripts/rasterize_favicon.py" --ico "$OUT/favicon.ico" \
  "$OUT/favicon-16x16.png" "$OUT/favicon-32x32.png"

echo "Generated favicons in $OUT"
