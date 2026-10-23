# Sourced by demo.tape: builds a throwaway home with fake caches and defines `safesweep`.
# Nothing here touches the real home directory.
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(cd "$here/.." && pwd)
rm -rf /tmp/h /tmp/h-tmp
mkdir -p /tmp/h /tmp/h-tmp
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
cc -shared -fPIC -o /tmp/fakeclock.so "$here/fakeclock.c" -ldl
export HOME=/tmp/h TMPDIR=/tmp/h-tmp PS1='$ '
safesweep() {
  if [ "$1" = why ]; then
    # Own PID namespace: why only sees three busy-loop workers, not the real machine's processes.
    unshare -rpf --mount-proc sh -c 'for i in 1 2 3; do yes >/dev/null & done; sleep 1; exec "$0" "$@"' \
      "$root/target/release/safesweep" "$@"
  else LD_PRELOAD=/tmp/fakeclock.so "$root/target/release/safesweep" "$@"; fi
}
