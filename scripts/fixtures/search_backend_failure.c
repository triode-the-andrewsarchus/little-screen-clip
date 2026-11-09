// Private acceptance only: one fatal compute status per CUDA model role.
// Delegates every other call to the installed library; never allocates an OOM.
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int32_t llama_process(void *context, int type, void *batch) {
    const char *directory = getenv("CLIP_TEST_BACKEND_FAILURE_DIR");
    char command[4096];
    int fd = open("/proc/self/cmdline", O_RDONLY | O_CLOEXEC);
    ssize_t length = fd >= 0 ? read(fd, command, sizeof(command)) : -1;
    if (fd >= 0) close(fd);
    const char cuda[] = "--device\0cuda";
    const char embedding[] = "--role\0embedding";
    const char answer[] = "--role\0answer";
    const char *role = NULL;
    if (length > 0 && memmem(command, (size_t)length, embedding, sizeof(embedding)-1)) role = "embedding";
    if (length > 0 && memmem(command, (size_t)length, answer, sizeof(answer)-1)) role = "answer";
    if (directory && role && length > 0 && memmem(command, (size_t)length, cuda, sizeof(cuda)-1)) {
        char path[4096];
        if (snprintf(path, sizeof(path), "%s/failed-%s.pid", directory, role) >= (int)sizeof(path)) abort();
        fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
        if (fd >= 0) {
            char pid[32];
            int count = snprintf(pid, sizeof(pid), "%ld\n", (long)getpid());
            if (write(fd, pid, (size_t)count) != count) abort();
            close(fd);
            return -2;
        }
        if (errno != EEXIST) abort();
    }
    static int32_t (*original)(void *, int, void *) = NULL;
    if (!original) {
        const char *path = getenv("CLIP_TEST_REAL_LLAMA");
        void *library = path ? dlopen(path, RTLD_NOW | RTLD_NOLOAD) : NULL;
        if (library) original = dlsym(library, "llama_process");
        if (!original) _exit(125);
    }
    return original(context, type, batch);
}
