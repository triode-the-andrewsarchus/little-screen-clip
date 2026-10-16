/* Private acceptance-only interposition; never manufacture kernel events. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>

static atomic_uint attempts;
static atomic_int monitored_fd = -1;
static atomic_int overflow_stage;
static _Atomic uint64_t enabled_ms;

static uint64_t milliseconds(void) {
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    return (uint64_t)now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

__attribute__((constructor)) static void owner(void) {
    if (!getenv("VEYLAR_UDEV_FAULT_OWNER")) {
        char pid[32];
        snprintf(pid, sizeof pid, "%ld", (long)getpid());
        setenv("VEYLAR_UDEV_FAULT_OWNER", pid, 1);
    }
}

static int owned(void) {
    const char *pid = getenv("VEYLAR_UDEV_FAULT_OWNER");
    return pid && strtol(pid, NULL, 10) == (long)getpid();
}

static void record(const char *event, int attempt) {
    const char *path = getenv("VEYLAR_UDEV_FAULT_LOG");
    if (!path) return;
    int fd = open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
    if (fd < 0) return;
    dprintf(fd, "%llu %ld %s %d\n", (unsigned long long)milliseconds(), (long)getpid(), event, attempt);
    close(fd);
}

int udev_monitor_enable_receiving(void *monitor) {
    int (*next)(void *) = dlsym(RTLD_NEXT, "udev_monitor_enable_receiving");
    if (!owned()) return next(monitor);
    unsigned attempt = atomic_fetch_add(&attempts, 1);
    if (attempt < 4 || attempt == 5 || attempt == 6) {
        record("denied", attempt);
        errno = EACCES;
        return -EACCES;
    }
    int result = next(monitor);
    record(result < 0 ? "real-error" : "enabled", attempt);
    if (result >= 0 && !atomic_load(&enabled_ms)) atomic_store(&enabled_ms, milliseconds());
    return result;
}

int udev_monitor_get_fd(void *monitor) {
    int (*next)(void *) = dlsym(RTLD_NEXT, "udev_monitor_get_fd");
    int fd = next(monitor);
    if (owned() && fd >= 0) atomic_store(&monitored_fd, fd);
    return fd;
}

int poll(struct pollfd *fds, nfds_t count, int timeout) {
    int (*next)(struct pollfd *, nfds_t, int) = dlsym(RTLD_NEXT, "poll");
    uint64_t enabled = atomic_load(&enabled_ms);
    int target = atomic_load(&monitored_fd);
    if (owned() && target >= 0 && enabled && milliseconds() - enabled >= 4000) {
        for (nfds_t i = 0; i < count; i++) {
            int expected = 0;
            if (fds[i].fd == target && atomic_compare_exchange_strong(&overflow_stage, &expected, 1)) {
                fds[i].revents = POLLIN;
                return 1;
            }
        }
    }
    return next(fds, count, timeout);
}

void *udev_monitor_receive_device(void *monitor) {
    void *(*next)(void *) = dlsym(RTLD_NEXT, "udev_monitor_receive_device");
    int expected = 1;
    if (owned() && atomic_compare_exchange_strong(&overflow_stage, &expected, 2)) {
        record("overflow", -1);
        errno = ENOBUFS;
        return NULL;
    }
    return next(monitor);
}
