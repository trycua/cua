// arc-launch: the executable of ArcDriverBench.app (bundle id com.trycua.bench.arcdriver), Amendment 9.
//
// macOS grants Accessibility and Screen Recording to the *responsible* process. For a stdio MCP server started by
// claude under Terminal.app, that is Terminal. This launcher makes itself the responsible process instead, so the
// grants can go to this small app and to nothing else:
//
//   1. claude starts  arc-launch <program> <args...>                              (responsible: Terminal)
//   2. it re-spawns itself with responsibility disclaimed                         (responsible: arc-launch itself)
//   3. that copy spawns <program> <args...> normally and waits for it              (responsible: arc-launch, step 2)
//
// stdin, stdout and stderr are inherited unchanged, so the MCP stream passes straight through. Each launcher waits
// for its child and exits with the child's status. SIGTERM, SIGINT and SIGHUP are passed on to the child once each;
// the children stay in claude's process group, so a group kill reaches all three processes.
//
// Build (in the VM): clang -O2 -Wall -o arc-launch arc_launch.c

#include <errno.h>
#include <mach-o/dyld.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;

// Exported by libsystem (libquarantine); declared here because no public header has it.
int responsibility_spawnattrs_setdisclaim(posix_spawnattr_t *attrs, int disclaim);

#define STAGE_ENV "ARC_LAUNCH_STAGE"

static volatile pid_t child_pid = 0;
static volatile sig_atomic_t forwarded[32];

static void forward(int sig) {
    if (child_pid > 0 && sig > 0 && sig < 32 && !forwarded[sig]) {
        forwarded[sig] = 1;
        kill(child_pid, sig);
    }
}

static int wait_child(pid_t pid) {
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

static void install_forwarding(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = forward;
    sigemptyset(&sa.sa_mask);
    // a caught signal is reset to its default in the spawned program, which installs its own handlers
    sigaction(SIGTERM, &sa, NULL);
    sigaction(SIGINT, &sa, NULL);
    sigaction(SIGHUP, &sa, NULL);
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: arc-launch <program> [args...]\n");
        return 64;
    }
    posix_spawnattr_t attr;
    if (posix_spawnattr_init(&attr) != 0) return 71;
    pid_t pid = 0;
    int rc;
    const char *stage = getenv(STAGE_ENV);
    install_forwarding();
    if (stage == NULL || strcmp(stage, "disclaimed") != 0) {
        // Stage 1: start this same executable again as its own responsible process.
        char self[4096];
        uint32_t size = sizeof self;
        if (_NSGetExecutablePath(self, &size) != 0) {
            fprintf(stderr, "arc-launch: executable path too long\n");
            return 70;
        }
        if (responsibility_spawnattrs_setdisclaim(&attr, 1) != 0) {
            fprintf(stderr, "arc-launch: cannot disclaim responsibility\n");
            return 70;
        }
        setenv(STAGE_ENV, "disclaimed", 1);
        argv[0] = self;
        rc = posix_spawn(&pid, self, NULL, &attr, argv, environ);
    } else {
        // Stage 2: run the program as a normal child, so this app stays responsible for it.
        unsetenv(STAGE_ENV);
        rc = posix_spawn(&pid, argv[1], NULL, &attr, argv + 1, environ);
    }
    posix_spawnattr_destroy(&attr);
    if (rc != 0) {
        fprintf(stderr, "arc-launch: spawn failed: %s\n", strerror(rc));
        return 127;
    }
    child_pid = pid;
    return wait_child(pid);
}
