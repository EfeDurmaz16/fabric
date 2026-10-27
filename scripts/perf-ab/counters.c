/*
 * The counters of the A/B performance lab (scripts/bench-ab.sh).
 *
 * Loaded into every process of a counted run with DYLD_INSERT_LIBRARIES,
 * it counts, in each `fabric` process, the work that does not depend on how
 * loaded the machine is: processes started, file flushes by kind, opens,
 * renames and unlinks, allocations, and the bytes and calls of every read
 * and write, by the class of file they touch. It also keeps the process's
 * peak resident size and CPU time.
 *
 * Each `fabric` process maps a page of its own,
 * FABRIC_AB_COUNTS/<pid>.<ns>.page, shared with the file, so the e2e test
 * `ab::paired` reads the counts of a running daemon as well as those of a
 * CLI that has exited, and sums the pages before and after each op. Beside
 * it, <pid>.<ns>.spawns gets a line per process started: the program's
 * file name and its arguments. Any other process, git or a harness, maps
 * nothing and counts nothing; the calls pass straight through.
 *
 * macOS only: dyld's __interpose section replaces each function for every
 * image but this one, so the calls below reach the real functions.
 * Built by scripts/bench-ab.sh with `cc -dynamiclib -O2`.
 *
 * The layout is read by crates/fabric/tests/e2e/ab.rs; change both or
 * neither. The page is 512 little-endian u64 words:
 *   word 0       magic, "FABAB\x01\0\0"
 *   word 1..     the scalar counters, in the order of enum word
 *   bytes 1024   the process's arguments after the program, 128 bytes
 *   bytes 2048   the programs started, 32 slots of a u64 count and 56
 *                bytes of name
 */

#include <crt_externs.h>
#include <fcntl.h>
#include <spawn.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/param.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <time.h>
#include <unistd.h>

#define PAGE_BYTES 4096
#define ROLE_OFFSET 1024
#define ROLE_BYTES 128
#define PROGRAMS_OFFSET 2048
#define PROGRAM_SLOTS 32
#define PROGRAM_NAME 56

/* The classes of file a read, write or flush touches. */
enum class {
    C_JOURNAL,  /* <state>/managed/journal.sqlite and its -wal, -shm */
    C_LEDGER,   /* <state>/jobs/<id>/ledger and its temporaries */
    C_JOBS,     /* every other file under <state>/jobs */
    C_STORE,    /* <state>/store, its catalog and objects */
    C_STATE,    /* every other file under <state> */
    C_OUTSIDE,  /* a file outside <state>: workspaces, the home, temp */
    C_IPC,      /* not a file: pipes, sockets, terminals */
    CLASSES
};

/* The scalar words, after the magic. */
enum word {
    W_MAGIC,
    W_PID,
    W_EXITED,
    W_MAXRSS,
    W_CPU_US,
    W_SPAWNS,
    W_EXECS,
    W_FSYNC,
    W_FULLFSYNC,
    W_BARRIERFSYNC,
    W_OPENS,
    W_RENAMES,
    W_UNLINKS,
    W_MALLOCS,
    W_MALLOC_BYTES,
    W_FREES,
    W_LOCK,
    W_WRITE_BYTES,
    W_WRITE_CALLS = W_WRITE_BYTES + CLASSES,
    W_READ_BYTES = W_WRITE_CALLS + CLASSES,
    W_READ_CALLS = W_READ_BYTES + CLASSES,
    W_SYNCS = W_READ_CALLS + CLASSES,
    W_END = W_SYNCS + CLASSES
};

_Static_assert(W_END * 8 <= ROLE_OFFSET, "the counters overrun the role");

static volatile uint64_t *page;
static int spawn_log = -1;
static char state[MAXPATHLEN];
static size_t state_len;

static void add(int word, uint64_t n) {
    if (page != NULL) {
        __atomic_fetch_add(&page[word], n, __ATOMIC_RELAXED);
    }
}

/* Notes the process's CPU time and peak resident size so far. */
static void note_usage(void) {
    struct rusage usage;
    if (page == NULL || getrusage(RUSAGE_SELF, &usage) != 0) {
        return;
    }
    uint64_t cpu = (uint64_t)usage.ru_utime.tv_sec * 1000000 + (uint64_t)usage.ru_utime.tv_usec
                   + (uint64_t)usage.ru_stime.tv_sec * 1000000 + (uint64_t)usage.ru_stime.tv_usec;
    /* ru_maxrss is in bytes on macOS. */
    __atomic_store_n(&page[W_MAXRSS], (uint64_t)usage.ru_maxrss, __ATOMIC_RELAXED);
    __atomic_store_n(&page[W_CPU_US], cpu, __ATOMIC_RELAXED);
}

