#!/bin/sh
# Tests for install.sh. Run: sh tests/install_test.sh   (needs curl, tar, sha256sum; no network)
#
# A fake release is built in a temporary directory and served through SAFESWEEP_BASE_URL as
# file:// URLs, so nothing is downloaded from GitHub and nothing outside the temporary directory
# is touched (HOME is pointed there too). `uname` is replaced by a stand-in to test platform checks.
# Each test states what must be refused or done; every refusal also checks that nothing was installed.
set -u
here=$(cd "$(dirname "$0")/.." && pwd)
INSTALL="$here/install.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
pass=0; fail=0

ok() { pass=$((pass + 1)); printf 'ok   %s\n' "$1"; }
bad() { fail=$((fail + 1)); printf 'FAIL %s\n' "$1"; }
# check NAME CONDITION...: passes when the command succeeds
check() { name=$1; shift; if "$@"; then ok "$name"; else bad "$name"; fi; }
contains() { printf '%s' "$1" | grep -qF -- "$2"; }
# refused_with TEXT: the last run failed AND its output says TEXT (so the right check refused it,
# not some later failure such as a download that happens to 404)
refused_with() { [ "$RC" != 0 ] && contains "$OUT" "$1"; }

# make_release VERSION: a fake release for the host's real CPU in $work/rel/vVERSION
make_release() {
  v=$1
  case "$(uname -m)" in aarch64|arm64) t=aarch64-unknown-linux-musl ;; *) t=x86_64-unknown-linux-musl ;; esac
  n="safesweep-v$v-$t"
  d="$work/rel/v$v"; rm -rf "$d" "$work/pkg"; mkdir -p "$d" "$work/pkg/$n"
  printf '#!/bin/sh\necho "safesweep %s"\n' "$v" > "$work/pkg/$n/safesweep"; chmod 755 "$work/pkg/$n/safesweep"
  echo "man page" > "$work/pkg/$n/safesweep.1"
  echo "bash completion" > "$work/pkg/$n/safesweep.bash"
  echo "zsh completion" > "$work/pkg/$n/_safesweep"
  echo "fish completion" > "$work/pkg/$n/safesweep.fish"
  (cd "$work/pkg" && tar czf "$d/$n.tar.gz" "$n")
  (cd "$d" && sha256sum "$n.tar.gz" > "$n.tar.gz.sha256")
  SUMFILE="$d/$n.tar.gz.sha256"
}

# run_install ARGS...: run the installer against the fake releases; sets OUT and RC
run_install() {
  OUT=$(HOME="$work/home" SAFESWEEP_BASE_URL="file://$work/rel" sh "$INSTALL" "$@" 2>&1); RC=$?
}
fresh_home() { rm -rf "${work:?}/home" "${work:?}/bin"; mkdir -p "$work/home"; }

# --- success ----------------------------------------------------------------------------------
make_release 1.2.3; fresh_home
run_install --version 1.2.3
check "installs into ~/.local/bin by default"    test -x "$work/home/.local/bin/safesweep"
check "the installed program is the right one"    test "$("$work/home/.local/bin/safesweep")" = "safesweep 1.2.3"
check "exits 0"                                   test "$RC" = 0
check "installs the man page under the prefix"    test -f "$work/home/.local/share/man/man1/safesweep.1"
check "installs bash, zsh and fish completions"   test -f "$work/home/.local/share/bash-completion/completions/safesweep" -a -f "$work/home/.local/share/zsh/site-functions/_safesweep" -a -f "$work/home/.local/share/fish/vendor_completions.d/safesweep.fish"
check "warns when the directory is not in PATH"   contains "$OUT" "not in your PATH"
check "leaves no temporary files behind"          test -z "$(find "$work/home/.local/bin" -name '.safesweep.new.*')"

fresh_home
run_install --version v1.2.3 --bin-dir "$work/bin"
check "accepts a leading v and --bin-dir"         test "$RC" = 0 -a -x "$work/bin/safesweep"
check "extras follow a .../bin directory"         test -f "$work/share/man/man1/safesweep.1"

fresh_home; rm -rf "$work/share"
run_install --version 1.2.3 --bin-dir "$work/tools"
check "no extras when the dir is not .../bin"     test "$RC" = 0 -a -x "$work/tools/safesweep" -a ! -e "$work/share" -a ! -e "$work/tools/share"

fresh_home; mkdir -p "$work/home/.local/bin"; echo old > "$work/home/.local/bin/safesweep"
run_install --version 1.2.3
check "replaces an existing install"              test "$("$work/home/.local/bin/safesweep")" = "safesweep 1.2.3"

fresh_home
OUT=$(HOME="$work/home" SAFESWEEP_BASE_URL="file://$work/rel" SAFESWEEP_VERSION=1.2.3 sh "$INSTALL" 2>&1); RC=$?
check "reads SAFESWEEP_VERSION"                   test "$RC" = 0 -a -x "$work/home/.local/bin/safesweep"

