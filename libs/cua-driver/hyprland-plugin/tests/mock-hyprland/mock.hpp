#pragma once

#include <cstdint>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <typeinfo>
#include <unordered_map>
#include <utility>
#include <variant>
#include <vector>

#define GIT_COMMIT_HASH "mock-hyprland-hash"
#define AQUAMARINE_VERSION "1.2.3"
#define HYPRUTILS_VERSION "2.3.4"
#define HYPRGRAPHICS_VERSION "3.4.5"
#define HYPRCURSOR_VERSION "4.5.6"
#define HYPRLANG_VERSION "5.6.7"
#define HYPRLAND_API_VERSION "0.1"
#define APICALL extern "C"
#define EXPORT __attribute__((visibility("default")))
#define HANDLE void*
#define PLUGIN_API_VERSION pluginAPIVersion
#define PLUGIN_INIT pluginInit
#define PLUGIN_EXIT pluginExit

APICALL inline EXPORT const char* __hyprland_api_get_client_hash() {
    return "mock-hyprland-hash_aq_1.2_hu_2.3_hg_3.4_hc_4.5_hlg_5.6";
}

APICALL EXPORT const char* __hyprland_api_get_hash();

template <typename T>
using SP = std::shared_ptr<T>;

template <typename T, typename... Args>
SP<T> makeShared(Args&&... args) {
    return std::make_shared<T>(std::forward<Args>(args)...);
}

struct PLUGIN_DESCRIPTION_INFO {
    std::string name;
    std::string description;
    std::string author;
    std::string version;
};

struct SVersionInfo {
    std::string hash = GIT_COMMIT_HASH;
    std::string tag;
    bool dirty = false;
    std::string branch;
    std::string message;
    std::string commits;
};

enum eHyprCtlOutputFormat : std::uint8_t {
    FORMAT_NORMAL = 0,
    FORMAT_JSON,
};

struct SHyprCtlCommand {
    std::string name;
    bool exact = true;
    std::function<std::string(eHyprCtlOutputFormat, std::string)> fn;
};

struct CHyprSignalListener {
    CHyprSignalListener() = default;
    explicit CHyprSignalListener(std::function<void()> reset_callback)
        : m_reset_callback(std::move(reset_callback)) {}
    ~CHyprSignalListener() { reset(); }

    CHyprSignalListener(const CHyprSignalListener&) = delete;
    CHyprSignalListener& operator=(const CHyprSignalListener&) = delete;

    CHyprSignalListener(CHyprSignalListener&& other) noexcept
        : m_reset_callback(std::exchange(other.m_reset_callback, {})) {}

    CHyprSignalListener& operator=(CHyprSignalListener&& other) noexcept {
        if (this == &other)
            return *this;
        reset();
        m_reset_callback = std::exchange(other.m_reset_callback, {});
        return *this;
    }

    void reset() {
        if (!m_reset_callback)
            return;
        auto reset_callback = std::move(m_reset_callback);
        reset_callback();
    }

  private:
    std::function<void()> m_reset_callback;
};

namespace Supplementary {

using PropRefreshBits = std::uint64_t;

} // namespace Supplementary

namespace Config {

using BOOL = bool;

namespace Values {

inline std::optional<Config::BOOL> configured_bool_override;

struct SBoolValueOptions {
    Supplementary::PropRefreshBits refresh = 0;
    const char* deprecationNotice = nullptr;
};

class IValue {
  protected:
    IValue(Supplementary::PropRefreshBits, const char* = nullptr) {}

  public:
    virtual ~IValue() = default;
    virtual const std::type_info* underlying() const = 0;
    virtual void commence() = 0;
};

class CBoolValue : public IValue {
  public:
    CBoolValue(const char*, const char*, Config::BOOL value,
               SBoolValueOptions&& options = {})
        : IValue(options.refresh, options.deprecationNotice),
          m_value(configured_bool_override.value_or(value)) {}
    const std::type_info* underlying() const override { return &typeid(Config::BOOL); }
    void commence() override {}
    Config::BOOL value() const { return m_value; }

  private:
    Config::BOOL m_value;
};

} // namespace Values
} // namespace Config

namespace IPC::Socket1 {

enum eOutputFormat : std::uint8_t {
    FORMAT_NORMAL = 0,
    FORMAT_JSON,
};

enum eCommandMatch : std::uint8_t {
    COMMAND_MATCH_EXACT = 0,
    COMMAND_MATCH_PREFIX,
};

struct SRequest {
    eOutputFormat format = FORMAT_NORMAL;
};

struct SResponse {
    using TResult = std::variant<std::string>;

    explicit SResponse(std::string value) : result(std::move(value)) {}
    TResult result;
};

struct SCommand {
    std::string name;
    eCommandMatch match = COMMAND_MATCH_EXACT;
    std::function<SResponse(const SRequest&)> handler;
};

} // namespace IPC::Socket1

namespace Event {

struct ReloadedEvent {
    struct State {
        std::size_t next_id = 1;
        std::size_t deliveries = 0;
        std::unordered_map<std::size_t, std::function<void()>> callbacks;
    };

    template <typename Callback>
    CHyprSignalListener listen(Callback&& listener) {
        const auto id = state->next_id++;
        state->callbacks.emplace(id, std::forward<Callback>(listener));
        const std::weak_ptr<State> weak_state = state;
        return CHyprSignalListener{[weak_state, id] {
            if (const auto locked = weak_state.lock())
                locked->callbacks.erase(id);
        }};
    }

    void emit() {
        std::vector<std::function<void()>> callbacks;
        callbacks.reserve(state->callbacks.size());
        for (const auto& [id, callback] : state->callbacks) {
            static_cast<void>(id);
            callbacks.push_back(callback);
        }
        for (const auto& callback : callbacks) {
            ++state->deliveries;
            callback();
        }
    }

    std::size_t active_count() const { return state->callbacks.size(); }
    std::size_t delivery_count() const { return state->deliveries; }

    std::shared_ptr<State> state = std::make_shared<State>();
};

struct EventBus {
    struct {
        struct {
            ReloadedEvent reloaded;
        } config;
    } m_events;
};

inline EventBus* bus() {
    static EventBus event_bus;
    return &event_bus;
}

} // namespace Event

namespace HyprlandAPI {

inline SVersionInfo runtime_version{};
inline std::string runtime_abi_hash = __hyprland_api_get_client_hash();
inline SP<IPC::Socket1::SCommand> registered_command;
inline SP<SHyprCtlCommand> registered_legacy_command;
inline bool config_registration_succeeds = true;

inline SVersionInfo getHyprlandVersion(HANDLE) {
    return runtime_version;
}

inline bool addConfigValueV2(HANDLE, SP<Config::Values::IValue>) {
    return config_registration_succeeds;
}

inline SP<IPC::Socket1::SCommand> registerHyprCtlCommand(
    HANDLE, IPC::Socket1::SCommand command) {
    registered_command = makeShared<IPC::Socket1::SCommand>(std::move(command));
    return registered_command;
}

inline SP<SHyprCtlCommand> registerHyprCtlCommand(
    HANDLE, SHyprCtlCommand command) {
    registered_legacy_command = makeShared<SHyprCtlCommand>(std::move(command));
    return registered_legacy_command;
}

} // namespace HyprlandAPI

APICALL inline EXPORT const char* __hyprland_api_get_hash() {
    return HyprlandAPI::runtime_abi_hash.c_str();
}
