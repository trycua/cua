include(CheckCXXSourceCompiles)

function(cua_detect_hyprland_api output includes flags)
    # Probes must use the module's language mode; function scope preserves the
    # caller's try-compile settings, including settings that were unset.
    set(CMAKE_CXX_STANDARD 26)
    set(CMAKE_CXX_STANDARD_REQUIRED ON)
    set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)
    set(CMAKE_REQUIRED_INCLUDES ${includes})
    string(JOIN " " CMAKE_REQUIRED_FLAGS ${flags})
    # Reconfigure after a header upgrade must not reuse a previous ABI tier.
    unset(CUA_HYPRLAND_HAS_SOCKET1_STATUS_API CACHE)
    unset(CUA_HYPRLAND_HAS_LEGACY_STATUS_API CACHE)
    check_cxx_source_compiles(
        "#include <src/plugins/PluginAPI.hpp>\nint main() { IPC::Socket1::SCommand command{}; (void)command; }"
        CUA_HYPRLAND_HAS_SOCKET1_STATUS_API)
    if(CUA_HYPRLAND_HAS_SOCKET1_STATUS_API)
        set(${output} 1 PARENT_SCOPE)
        return()
    endif()
    check_cxx_source_compiles(
        "#include <src/plugins/PluginAPI.hpp>\nint main() { SHyprCtlCommand command{}; (void)command; }"
        CUA_HYPRLAND_HAS_LEGACY_STATUS_API)
    if(CUA_HYPRLAND_HAS_LEGACY_STATUS_API)
        set(${output} 0 PARENT_SCOPE)
    else()
        set(${output} unsupported PARENT_SCOPE)
    endif()
endfunction()
