#pragma once

#include <cerrno>
#include <string>
#include <sys/stat.h>
#include <unistd.h>

namespace cua::hyprland {
namespace owned_socket_path_detail {

inline bool is_owned_socket(const struct stat& metadata, uid_t owner) noexcept {
    return S_ISSOCK(metadata.st_mode) && metadata.st_uid == owner;
}

inline bool matches_identity(const struct stat& current,
                             const struct stat& captured, uid_t owner) noexcept {
    return is_owned_socket(current, owner) &&
           current.st_uid == captured.st_uid &&
           current.st_dev == captured.st_dev && current.st_ino == captured.st_ino;
}

} // namespace owned_socket_path_detail

// Call capture immediately after a successful bind, in a private runtime
// directory. Pathname inspection and unlink are not atomic: this guards normal
// lifecycle replacement, not hostile races by another process with the same UID.
// Cleanup is explicit so its outcome can be reported by the listener's owner.
class OwnedSocketPath {
  public:
    enum class CleanupResult { NotCaptured, Removed, AlreadyAbsent, Replaced, Failed };

    OwnedSocketPath() = default;
    OwnedSocketPath(const OwnedSocketPath&) = delete;
    OwnedSocketPath& operator=(const OwnedSocketPath&) = delete;

    bool capture(const std::string& path) {
        m_captured = false;
        m_error = 0;
        m_path = path;
        if (lstat(m_path.c_str(), &m_metadata) != 0) {
            m_error = errno;
            return false;
        }
        if (!owned_socket_path_detail::is_owned_socket(m_metadata, getuid())) {
            m_error = EPERM;
            return false;
        }
        m_captured = true;
        return true;
    }

    // Every attempt consumes ownership, including a failed or refused cleanup.
    // A later retire/destructor must not retry against a newly installed path.
    CleanupResult cleanup() noexcept {
        m_error = 0;
        if (!m_captured)
            return CleanupResult::NotCaptured;
        m_captured = false;

        struct stat current{};
        if (lstat(m_path.c_str(), &current) != 0) {
            const int error = errno;
            if (error == ENOENT)
                return CleanupResult::AlreadyAbsent;
            m_error = error;
            return CleanupResult::Failed;
        }
        if (!owned_socket_path_detail::matches_identity(current, m_metadata, getuid()))
            return CleanupResult::Replaced;
        if (unlink(m_path.c_str()) != 0) {
            const int error = errno;
            if (error == ENOENT)
                return CleanupResult::AlreadyAbsent;
            m_error = error;
            return CleanupResult::Failed;
        }
        return CleanupResult::Removed;
    }

    int error_number() const noexcept { return m_error; }

  private:
    std::string m_path;
    struct stat m_metadata{};
    bool m_captured = false;
    int m_error = 0;
};

} // namespace cua::hyprland
