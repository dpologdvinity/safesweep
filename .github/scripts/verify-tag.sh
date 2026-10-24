#!/usr/bin/env bash
# Refuses to go on unless TAG is a release tag that matches Cargo.toml in the checked-out tree.
# Usage: verify-tag.sh TAG   (run after checking out TAG). Prints "stable" or "prerelease".
set -euo pipefail
tag=${1:?usage: verify-tag.sh TAG}
semver='^v([0-9]+\.[0-9]+\.[0-9]+)(-[0-9A-Za-z.-]+)?$'
if [[ ! $tag =~ $semver ]]; then
  echo "::error::'$tag' is not a release tag (expected vMAJOR.MINOR.PATCH[-pre])" >&2
  exit 1
fi
# first `version = "..."` in Cargo.toml is the [package] one
ver=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n1)
if [ "v$ver" != "$tag" ]; then
  echo "::error::tag $tag does not match Cargo.toml version '$ver'" >&2
  exit 1
fi
[[ $tag == *-* ]] && echo prerelease || echo stable
