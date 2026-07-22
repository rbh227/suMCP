#!/usr/bin/env bash
# Regenerate the README hero screenshot from the committed demo fixture.
# Crop target: header band, facts strip, Needs review, Timeline, and the
# Struggle areas table, ending cleanly at its footnote (File stories is cut
# off below the fold on purpose). Window width 960 matches the report's
# 920px content column; if the report layout changes, re-open the PNG and
# retune the height in --window-size until the crop again ends right after
# the Struggle areas table with nothing sliced mid-section.
# Requires: cargo, Google Chrome. Usage: scripts/render_demo_report.sh
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HTML="$(mktemp -t sumcp-demo-XXXX).html"
PNG="$ROOT/docs/assets/report-screenshot.png"
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

cargo build --release --manifest-path "$ROOT/Cargo.toml"
"$ROOT/target/release/sumcp" --file "$ROOT/fixtures/demo/demo-session.jsonl" --html > "$HTML"
"$CHROME" --headless --disable-gpu --hide-scrollbars \
  --window-size=960,820 --screenshot="$PNG" "file://$HTML"
echo "wrote $PNG"
