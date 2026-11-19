#include "owned_socket_path.hpp"

#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <iostream>
#include <string>
#include <sys/socket.h>
#include <sys/un.h>

namespace {
using cua::hyprland::OwnedSocketPath;
using Result = OwnedSocketPath::CleanupResult;

void check(bool condition, const char* message) {
    if (!condition) {
        std::cerr << "FAIL: " << message << '\n';
        std::exit(1);
    }
}

class TempDirectory {
  public:
    TempDirectory() {
        char pattern[] = "/tmp/cua-owned-socket-XXXXXX";
        const char* created = mkdtemp(pattern);
        check(created != nullptr, "create temporary directory");
        path = created;
    }
    ~TempDirectory() { check(rmdir(path.c_str()) == 0, "remove empty test directory"); }
    std::string path;
};

class BoundSocket {
  public:
    explicit BoundSocket(const std::string& path) {
        fd = socket(AF_UNIX, SOCK_STREAM, 0);
        check(fd >= 0, "create Unix socket");
        sockaddr_un address{};
        address.sun_family = AF_UNIX;
        check(path.size() < sizeof(address.sun_path), "socket path fits");
        std::memcpy(address.sun_path, path.c_str(), path.size() + 1);
        check(bind(fd, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0,
              "bind Unix socket");
    }
    ~BoundSocket() { check(close(fd) == 0, "close test socket"); }
    int fd = -1;
};

struct stat metadata(const std::string& path) {
    struct stat result{};
    check(lstat(path.c_str(), &result) == 0, "inspect test path without following symlinks");
    return result;
}

void remove_path(const std::string& path) {
    check(unlink(path.c_str()) == 0, "remove exact test fixture path");
}

void create_file(const std::string& path) {
    const int fd = open(path.c_str(), O_CREAT | O_EXCL | O_WRONLY, 0600);
    check(fd >= 0, "create regular-file fixture");
    check(close(fd) == 0, "close regular-file fixture");
}

void check_absent(const std::string& path) {
    struct stat value{};
    check(lstat(path.c_str(), &value) != 0 && errno == ENOENT, "socket path is absent");
}

void test_cleanup() {
    TempDirectory directory;
    const auto path = directory.path + "/socket";
    OwnedSocketPath owner;
    check(owner.cleanup() == Result::NotCaptured, "uncaptured cleanup is inert");
    BoundSocket socket(path);
    check(owner.capture(path), "capture bound socket");
    check(owner.cleanup() == Result::Removed, "remove matching socket");
    check_absent(path);
    check(owner.error_number() == 0, "successful cleanup has no error");
    check(owner.cleanup() == Result::NotCaptured, "cleanup is idempotent");
}

void test_missing_path() {
    TempDirectory directory;
    const auto path = directory.path + "/socket";
    OwnedSocketPath owner;
    check(!owner.capture(path) && owner.error_number() == ENOENT,
          "missing capture reports its error");
    BoundSocket socket(path);
    check(owner.cleanup() == Result::NotCaptured, "failed capture cannot remove a later socket");
    check(S_ISSOCK(metadata(path).st_mode), "later socket survives failed capture");
    check(owner.capture(path), "capture existing bound fixture");
    remove_path(path);
    check(owner.cleanup() == Result::AlreadyAbsent, "removed path is explicitly reported");
    check(owner.cleanup() == Result::NotCaptured, "absent cleanup consumes identity");
}

void test_replaced_socket() {
    TempDirectory directory;
    const auto path = directory.path + "/socket";
    const auto moved = directory.path + "/original";
    BoundSocket original(path);
    OwnedSocketPath owner;
    check(owner.capture(path), "capture original socket");
    // Keep the original inode alive to avoid filesystem inode reuse in this test.
    check(rename(path.c_str(), moved.c_str()) == 0, "move original socket");
    BoundSocket replacement(path);
    const auto before = metadata(path);
    check(owner.cleanup() == Result::Replaced, "refuse replacement socket");
    check(metadata(path).st_ino == before.st_ino, "replacement socket survives");
    check(owner.cleanup() == Result::NotCaptured, "refusal consumes identity");
    remove_path(path);
    remove_path(moved);
}

void test_replaced_file_and_symlink() {
    for (bool symlink_fixture : {false, true}) {
        TempDirectory directory;
        const auto path = directory.path + "/socket";
        const auto moved = directory.path + "/original";
        BoundSocket original(path);
        OwnedSocketPath owner;
        check(owner.capture(path), "capture socket before replacement");
        check(rename(path.c_str(), moved.c_str()) == 0, "move socket before replacement");
        if (symlink_fixture)
            check(symlink(moved.c_str(), path.c_str()) == 0, "create socket-targeting symlink");
        else
            create_file(path);
        check(owner.cleanup() == Result::Replaced, "refuse non-socket replacement");
        const auto current = metadata(path);
        check(symlink_fixture ? S_ISLNK(current.st_mode) : S_ISREG(current.st_mode),
              "replacement object survives");
        check(!owner.capture(path) && owner.error_number() == EPERM,
              "capture rejects symlink or regular file");
        check(owner.cleanup() == Result::NotCaptured, "invalid capture fails closed");
        check(S_ISSOCK(metadata(moved).st_mode), "original socket survives symlink inspection");
        remove_path(path);
        remove_path(moved);
    }
}

void test_metadata_identity() {
    TempDirectory directory;
    const auto path = directory.path + "/socket";
    BoundSocket socket(path);
    const auto captured = metadata(path);
    namespace detail = cua::hyprland::owned_socket_path_detail;
    check(detail::matches_identity(captured, captured, getuid()), "same owned identity matches");
    auto changed = captured;
    changed.st_uid = captured.st_uid == 0 ? 1 : 0;
    check(!detail::is_owned_socket(changed, getuid()), "capture predicate refuses foreign UID");
    check(!detail::matches_identity(changed, captured, getuid()), "cleanup refuses foreign UID");
    changed = captured;
    changed.st_dev = captured.st_dev == 0 ? 1 : 0;
    check(!detail::matches_identity(changed, captured, getuid()), "device mismatch refuses cleanup");
    changed = captured;
    changed.st_ino = captured.st_ino == 0 ? 1 : 0;
    check(!detail::matches_identity(changed, captured, getuid()), "inode mismatch refuses cleanup");
    changed = captured;
    changed.st_mode = S_IFREG | 0600;
    check(!detail::matches_identity(changed, captured, getuid()), "type mismatch refuses cleanup");
    remove_path(path);
}

void test_inspection_failure() {
    TempDirectory directory;
    const auto parent = directory.path + "/parent";
    const auto moved = directory.path + "/moved";
    const auto path = parent + "/socket";
    check(mkdir(parent.c_str(), 0700) == 0, "create socket parent");
    BoundSocket socket(path);
    OwnedSocketPath owner;
    check(owner.capture(path), "capture before path becomes inaccessible");
    check(rename(parent.c_str(), moved.c_str()) == 0, "move socket parent");
    create_file(parent);
    check(owner.cleanup() == Result::Failed && owner.error_number() == ENOTDIR,
          "inspection failure is explicit and retains errno");
    check(owner.cleanup() == Result::NotCaptured, "failed cleanup consumes identity");
    check(S_ISSOCK(metadata(moved + "/socket").st_mode), "inspection failure preserves socket");
    remove_path(parent);
    remove_path(moved + "/socket");
    check(rmdir(moved.c_str()) == 0, "remove moved empty parent");
}

void test_unlink_failure() {
    if (geteuid() == 0) {
        std::cout << "SKIP: directory permissions cannot force unlink failure as root\n";
        return;
    }
    TempDirectory directory;
    const auto path = directory.path + "/socket";
    BoundSocket socket(path);
    OwnedSocketPath owner;
    check(owner.capture(path), "capture before unlink permission failure");
    check(chmod(directory.path.c_str(), 0500) == 0, "remove directory write permission");
    const auto result = owner.cleanup();
    const int error = owner.error_number();
    check(chmod(directory.path.c_str(), 0700) == 0, "restore test directory permissions");
    check(result == Result::Failed && (error == EACCES || error == EPERM),
          "unlink failure is explicit and retains errno");
    check(S_ISSOCK(metadata(path).st_mode), "failed unlink preserves socket");
    check(owner.cleanup() == Result::NotCaptured, "failed unlink consumes identity");
    remove_path(path);
}
} // namespace

int main() {
    test_cleanup();
    test_missing_path();
    test_replaced_socket();
    test_replaced_file_and_symlink();
    test_metadata_identity();
    test_inspection_failure();
    test_unlink_failure();
    std::cout << "owned socket path tests passed\n";
}
