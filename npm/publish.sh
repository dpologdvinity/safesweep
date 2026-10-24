#!/usr/bin/env bash
# Publishes the npm packages built by npm/build.sh, skipping any version that is already on npm.
#
# Usage:     npm/publish.sh TAG DIST_TAG OUT     (for example: npm/publish.sh v0.6.4 latest /tmp/npm)
#              TAG       the release tag, such as v0.6.4; the npm version is the tag without the "v"
#              DIST_TAG  the npm dist-tag: `latest`, or `next` for a prerelease
#              OUT       the directory npm/build.sh wrote the three packages into
# Needs:     npm 11.5.1 or newer, logged in or able to use trusted publishing (OIDC)
#
# The platform packages go first, so the launcher's optionalDependencies already exist when the
# launcher itself is published. Because published versions are skipped, running this again after a
# partial failure publishes only what is missing, which is what lets the workflow try trusted
# publishing first and fall back to a token for the rest.
set -euo pipefail
tag=${1:?usage: npm/publish.sh TAG DIST_TAG OUT}
dist=${2:?usage: npm/publish.sh TAG DIST_TAG OUT}
out=${3:?usage: npm/publish.sh TAG DIST_TAG OUT}
ver=${tag#v}

for p in safesweep-linux-x64 safesweep-linux-arm64 safesweep; do
  if npm view "$p@$ver" version >/dev/null 2>&1; then
    echo "$p@$ver already published"
    continue
  fi
  (cd "$out/$p" && npm publish --provenance --access public --tag "$dist")
done
