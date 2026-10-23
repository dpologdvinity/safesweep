# Sourced by make.sh: builds a throwaway home with fake caches and defines `safesweep`.
# Nothing here touches the real home directory. /tmp/h and /tmp/h-tmp keep the GIF's paths
# short; they are created fresh with mode 700 and the script stops if another user owns them.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$here/.." && pwd)
rm -rf /tmp/h /tmp/h-tmp
mkdir -m 700 /tmp/h /tmp/h-tmp || { echo "demo: cannot create /tmp/h or /tmp/h-tmp" >&2; return 1; }
priv=$(mktemp -d)  # private dir for the LD_PRELOAD shim: never load code from a guessable path
mk() { mkdir -p "$(dirname "$1")"; dd if=/dev/zero of="$1" bs=1M count="$2" status=none; }
mk /tmp/h/.cache/pip/http/blob 180
mk /tmp/h/.cache/ms-playwright/chromium/chrome 420
mk /tmp/h/.cache/fontconfig/cache 2
mk /tmp/h/.cache/thumbnails/large/t 12
mk /tmp/h/.cache/chromium/Cache/data 60
mk /tmp/h/.npm/_cacache/content/blob 90
mk /tmp/h-tmp/build-4821/out.o 50
mk /tmp/h-tmp/pytest-of-me/log 8
tail -f /tmp/h/.cache/chromium/Cache/data >/dev/null 2>&1 &  # a process holding a file open
cc -shared -fPIC -o "$priv/fakeclock.so" "$here/fakeclock.c" -ldl || return 1
export HOME=/tmp/h TMPDIR=/tmp/h-tmp PS1='$ '
safesweep() {
  if [ "$1" = why ]; then
    # Own PID namespace: why only sees three busy-loop workers, not the real machine's processes.
    unshare -rpf --mount-proc sh -c 'for i in 1 2 3; do yes >/dev/null & done; sleep 1; exec "$0" "$@"' \
      "$root/target/release/safesweep" "$@"
  else LD_PRELOAD="$priv/fakeclock.so" "$root/target/release/safesweep" "$@"; fi
}
