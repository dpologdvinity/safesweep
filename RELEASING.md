# Releasing

1. On a branch, bump `version` in `Cargo.toml`, run `cargo build` (updates `Cargo.lock`), and add the
   release to `CHANGELOG.md`. Open a PR and merge it once CI passes.
2. From an up-to-date `master`:

   ```sh
   v=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version')
   : "${v:?could not read the version; is cargo on PATH?}"
   git tag -a "v$v" -m "safesweep $v" && git push origin "v$v"
   gh release create "v$v" --title "safesweep $v" --notes-file <(sed -n "/^## $v/,/^## /p" CHANGELOG.md | sed '1d;$d')
   ```

Publishing the release does the rest:

| Workflow | Publishes |
|---|---|
| `release.yml` | static binaries, `.deb`, `.rpm` on the release; crates.io; the apt repo on GitHub Pages |
| `pypi.yml` | wheels and sdist to PyPI |
| `npm.yml` | npm packages, after `release.yml` succeeds (trusted publishing; there is no npm token) |
| `dpologdvinity/homebrew-tap` | formula; `release.yml` asks the tap to update right away (it also polls every 6 hours) |

Before anything publishes, `verify-tag.yml` (called first by `release.yml`, `pypi.yml` and `npm.yml`)
refuses a tag that is not `vMAJOR.MINOR.PATCH[-pre]` or does not match `Cargo.toml`. A tag with a `-pre` suffix is
a prerelease: it gets binaries, crates.io and PyPI, goes to npm under the `next` dist-tag, and does
not touch the apt repo or Homebrew.

Re-running is safe: each step skips what is already published, and release assets are never replaced
(rebuilt binaries are not byte-identical, and Homebrew and apt pin checksums; a missing `.sha256` is
derived from the tarball already on the release). The apt repo holds only the latest stable release, so
re-running `release.yml` for an older tag leaves it alone.

To publish one channel for an existing release: `gh workflow run npm.yml -f tag=vX.Y.Z` (same for
`pypi.yml` and `release.yml`). Tags older than v0.6.0 predate the packaging scripts and cannot be re-run.
