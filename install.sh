#!/bin/sh
# safesweep installer: downloads a release binary, checks its checksum, and installs it.
#
#   curl -fsSL https://github.com/dpologdvinity/safesweep/releases/latest/download/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --version 0.6.5 --bin-dir /usr/local/bin
#
# Options:
#   -v, --version X   install this version (for example 0.6.5) instead of the latest stable release
#   -b, --bin-dir D   install the program into D (default: $HOME/.local/bin)
#   -h, --help        show this help
# Environment (same as the options): SAFESWEEP_VERSION, SAFESWEEP_BIN_DIR.
#   SAFESWEEP_BASE_URL  where releases are downloaded from (default: GitHub). For mirrors and tests.
#
# What it does, in order:
#   1. refuses anything but Linux (including WSL) on x86_64 or aarch64: the program reads /proc;
#   2. finds the version (the latest stable release unless one is given) and checks it looks like
#      a version number, since it ends up in URLs and file names;
#   3. downloads the release tarball and its .sha256 file, and refuses to go on if they disagree;
#   4. installs the program, and when D is a ".../bin" directory also the man page and the bash,
#      zsh and fish completions under the matching prefix (D without "/bin", e.g. ~/.local/share);
#   5. runs the installed program and checks that it reports the expected version.
#
# It never uses sudo and never touches anything outside the directories above. To uninstall, delete
# the files it prints. POSIX sh, so it runs under dash, ash (Alpine) and bash.
#
# Everything is inside main(), which runs on the last line: if the download is cut short, the shell
# sees an incomplete function definition and runs nothing, instead of executing half a script.

set -eu

REPO="dpologdvinity/safesweep"
BASE_URL="${SAFESWEEP_BASE_URL:-https://github.com/$REPO/releases/download}"

say() { printf 'safesweep-install: %s\n' "$*"; }
err() { printf 'safesweep-install: error: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<'HELP'
safesweep installer: downloads a release binary, checks its checksum, and installs it.

  curl -fsSL https://github.com/dpologdvinity/safesweep/releases/latest/download/install.sh | sh
  curl -fsSL .../install.sh | sh -s -- --version 0.6.5 --bin-dir /usr/local/bin

Options:
  -v, --version X   install this version (for example 0.6.5) instead of the latest stable release
  -b, --bin-dir D   install the program into D (default: $HOME/.local/bin)
  -h, --help        show this help
Environment: SAFESWEEP_VERSION, SAFESWEEP_BIN_DIR (same as the options), SAFESWEEP_BASE_URL (mirror).
Linux and WSL only; it never uses sudo. Details: the comments at the top of install.sh.
HELP
}

# Success if the command exists.
have() { command -v "$1" >/dev/null 2>&1; }

# download URL FILE: save URL to FILE with curl, or wget if curl is missing. Fails on HTTP errors.
download() {
  if have curl; then
    curl -fsSL --retry 3 -o "$2" "$1" || err "download failed: $1"
  elif have wget; then
    wget -q -O "$2" "$1" || err "download failed: $1"
  else
    err "need curl or wget to download files"
  fi
}

# sha256 FILE: print the SHA-256 of FILE (hex only).
sha256() {
  if have sha256sum; then sha256sum "$1" | cut -d' ' -f1
  elif have shasum; then shasum -a 256 "$1" | cut -d' ' -f1
  else err "need sha256sum or shasum to verify the download"
  fi
}