# --- refusals: each must fail and install nothing ---------------------------------------------
make_release 1.2.3; fresh_home
printf '%s  safesweep-v1.2.3-x.tar.gz\n' "$(printf 'x' | sha256sum | cut -d' ' -f1)" > "$SUMFILE"
run_install --version 1.2.3
check "refuses a checksum mismatch"               test "$RC" != 0
check "...and says so"                            contains "$OUT" "checksum mismatch"
check "...and installs nothing"                   test ! -e "$work/home/.local/bin/safesweep"

make_release 1.2.3; fresh_home; echo "not a checksum" > "$SUMFILE"
run_install --version 1.2.3
check "refuses a malformed checksum file"         refused_with "checksum file is malformed"
check "...and installs nothing"                   test ! -e "$work/home/.local/bin/safesweep"

make_release 1.2.3; fresh_home; echo "abc123  file.tar.gz" > "$SUMFILE"
run_install --version 1.2.3
check "refuses a checksum of the wrong length"    refused_with "checksum file is malformed"

make_release 1.2.3; fresh_home; : > "$SUMFILE"
run_install --version 1.2.3
check "refuses an empty checksum file"            refused_with "checksum file is malformed"

make_release 1.2.3; fresh_home; rm "$SUMFILE"
run_install --version 1.2.3
check "refuses when the checksum is missing"      test "$RC" != 0 -a ! -e "$work/home/.local/bin/safesweep"

make_release 1.2.3; fresh_home
run_install --version 9.9.9
check "fails clearly for a version that does not exist" test "$RC" != 0 -a ! -e "$work/home/.local/bin/safesweep"

for v in 'v1.2' '1.2.3.4' '1.2.x' '1.2.3; touch /tmp/pwned' '../1.2.3' '-1.2.3' '1..3' '.1.2.3' '1.2.3.'; do
  fresh_home; run_install --version "$v"
  check "rejects the version '$v'"                refused_with "not a release version"
  check "...and installs nothing"                 test ! -e "$work/home/.local/bin/safesweep" -a ! -e /tmp/pwned
done

fresh_home; run_install --bin-dir relative/dir --version 1.2.3
check "rejects a relative --bin-dir"              refused_with "must be an absolute path"
run_install --bogus
check "rejects an unknown option"                 refused_with "unknown option"
run_install --version
check "rejects an option with no value"           refused_with "needs a value"

# the program that gets installed must report the version that was asked for
make_release 1.2.3; fresh_home
case "$(uname -m)" in aarch64|arm64) t=aarch64-unknown-linux-musl ;; *) t=x86_64-unknown-linux-musl ;; esac
n="safesweep-v1.2.3-$t"; printf '#!/bin/sh\necho "safesweep 6.6.6"\n' > "$work/pkg/$n/safesweep"; chmod 755 "$work/pkg/$n/safesweep"
(cd "$work/pkg" && tar czf "$work/rel/v1.2.3/$n.tar.gz" "$n"); (cd "$work/rel/v1.2.3" && sha256sum "$n.tar.gz" > "$n.tar.gz.sha256")
run_install --version 1.2.3
check "fails when the program reports another version" refused_with "expected 'safesweep 1.2.3'"

# --- platform checks, with a stand-in uname ---------------------------------------------------
fake_uname() { # fake_uname SYSTEM MACHINE
  mkdir -p "$work/fakebin"
  # shellcheck disable=SC2016  # single quotes on purpose: $1 is for the generated script, not this one
  printf '#!/bin/sh\ncase "$1" in -s) echo %s ;; -m) echo %s ;; *) echo %s ;; esac\n' "$1" "$2" "$1" > "$work/fakebin/uname"
  chmod 755 "$work/fakebin/uname"
}
make_release 1.2.3; fresh_home
fake_uname Darwin arm64
OUT=$(PATH="$work/fakebin:$PATH" HOME="$work/home" SAFESWEEP_BASE_URL="file://$work/rel" sh "$INSTALL" --version 1.2.3 2>&1); RC=$?
check "refuses macOS"                             refused_with "only runs on Linux"
fake_uname Linux armv7l
OUT=$(PATH="$work/fakebin:$PATH" HOME="$work/home" SAFESWEEP_BASE_URL="file://$work/rel" sh "$INSTALL" --version 1.2.3 2>&1); RC=$?
check "refuses an unsupported CPU"                refused_with "unsupported CPU"
rm -rf "$work/fakebin"

# --- the script itself ------------------------------------------------------------------------
run_install --help
check "--help exits 0"                            test "$RC" = 0
check "--help lists the options"                  contains "$OUT" "--bin-dir"
check "the last line runs main (truncation safe)" test "$(tail -n 1 "$INSTALL")" = 'main "$@"'
# sudo as a command (at the start of a line or after ; & | ( or a backtick), not the word in help text
check "never runs sudo"                           test -z "$(grep -v '^ *#' "$INSTALL" | grep -E '(^|[;&|(`]) *sudo( |$)')"

printf '\n%s passed, %s failed\n' "$pass" "$fail"
[ "$fail" = 0 ]
