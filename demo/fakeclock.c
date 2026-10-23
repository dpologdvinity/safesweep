// Demo only: makes safesweep see "now" as 30 days later, so freshly created fixtures count as old.
// cc -shared -fPIC -o fakeclock.so fakeclock.c -ldl
#define _GNU_SOURCE
#include <dlfcn.h>
#include <time.h>

int clock_gettime(clockid_t clk, struct timespec *ts) {
    static int (*real)(clockid_t, struct timespec *);
    if (!real) real = dlsym(RTLD_NEXT, "clock_gettime");
    int r = real(clk, ts);
    if (clk == CLOCK_REALTIME) ts->tv_sec += 30 * 86400;
    return r;
}
