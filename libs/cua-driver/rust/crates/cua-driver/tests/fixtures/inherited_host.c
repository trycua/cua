// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <errno.h>

int main(int argc, char **argv) {
    if (argc < 3) return 64;
    int pair[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, pair)) return 70;
    pid_t child = fork();
    if (child < 0) return 70;
    if (!child) {
        if (dup2(pair[1], 3) < 0) _exit(70);
        if (pair[0] != 3) close(pair[0]);
        if (pair[1] != 3) close(pair[1]);
        fcntl(3, F_SETFD, 0);
        int null = open("/dev/null", O_RDWR);
        dup2(null, STDIN_FILENO);
        dup2(null, STDOUT_FILENO);
        if (null > 3) close(null);
        execv(argv[1], argv + 2);
        _exit(70);
    }
    close(pair[1]);
    printf("%d\n", child);
    fflush(stdout);
    struct pollfd fds[2] = {{STDIN_FILENO, POLLIN, 0}, {pair[0], POLLIN, 0}};
    char buffer[8192];
    signal(SIGPIPE, SIG_IGN);
    for (;;) {
        if (poll(fds, 2, -1) < 0) {
            if (errno == EINTR) continue;
            kill(child, SIGKILL);
            break;
        }
        int done = 0;
        for (int i = 0; i < 2; ++i) {
            if (!fds[i].revents) continue;
            ssize_t count = read(fds[i].fd, buffer, sizeof buffer);
            if (count <= 0) { done = 1; break; }
            int dest = i == 0 ? pair[0] : STDOUT_FILENO;
            ssize_t sent = 0;
            while (sent < count) {
                ssize_t n = write(dest, buffer + sent, (size_t)(count - sent));
                if (n <= 0) { done = 1; break; }
                sent += n;
            }
        }
        if (done) break;
    }
    close(pair[0]);
    int status;
    if (waitpid(child, &status, 0) < 0) return 70;
    return WIFEXITED(status) ? WEXITSTATUS(status) : 70;
}
