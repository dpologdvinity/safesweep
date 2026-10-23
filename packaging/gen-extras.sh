#!/usr/bin/env bash
# Generates shell completions and the man page from a built binary into OUT/extras, for the
# .deb/.rpm metadata in Cargo.toml and the release tarballs. Usage: gen-extras.sh BINARY [OUT]
set -euo pipefail
bin=$1 out=${2:-target}/extras
mkdir -p "$out"
"$bin" completions bash > "$out/safesweep.bash"
"$bin" completions zsh > "$out/_safesweep"
"$bin" completions fish > "$out/safesweep.fish"
"$bin" man > "$out/safesweep.1"
echo "$out"
