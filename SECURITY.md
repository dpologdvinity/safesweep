# Security policy

`safesweep` deletes files, so its safety matters more than its features. Reports of problems are
welcome and taken seriously.

## Supported versions

Only the latest release is supported. Upgrade first: several releases before 0.5.0 contain bugs that
were fixed after security reviews (for example, files used inside containers were not detected).

## What counts as a vulnerability

Anything that makes `safesweep` act against its promises (see
[ARCHITECTURE.md](ARCHITECTURE.md#the-safety-model)). In particular:

- **A false "safe"**: a file deleted although a program was using it, it was changed recently, it
  belonged to another user, or it lay outside the rule's directory.
- **An escape**: following a symlink or mount point out of the intended directory; deleting something no
  rule names.
- **Privilege misuse**: a way to make a root run delete files the user could not, for example through
  `HOME`, `TMPDIR` or a symlinked cache directory.
- **Untrusted text reaching the terminal or the log unescaped**: a file name, error message, process name
  or command line that can fake output rows or inject terminal escape sequences.
- **A supply-chain weakness** in the release process: a way to publish a package that is not built from
  this repository's tagged source, or to leak a publishing credential.

Ordinary bugs (wrong output, crashes without data loss) are welcome as normal
[issues](https://github.com/dpologdvinity/safesweep/issues).

## How to report

Please **do not open a public issue** for a vulnerability. Use GitHub's private reporting:

1. Open <https://github.com/dpologdvinity/safesweep/security/advisories/new>.
2. Describe what happens, the exact steps or a small script that reproduces it (a throwaway directory
   with fake files is ideal; never test against real data), the version, and your system.

You will get an acknowledgement as soon as the maintainer sees it, normally within a few days. This is a
single-maintainer project, so there is no formal deadline, but a confirmed data-loss or privilege issue
is handled before anything else, and a fixed release is published with credit to you (if you want it).

## What is already defended against

The checks and the reasoning are in [ARCHITECTURE.md](ARCHITECTURE.md#the-safety-model). Releases are
built in GitHub Actions from tagged source; every action is pinned to a commit, publishing tokens are
scoped to the jobs that need them, and a release tag must match the version in `Cargo.toml` before
anything is published.

## Verifying a download

Release tarballs have a `.sha256` file next to them (the `curl` installer, `install.sh`, checks it before it installs
anything and never uses `sudo`), the apt repository is signed (fingerprint
`A5DA1DFEA1ADAEAD756B0A8F88D35216B52C4C69`, public key at
<https://dpologdvinity.github.io/safesweep/key.asc>), and the npm packages are published with provenance
(npmjs.com shows the GitHub Actions run that built each one).