static int starts_with(const char *s, const char *prefix) {
    return strncmp(s, prefix, strlen(prefix)) == 0;
}

/* The class of the file `fd` names. */
static int classify(int fd) {
    struct stat st;
    char path[MAXPATHLEN];
    if (fstat(fd, &st) != 0) {
        return C_OUTSIDE;
    }
    if (!S_ISREG(st.st_mode) && !S_ISDIR(st.st_mode)) {
        return C_IPC;
    }
    if (state_len == 0 || fcntl(fd, F_GETPATH, path) == -1
        || strncmp(path, state, state_len) != 0) {
        return C_OUTSIDE;
    }
    const char *rest = path + state_len;
    if (starts_with(rest, "managed/journal.sqlite")) {
        return C_JOURNAL;
    }
    if (starts_with(rest, "jobs/")) {
        const char *base = strrchr(rest, '/');
        return base != NULL && starts_with(base + 1, "ledger") ? C_LEDGER : C_JOBS;
    }
    if (starts_with(rest, "store/") || strcmp(rest, "store") == 0) {
        return C_STORE;
    }
    return C_STATE;
}

static void count_io(int base_word, int calls_word, int fd, ssize_t n) {
    if (page == NULL) {
        return;
    }
    int class = classify(fd);
    add(calls_word + class, 1);
    if (n > 0) {
        add(base_word + class, (uint64_t)n);
    }
}

static void lock(void) {
    /* The page is shared with a forked child, so this spins on the page. */
    for (long tries = 0; tries < 100000000; tries++) {
        if (__atomic_exchange_n(&page[W_LOCK], 1, __ATOMIC_ACQUIRE) == 0) {
            return;
        }
    }
}

static void unlock(void) {
    __atomic_store_n(&page[W_LOCK], 0, __ATOMIC_RELEASE);
}

/* Notes one start of the program at `path` with `argv` in the spawn log. */
static void log_spawn(const char *name, char *const argv[]) {
    char line[1024];
    size_t used = 0;
    int wrote = snprintf(line, sizeof line - 1, "%s", name);
    used = wrote > 0 ? (size_t)wrote : 0;
    for (int i = 1; argv != NULL && argv[i] != NULL && used < sizeof line - 2; i++) {
        wrote = snprintf(line + used, sizeof line - 1 - used, " %s", argv[i]);
        if (wrote < 0) {
            break;
        }
        used += (size_t)wrote;
    }
    if (used > sizeof line - 2) {
        used = sizeof line - 2;
    }
    for (size_t i = 0; i < used; i++) {
        if (line[i] == '\n' || line[i] == '\t') {
            line[i] = ' ';
        }
    }
    line[used] = '\n';
    (void)!write(spawn_log, line, used + 1);
}

/* Counts one start of the program at `path`, by its file name. */
static void count_program(const char *path, char *const argv[]) {
    if (page == NULL || path == NULL) {
        return;
    }
    const char *slash = strrchr(path, '/');
    const char *name = slash != NULL ? slash + 1 : path;
    if (spawn_log >= 0) {
        log_spawn(name, argv);
    }
    volatile unsigned char *slots = (volatile unsigned char *)page + PROGRAMS_OFFSET;
    lock();
    for (int i = 0; i < PROGRAM_SLOTS; i++) {
        volatile unsigned char *slot = slots + i * (8 + PROGRAM_NAME);
        char *slot_name = (char *)(slot + 8);
        if (slot_name[0] == '\0') {
            strncpy(slot_name, name, PROGRAM_NAME - 1);
        }
        if (strncmp(slot_name, name, PROGRAM_NAME - 1) == 0) {
            __atomic_fetch_add((volatile uint64_t *)slot, 1, __ATOMIC_RELAXED);
            break;
        }
    }
    unlock();
}

