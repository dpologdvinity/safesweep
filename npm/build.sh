#!/usr/bin/env bash
# Builds the npm packages for a release into OUT: safesweep (launcher) plus one package per
# platform holding the static binary from the GitHub release. Usage: npm/build.sh v0.5.0 OUT
set -euo pipefail
tag=$1 out=$2 ver=${1#v}
repo=dpologdvinity/safesweep
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
mkdir -p "$out"

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
  jq -n --arg v "$ver" --arg a "$arch" --arg r "$repo" '{
    name: "safesweep-linux-\($a)", version: $v,
    description: "safesweep binary for Linux \($a); install the safesweep package instead",
    license: "MIT OR Apache-2.0", repository: { type: "git", url: "git+https://github.com/\($r).git" },
    os: ["linux"], cpu: [$a], files: ["bin/safesweep"], preferUnplugged: true
  }' > "$dir/package.json"
done

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
