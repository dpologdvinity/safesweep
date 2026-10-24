// Demo only. A tiny preload library that makes a program see "now" as 30 days later, so the
// files the demo creates a moment ago count as old enough for safesweep to delete them.
//
// Build:  cc -shared -fPIC -o fakeclock.so fakeclock.c -ldl
// Use:    LD_PRELOAD=./fakeclock.so safesweep scan        (demo/env.sh does this)
//
// It replaces `clock_gettime` for the wall clock only; every other clock (the ones used to
// measure durations) is untouched. It only works on dynamically linked programs, so the
// static release binaries ignore it: the demo uses the locally built one.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <time.h>

int clock_gettime(clockid_t clk, struct timespec *ts) {
    // look up the real implementation once, then forward to it
    static int (*real)(clockid_t, struct timespec *);
    if (!real) real = dlsym(RTLD_NEXT, "clock_gettime");
    int r = real(clk, ts);
    if (clk == CLOCK_REALTIME) ts->tv_sec += 30 * 86400;
    return r;
}
