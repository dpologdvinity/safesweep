#!/usr/bin/env bash
# Builds the npm packages for a release, ready for `npm publish`.
#
# Usage:        npm/build.sh TAG OUT        (for example: npm/build.sh v0.6.1 /tmp/npm)
# Needs:        bash, curl, tar, sha256sum, jq; the GitHub release TAG must already have its
#               binaries attached (the release workflow does that first).
# Produces:     OUT/safesweep-linux-x64/    package holding the x86_64 binary
#               OUT/safesweep-linux-arm64/  package holding the aarch64 binary
#               OUT/safesweep/              the package users install: a small Node launcher
#                                           (bin/safesweep.js) plus optionalDependencies on the two above
#
# Why two kinds of package: npm installs only the optional dependency whose `os` and `cpu` fields
# match the machine, so each user downloads one binary. The launcher finds that binary and runs it.
#
# Safety: the binaries are not rebuilt. They are the ones already published on the GitHub release,
# and each tarball is checked against its published .sha256 file before it goes into a package.
set -euo pipefail
tag=${1:?usage: npm/build.sh TAG OUT} out=${2:?usage: npm/build.sh TAG OUT}
ver=${tag#v} # the npm version is the tag without the leading "v"
repo=dpologdvinity/safesweep
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
mkdir -p "$out"

# One package per CPU architecture (npm names: x64, arm64; Rust target triples differ).
for arch in x64 arm64; do
  triple=$([ $arch = x64 ] && echo x86_64 || echo aarch64)-unknown-linux-musl
  dir="$out/safesweep-linux-$arch"
  mkdir -p "$dir/bin"
  name="safesweep-$tag-$triple.tar.gz"
  base="https://github.com/$repo/releases/download/$tag"
  tmp=$(mktemp -d)
  curl -fsSL -o "$tmp/$name" "$base/$name"
  curl -fsSL -o "$tmp/$name.sha256" "$base/$name.sha256"
  # the published checksum must match the tarball we are about to ship inside an npm package
  (cd "$tmp" && sha256sum -c "$name.sha256")
  tar xzf "$tmp/$name" -C "$dir" --strip-components=1 "safesweep-$tag-$triple/safesweep"
  rm -rf "$tmp"
  mv "$dir/safesweep" "$dir/bin/safesweep"
  chmod 755 "$dir/bin/safesweep"
  # the binary this runner can execute must report the version being published
  if [ "$arch" = x64 ] && [ "$(uname -m)" = x86_64 ]; then
    [ "$("$dir/bin/safesweep" --version)" = "safesweep $ver" ] || { echo "binary version does not match $ver" >&2; exit 1; }
  fi
  # `os` and `cpu` make npm skip this package on other machines; `preferUnplugged` stops Yarn
  # Plug'n'Play from keeping the binary inside a zip, where it cannot be executed.
  jq -n --arg v "$ver" --arg a "$arch" --arg r "$repo" '{
    name: "safesweep-linux-\($a)", version: $v,
    description: "safesweep binary for Linux \($a); install the safesweep package instead",
    license: "MIT OR Apache-2.0", repository: { type: "git", url: "git+https://github.com/\($r).git" },
    os: ["linux"], cpu: [$a], files: ["bin/safesweep"], preferUnplugged: true
  }' > "$dir/package.json"
done

# The package users install: the launcher script plus pointers to the platform packages.
main="$out/safesweep"
mkdir -p "$main/bin"
cp "$here/safesweep/bin/safesweep.js" "$main/bin/"
cp "$root/README.md" "$root/LICENSE-MIT" "$root/LICENSE-APACHE" "$main/"
jq -n --arg v "$ver" --arg r "$repo" '{
  name: "safesweep", version: $v,
  description: "Clean caches and temp files that are provably safe to delete (Linux and WSL)",
  license: "MIT OR Apache-2.0", homepage: "https://github.com/\($r)",
  repository: { type: "git", url: "git+https://github.com/\($r).git" },
  keywords: ["cache", "cleanup", "disk", "wsl", "cli"],
  bin: { safesweep: "bin/safesweep.js" }, files: ["bin", "README.md", "LICENSE-MIT", "LICENSE-APACHE"],
  os: ["linux"], engines: { node: ">=16" },
  optionalDependencies: { "safesweep-linux-x64": $v, "safesweep-linux-arm64": $v }
}' > "$main/package.json"
echo "$out"
