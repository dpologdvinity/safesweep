#!/usr/bin/env bash
# Rebuilds demo/demo.gif (the animation at the top of the README).
#
# Usage:   cargo build --release && demo/make.sh
# Needs:   cc (to build the fake clock), uv (to install Pillow, a Python imaging library),
#          unshare (user namespaces), and the release binary in target/release.
#
# How it works: demo/env.sh builds a throwaway home directory with fake caches and a fake clock;
# the real binary runs three commands there and its real output is captured; demo/render.py then
# draws that output as terminal frames. The GIF is drawn rather than screen-recorded because
# recorders drop frames on a busy machine. Nothing here touches the real home directory.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$(mktemp -d)
bash -c "
  set -e
  source '$here/env.sh'
  safesweep scan > '$out/scan.txt'
  safesweep clean --apply > '$out/apply.txt'
  safesweep why --top 4 > '$out/why.txt'
  kill \$(jobs -p) 2>/dev/null || true
"
venv="${XDG_CACHE_HOME:-$HOME/.cache}/safesweep-demo-venv"  # user-owned, not a shared /tmp path
[ -d "$venv" ] || { uv venv -q "$venv" && uv pip install -q --python "$venv" pillow; }
"$venv/bin/python" "$here/render.py" "$here/demo.gif" \
  "safesweep scan" "$out/scan.txt" \
  "safesweep clean --apply" "$out/apply.txt" \
  "safesweep why --top 4" "$out/why.txt"
