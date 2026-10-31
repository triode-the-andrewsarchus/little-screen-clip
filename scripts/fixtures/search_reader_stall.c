#define _GNU_SOURCE
/* Test-only preload: hold one owned reader's enumeration, file read or request.
 * It never changes the application binary, file contents or the host compositor.
 * The gate must be released by the scenario's finally block.
 */
#include <dirent.h>
#include <dlfcn.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static char request_prefix[4096];
static size_t request_length;

static int target_matches(const char *path, const char *target) {
    if (path) {
        char actual[PATH_MAX];
        ssize_t size = readlink(path, actual, sizeof(actual) - 1);
        if (size < 0) return 0;
        actual[size] = 0;
        return strcmp(actual, target) == 0;
    }
    if (!getenv("CLIP_TEST_STALL_AT_REQUEST")) return 0;
    char quoted[PATH_MAX + 3];
    int size = snprintf(quoted, sizeof(quoted), "\"%s\"", target);
    return size > 0 && (size_t)size < sizeof(quoted) && memmem(request_prefix, request_length, quoted, (size_t)size);
}

static int has_argument(const char *argument) {
    char command[4096];
    int fd = open("/proc/self/cmdline", O_RDONLY | O_CLOEXEC);
    if (fd < 0) return 0;
    ssize_t length = read(fd, command, sizeof(command));
    close(fd);
    return length > 0 && memmem(command, (size_t)length, argument, strlen(argument) + 1);
}

static void hold_process(const char *marker, const char *release, int exclusive) {
    int flags = O_WRONLY | O_CREAT | O_CLOEXEC | (exclusive ? O_EXCL : O_TRUNC);
    int output = open(marker, flags, 0600);
    if (output < 0) return;
    dprintf(output, "%ld\n", (long)getpid());
    close(output);
    const struct timespec interval = { .tv_sec = 0, .tv_nsec = 10000000 };
    while (access(release, F_OK) != 0) nanosleep(&interval, NULL);
}

static void hold_model_request(const void *buffer, size_t length) {
    const char *text = getenv("CLIP_TEST_STALL_MODEL_TEXT");
    const char *marker = getenv("CLIP_TEST_STALL_MARKER");
    const char *release = getenv("CLIP_TEST_STALL_RELEASE");
    const char *role = getenv("CLIP_TEST_STALL_MODEL_ROLE");
    const char *op = getenv("CLIP_TEST_STALL_MODEL_OP");
    char operation[128];
    if (!role) role = "embedding";
    if (!op) op = "text";
    int size = strcmp(op, "*") == 0 ? snprintf(operation, sizeof(operation), "\"op\":") :
        snprintf(operation, sizeof(operation), "\"op\":\"%s\"", op);
    if (!text || !marker || !release || size < 0 || (size_t)size >= sizeof(operation) || access(release, F_OK) == 0) return;
    if (!memmem(buffer, length, operation, (size_t)size) ||
        !memmem(buffer, length, text, strlen(text)) || !has_argument(role)) return;
    /* One owned request only; replacement helpers can complete the retry. */
    hold_process(marker, release, 1);
}

static void wait_for_release(const char *path) {
    const char *target = getenv("CLIP_TEST_STALL_DIRECTORY");
    const char *marker = getenv("CLIP_TEST_STALL_MARKER");
    const char *release = getenv("CLIP_TEST_STALL_RELEASE");
    if (!target || !marker || !release || access(release, F_OK) == 0) return;
    const char *operation = getenv("CLIP_TEST_STALL_OPERATION");
    if (operation && !memmem(request_prefix, request_length, operation, strlen(operation))) return;
    if (!target_matches(path, target)) return;
    if (!has_argument("--internal-file-search-reader")) return;
    hold_process(marker, release, 0);
}

ssize_t read(int fd, void *buffer, size_t count) {
    ssize_t (*original)(int, void *, size_t) = dlsym(RTLD_NEXT, "read");
    if (fd >= 3 && getenv("CLIP_TEST_STALL_OPERATION")) {
        char label[64];
        snprintf(label, sizeof(label), "/proc/self/fd/%d", fd);
        wait_for_release(label);
    }
    ssize_t result = original(fd, buffer, count);
    if (fd == STDIN_FILENO && result > 0) hold_model_request(buffer, (size_t)result);
    if (fd == STDIN_FILENO && result > 0 && request_length < sizeof(request_prefix)) {
        size_t length = (size_t)result;
        if (length > sizeof(request_prefix) - request_length) length = sizeof(request_prefix) - request_length;
        memcpy(request_prefix + request_length, buffer, length);
        request_length += length;
    }
    if (fd == STDIN_FILENO && result == 0) wait_for_release(NULL);
    return result;
}

DIR *opendir(const char *path) {
    wait_for_release(path);
    DIR *(*original)(const char *) = dlsym(RTLD_NEXT, "opendir");
    return original(path);
}

DIR *fdopendir(int fd) {
    char label[64];
    snprintf(label, sizeof(label), "/proc/self/fd/%d", fd);
    wait_for_release(label);
    DIR *(*original)(int) = dlsym(RTLD_NEXT, "fdopendir");
    return original(fd);
}