__attribute__((constructor)) static void start(void) {
    const char *dir = getenv("FABRIC_AB_COUNTS");
    if (dir == NULL || strcmp(getprogname(), "fabric") != 0) {
        return;
    }
    const char *home = getenv("FABRIC_HOME");
    if (home != NULL && strlen(home) + 2 < sizeof state) {
        snprintf(state, sizeof state, "%s/", home);
        state_len = strlen(state);
    }
    struct timespec now;
    clock_gettime(CLOCK_REALTIME, &now);
    char path[MAXPATHLEN];
    snprintf(path, sizeof path, "%s/%d.%ld%09ld.spawns", dir, getpid(), (long)now.tv_sec,
             now.tv_nsec);
    spawn_log = open(path, O_WRONLY | O_CREAT | O_EXCL | O_APPEND | O_CLOEXEC, 0600);
    snprintf(path, sizeof path, "%s/%d.%ld%09ld.page", dir, getpid(), (long)now.tv_sec, now.tv_nsec);
    int fd = open(path, O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (fd < 0) {
        return;
    }
    if (ftruncate(fd, PAGE_BYTES) != 0) {
        close(fd);
        return;
    }
    void *mapped = mmap(NULL, PAGE_BYTES, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    close(fd);
    if (mapped == MAP_FAILED) {
        return;
    }
    volatile uint64_t *words = mapped;
    char *role = (char *)mapped + ROLE_OFFSET;
    int argc = *_NSGetArgc();
    char **argv = *_NSGetArgv();
    size_t used = 0;
    for (int i = 1; i < argc && used + 1 < ROLE_BYTES; i++) {
        int wrote = snprintf(role + used, ROLE_BYTES - used, "%s%s", i > 1 ? " " : "", argv[i]);
        if (wrote < 0) {
            break;
        }
        used += (size_t)wrote;
    }
    words[W_PID] = (uint64_t)getpid();
    memcpy((void *)words, "FABAB\x01\0\0", 8);
    page = words;
    note_usage();
}

__attribute__((destructor)) static void stop(void) {
    note_usage();
    if (page != NULL) {
        __atomic_store_n(&page[W_EXITED], 1, __ATOMIC_RELEASE);
    }
}

static int my_open(const char *path, int flags, ...) {
    mode_t mode = 0;
    if (flags & O_CREAT) {
        va_list ap;
        va_start(ap, flags);
        mode = (mode_t)va_arg(ap, int);
        va_end(ap);
    }
    add(W_OPENS, 1);
    return open(path, flags, mode);
}

static int my_openat(int dirfd, const char *path, int flags, ...) {
    mode_t mode = 0;
    if (flags & O_CREAT) {
        va_list ap;
        va_start(ap, flags);
        mode = (mode_t)va_arg(ap, int);
        va_end(ap);
    }
    add(W_OPENS, 1);
    return openat(dirfd, path, flags, mode);
}

static int my_fcntl(int fd, int cmd, ...) {
    va_list ap;
    va_start(ap, cmd);
    void *arg = va_arg(ap, void *);
    va_end(ap);
    if (cmd == F_FULLFSYNC || cmd == F_BARRIERFSYNC) {
        add(cmd == F_FULLFSYNC ? W_FULLFSYNC : W_BARRIERFSYNC, 1);
        if (page != NULL) {
            add(W_SYNCS + classify(fd), 1);
        }
        note_usage();
    }
    return fcntl(fd, cmd, arg);
}

static int my_fsync(int fd) {
    add(W_FSYNC, 1);
    if (page != NULL) {
        add(W_SYNCS + classify(fd), 1);
    }
    note_usage();
    return fsync(fd);
}

static ssize_t my_write(int fd, const void *buf, size_t n) {
    ssize_t done = write(fd, buf, n);
    count_io(W_WRITE_BYTES, W_WRITE_CALLS, fd, done);
    return done;
}

static ssize_t my_pwrite(int fd, const void *buf, size_t n, off_t at) {
    ssize_t done = pwrite(fd, buf, n, at);
    count_io(W_WRITE_BYTES, W_WRITE_CALLS, fd, done);
    return done;
}

static ssize_t my_writev(int fd, const struct iovec *iov, int count) {
    ssize_t done = writev(fd, iov, count);
    count_io(W_WRITE_BYTES, W_WRITE_CALLS, fd, done);
    return done;
}

static ssize_t my_read(int fd, void *buf, size_t n) {
    ssize_t done = read(fd, buf, n);
    count_io(W_READ_BYTES, W_READ_CALLS, fd, done);
    return done;
}

static ssize_t my_pread(int fd, void *buf, size_t n, off_t at) {
    ssize_t done = pread(fd, buf, n, at);
    count_io(W_READ_BYTES, W_READ_CALLS, fd, done);
    return done;
}

static ssize_t my_readv(int fd, const struct iovec *iov, int count) {
    ssize_t done = readv(fd, iov, count);
    count_io(W_READ_BYTES, W_READ_CALLS, fd, done);
    return done;
}

static int my_rename(const char *from, const char *to) {
    add(W_RENAMES, 1);
    return rename(from, to);
}

static int my_renameat(int from_dir, const char *from, int to_dir, const char *to) {
    add(W_RENAMES, 1);
    return renameat(from_dir, from, to_dir, to);
}

static int my_unlink(const char *path) {
    add(W_UNLINKS, 1);
    return unlink(path);
}

static int my_unlinkat(int dir, const char *path, int flags) {
    add(W_UNLINKS, 1);
    return unlinkat(dir, path, flags);
}

/*
 * posix_spawnp tries posix_spawn in each PATH directory, and execvp and
 * execv call execve, through calls this library also replaces. Only the
 * outermost call of a thread counts, and a spawn counts once it succeeded.
 */
static __thread int nested;

static int my_posix_spawn(pid_t *pid, const char *path, const posix_spawn_file_actions_t *actions,
                          const posix_spawnattr_t *attr, char *const argv[], char *const envp[]) {
    int outer = nested++ == 0;
    int failed = posix_spawn(pid, path, actions, attr, argv, envp);
    nested--;
    if (outer && failed == 0) {
        add(W_SPAWNS, 1);
        count_program(path, argv);
        note_usage();
    }
    return failed;
}

static int my_posix_spawnp(pid_t *pid, const char *file, const posix_spawn_file_actions_t *actions,
                           const posix_spawnattr_t *attr, char *const argv[], char *const envp[]) {
    int outer = nested++ == 0;
    int failed = posix_spawnp(pid, file, actions, attr, argv, envp);
    nested--;
    if (outer && failed == 0) {
        add(W_SPAWNS, 1);
        count_program(file, argv);
        note_usage();
    }
    return failed;
}

static pid_t my_fork(void) {
    pid_t pid = fork();
    if (pid > 0) {
        add(W_SPAWNS, 1);
        note_usage();
    }
    return pid;
}

/*
 * A forked child counts the program it runs on the parent's page, before
 * the call, since an exec that works does not return.
 */
static int my_execve(const char *path, char *const argv[], char *const envp[]) {
    if (nested == 0) {
        add(W_EXECS, 1);
        count_program(path, argv);
    }
    return execve(path, argv, envp);
}

static int my_execvp(const char *file, char *const argv[]) {
    if (nested++ == 0) {
        add(W_EXECS, 1);
        count_program(file, argv);
    }
    int failed = execvp(file, argv);
    nested--;
    return failed;
}

static int my_execv(const char *path, char *const argv[]) {
    if (nested++ == 0) {
        add(W_EXECS, 1);
        count_program(path, argv);
    }
    int failed = execv(path, argv);
    nested--;
    return failed;
}

static void *my_malloc(size_t n) {
    add(W_MALLOCS, 1);
    add(W_MALLOC_BYTES, n);
    return malloc(n);
}

static void *my_calloc(size_t count, size_t n) {
    add(W_MALLOCS, 1);
    add(W_MALLOC_BYTES, count * n);
    return calloc(count, n);
}

static void *my_realloc(void *old, size_t n) {
    add(W_MALLOCS, 1);
    add(W_MALLOC_BYTES, n);
    return realloc(old, n);
}

static int my_posix_memalign(void **out, size_t align, size_t n) {
    add(W_MALLOCS, 1);
    add(W_MALLOC_BYTES, n);
    return posix_memalign(out, align, n);
}

static void my_free(void *p) {
    if (p != NULL) {
        add(W_FREES, 1);
    }
    free(p);
}

#define INTERPOSE(replacement, replacee)                                               \
    __attribute__((used)) static const struct {                                        \
        const void *with;                                                              \
        const void *what;                                                              \
    } interpose_##replacee __attribute__((section("__DATA,__interpose"))) = {          \
        (const void *)(unsigned long)&replacement, (const void *)(unsigned long)&replacee \
    }

INTERPOSE(my_open, open);
INTERPOSE(my_openat, openat);
INTERPOSE(my_fcntl, fcntl);
INTERPOSE(my_fsync, fsync);
INTERPOSE(my_write, write);
INTERPOSE(my_pwrite, pwrite);
INTERPOSE(my_writev, writev);
INTERPOSE(my_read, read);
INTERPOSE(my_pread, pread);
INTERPOSE(my_readv, readv);
INTERPOSE(my_rename, rename);
INTERPOSE(my_renameat, renameat);
INTERPOSE(my_unlink, unlink);
INTERPOSE(my_unlinkat, unlinkat);
INTERPOSE(my_posix_spawn, posix_spawn);
INTERPOSE(my_posix_spawnp, posix_spawnp);
INTERPOSE(my_fork, fork);
INTERPOSE(my_execve, execve);
INTERPOSE(my_execvp, execvp);
INTERPOSE(my_execv, execv);
INTERPOSE(my_malloc, malloc);
INTERPOSE(my_calloc, calloc);
INTERPOSE(my_realloc, realloc);
INTERPOSE(my_posix_memalign, posix_memalign);
INTERPOSE(my_free, free);
