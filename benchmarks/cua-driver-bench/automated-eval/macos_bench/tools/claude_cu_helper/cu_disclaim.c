// cu-disclaim: start Claude Desktop's app-cu-helper as its own responsible process (arm cc-claude-cu-helper,
// Amendment 11).
//
// macOS checks Accessibility against the *responsible* process. Under the bench runner the adapter's children
// would be attributed to Terminal.app. Inside Claude Desktop the helper runs under Claude.app, which holds the
// grants. Here the helper is started with responsibility disclaimed, so macOS checks the helper's own code identity
// (identifier app-cu-helper, Anthropic team Q6L2SF6YDW), and the Accessibility grant is given to that binary in
// System Settings, through the normal UI. Nothing of ours gets a grant, and Terminal's grants are not widened.
//
//   cu-disclaim <program> [args...]
//
// stdin, stdout and stderr are inherited unchanged (the helper's JSON-RPC passes straight through). The launcher
// waits for the child, passes SIGTERM/SIGINT/SIGHUP on once, and exits with the child's status. The child stays in
// the caller's process group, so a group kill reaches it.
//
// Build (in the VM): clang -O2 -Wall -o cu-disclaim cu_disclaim.c

#include <errno.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

// Exported by libsystem; declared here because no public header has it (the same call arc-driver's launcher uses).
int responsibility_spawnattrs_setdisclaim(posix_spawnattr_t *attrs, int disclaim);

static volatile pid_t child_pid = 0;
static volatile sig_atomic_t forwarded[32];

static void forward(int sig) {
    if (child_pid > 0 && sig > 0 && sig < 32 && !forwarded[sig]) {
        forwarded[sig] = 1;
        kill(child_pid, sig);
    }
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: cu-disclaim <program> [args...]\n");
        return 64;
    }
    posix_spawnattr_t attrs;
    posix_spawnattr_init(&attrs);
    if (responsibility_spawnattrs_setdisclaim(&attrs, 1) != 0) {
        fprintf(stderr, "cu-disclaim: responsibility_spawnattrs_setdisclaim failed\n");
        return 70;
    }
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = forward;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGTERM, &sa, NULL);
    sigaction(SIGINT, &sa, NULL);
    sigaction(SIGHUP, &sa, NULL);
    pid_t pid;
    int rc = posix_spawn(&pid, argv[1], NULL, &attrs, argv + 1, environ);
    posix_spawnattr_destroy(&attrs);
    if (rc != 0) {
        fprintf(stderr, "cu-disclaim: spawn %s: %s\n", argv[1], strerror(rc));
        return 127;
    }
    child_pid = pid;
    int status = 0;
    for (;;) {
        pid_t done = waitpid(pid, &status, 0);
        if (done == pid) break;
        if (done < 0 && errno != EINTR) return 127;
    }
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return 1;
}
