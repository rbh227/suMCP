#!/usr/bin/env bash
# Regenerate the README hero screenshot from the committed demo fixture.
# Requires: cargo, Google Chrome. Usage: scripts/render_demo_report.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HTML="$(mktemp -t sumcp-demo-XXXX).html"
PNG="$ROOT/docs/assets/report-screenshot.png"
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

cargo build --release --manifest-path "$ROOT/Cargo.toml"
"$ROOT/target/release/sumcp" --file "$ROOT/fixtures/demo/demo-session.jsonl" --html > "$HTML"
"$CHROME" --headless --disable-gpu --hide-scrollbars \
  --window-size=1100,1400 --screenshot="$PNG" "file://$HTML"
echo "wrote $PNG"
