# Releasing

1. On a branch, bump `version` in `Cargo.toml`, run `cargo build` (updates `Cargo.lock`), and add the
   release to `CHANGELOG.md`. Open a PR and merge it once CI passes.
2. From an up-to-date `master`:

   ```sh
   v=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version')
   git tag -a "v$v" -m "safesweep $v" && git push origin "v$v"
   gh release create "v$v" --title "safesweep $v" --notes-file <(sed -n "/^## $v/,/^## /p" CHANGELOG.md | sed '1d;$d')
   ```

Publishing the release does the rest:

| Workflow | Publishes |
|---|---|
| `release.yml` | static binaries, `.deb`, `.rpm` on the release; crates.io; the apt repo on GitHub Pages |
| `pypi.yml` | wheels and sdist to PyPI |
| `npm.yml` | npm packages, after `release.yml` succeeds |
| `dpologdvinity/homebrew-tap` | formula, picked up within 6 hours (or run its `update-safesweep` workflow) |

Each step skips what is already published, so re-running a workflow is safe. Re-runs never
replace release assets: rebuilt binaries are not byte-identical, and Homebrew and apt pin checksums.

To publish one channel for an existing release: `gh workflow run npm.yml -f tag=vX.Y.Z`
(same for `pypi.yml` and `release.yml`).
