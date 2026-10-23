#!/usr/bin/env bash
# Rebuilds demo/demo.gif from real runs in a throwaway home. Needs: cargo build --release, cc, uv.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$(mktemp -d)
bash -c "
  source '$here/env.sh'
  safesweep scan > '$out/scan.txt'
  safesweep clean --apply > '$out/apply.txt'
  safesweep why --top 4 > '$out/why.txt'
  kill \$(jobs -p) 2>/dev/null || true
"
[ -d /tmp/demo-venv ] || { uv venv -q /tmp/demo-venv && uv pip install -q --python /tmp/demo-venv pillow; }
/tmp/demo-venv/bin/python "$here/render.py" "$here/demo.gif" \
  "safesweep scan" "$out/scan.txt" \
  "safesweep clean --apply" "$out/apply.txt" \
  "safesweep why --top 4" "$out/why.txt"
