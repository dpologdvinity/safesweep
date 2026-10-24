#!/usr/bin/env bash
# Generates the shell completions and the man page by asking a built binary for them.
#
# Usage:     packaging/gen-extras.sh BINARY [OUT]       (OUT defaults to `target`)
# Produces:  OUT/extras/safesweep.bash  OUT/extras/_safesweep  OUT/extras/safesweep.fish
#            OUT/extras/safesweep.1     (bash, zsh and fish completions; the man page)
# Prints:    the directory it wrote to.
#
# These files are not stored in git: they are generated from the same command-line definition
# the program uses, so they cannot go out of date. The release workflow runs this before building
# the .deb and .rpm (whose asset lists in Cargo.toml point at OUT/extras) and before making the
# release tarballs.
set -euo pipefail
bin=${1:?usage: packaging/gen-extras.sh BINARY [OUT]} out=${2:-target}/extras
mkdir -p "$out"
"$bin" completions bash > "$out/safesweep.bash"
"$bin" completions zsh > "$out/_safesweep"
"$bin" completions fish > "$out/safesweep.fish"
"$bin" man > "$out/safesweep.1"
echo "$out"