# latest_version: print the newest stable release number (for example 0.6.5) without the "v".
# GitHub redirects /releases/latest to /releases/tag/vX.Y.Z; reading the redirect needs no API
# token and has no rate limit.
latest_version() {
  url="https://github.com/$REPO/releases/latest"
  if have curl; then
    final=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$url") || err "could not look up the latest release"
  elif have wget; then
    final=$(wget --spider -S "$url" 2>&1 | sed -n 's/^ *[Ll]ocation: *//p' | tail -n 1) || true
  else
    err "need curl or wget to download files"
  fi
  tag=${final##*/}
  [ -n "$tag" ] || err "could not look up the latest release"
  printf '%s\n' "${tag#v}"
}

main() {
  version="${SAFESWEEP_VERSION:-}"
  bin_dir="${SAFESWEEP_BIN_DIR:-}"
  while [ $# -gt 0 ]; do
    case "$1" in
      -v|--version) [ $# -ge 2 ] || err "$1 needs a value"; version=$2; shift 2 ;;
      -b|--bin-dir) [ $# -ge 2 ] || err "$1 needs a value"; bin_dir=$2; shift 2 ;;
      -h|--help) usage; exit 0 ;;
      *) err "unknown option: $1 (try --help)" ;;
    esac
  done

  # 1. Platform. The program reads /proc, so it only works on Linux (WSL included).
  os=$(uname -s)
  [ "$os" = Linux ] || err "safesweep only runs on Linux and WSL (this is $os)"
  case "$(uname -m)" in
    x86_64|amd64) target=x86_64-unknown-linux-musl ;;
    aarch64|arm64) target=aarch64-unknown-linux-musl ;;
    *) err "unsupported CPU: $(uname -m) (supported: x86_64, aarch64)" ;;
  esac

  # 2. Version. Only plain release numbers are accepted: it becomes part of URLs and file names.
  [ -n "$version" ] || version=$(latest_version)
  version=${version#v}
  case "$version" in
    ''|*[!0-9.]*|.*|*.|*..*) err "not a release version: '$version' (expected like 0.6.5)" ;;
  esac
  [ "$(printf '%s' "$version" | tr -cd . | wc -c | tr -d ' ')" = 2 ] || err "not a release version: '$version' (expected like 0.6.5)"

  [ -n "$bin_dir" ] || { [ -n "${HOME:-}" ] || err "HOME is not set; use --bin-dir"; bin_dir="$HOME/.local/bin"; }
  case "$bin_dir" in /*) ;; *) err "--bin-dir must be an absolute path: $bin_dir" ;; esac

  # 3. Download and verify. The work happens in a private temporary directory that is always removed.
  tmp=$(mktemp -d) || err "could not create a temporary directory"
  trap 'rm -rf "$tmp"' EXIT INT TERM HUP
  name="safesweep-v$version-$target"
  say "installing safesweep $version ($target)"
  download "$BASE_URL/v$version/$name.tar.gz" "$tmp/$name.tar.gz"
  download "$BASE_URL/v$version/$name.tar.gz.sha256" "$tmp/$name.tar.gz.sha256"
  expected=$(cut -d' ' -f1 "$tmp/$name.tar.gz.sha256" | head -n 1)
  case "$expected" in
    *[!0-9a-f]*|'') err "the checksum file is malformed" ;;
  esac
  [ "${#expected}" = 64 ] || err "the checksum file is malformed"
  actual=$(sha256 "$tmp/$name.tar.gz")
  [ "$actual" = "$expected" ] || err "checksum mismatch: the download is corrupt or has been tampered with (expected $expected, got $actual)"
  say "checksum verified"

  tar -xzf "$tmp/$name.tar.gz" -C "$tmp" || err "could not unpack the download"
  src="$tmp/$name"
  [ -f "$src/safesweep" ] || err "the download does not contain the program"

  # 4. Install. The program is copied next to its destination and renamed into place, so an
  # existing copy is replaced atomically and never left half-written.
  mkdir -p "$bin_dir" || err "cannot create $bin_dir"
  cp "$src/safesweep" "$bin_dir/.safesweep.new.$$" || err "cannot write to $bin_dir"
  chmod 755 "$bin_dir/.safesweep.new.$$"
  mv -f "$bin_dir/.safesweep.new.$$" "$bin_dir/safesweep" || err "cannot install into $bin_dir"
  say "installed $bin_dir/safesweep"

  # The man page and completions go under the prefix when the program goes into <prefix>/bin.
  case "$bin_dir" in
    */bin)
      prefix=${bin_dir%/bin}
      place() { # place SOURCE DESTINATION: copy if the release has the file (old releases lack it)
        [ -f "$1" ] || return 0
        mkdir -p "$(dirname "$2")" && cp "$1" "$2" && say "installed $2"
      }
      place "$src/safesweep.1" "$prefix/share/man/man1/safesweep.1"
      place "$src/safesweep.bash" "$prefix/share/bash-completion/completions/safesweep"
      place "$src/_safesweep" "$prefix/share/zsh/site-functions/_safesweep"
      place "$src/safesweep.fish" "$prefix/share/fish/vendor_completions.d/safesweep.fish"
      ;;
  esac

  # 5. Check that what was installed is what was asked for.
  got=$("$bin_dir/safesweep" --version 2>/dev/null) || err "the installed program does not run"
  [ "$got" = "safesweep $version" ] || err "the installed program says '$got', expected 'safesweep $version'"
  say "done: $got"

  case ":${PATH:-}:" in
    *":$bin_dir:"*) ;;
    *) say "note: $bin_dir is not in your PATH. Add it, for example: export PATH=\"$bin_dir:\$PATH\"" ;;
  esac
}

main "$@"
