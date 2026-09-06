cmake_minimum_required(VERSION 3.30)

project(cua_hyprland_plugin VERSION 0.1.0 LANGUAGES CXX)

option(CUA_HYPRLAND_BUILD_PLUGIN "Build the ABI-pinned Hyprland plugin" ON)
set(CUA_HYPRLAND_EXPECTED_VERSION "" CACHE STRING
    "Require an exact hyprland pkg-config version when building the plugin")

include(CTest)
include(cmake/DetectHyprlandAPI.cmake)

function(cua_hyprland_harden target)
    if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
        target_compile_definitions(${target} PRIVATE
            $<$<CONFIG:Release,RelWithDebInfo,MinSizeRel>:_FORTIFY_SOURCE=3>
            _GLIBCXX_ASSERTIONS)
        target_compile_options(${target} PRIVATE -fstack-protector-strong)
    endif()
    target_compile_options(${target} PRIVATE -fno-lto)
endfunction()

add_library(cua_hyprland_protocol STATIC
    src/protocol.cpp
    src/session.cpp
    src/status.cpp)
target_compile_features(cua_hyprland_protocol PUBLIC cxx_std_26)
target_include_directories(cua_hyprland_protocol PUBLIC include)
target_compile_options(cua_hyprland_protocol PRIVATE -Wall -Wextra -Wpedantic -Werror)
cua_hyprland_harden(cua_hyprland_protocol)
set_target_properties(cua_hyprland_protocol PROPERTIES POSITION_INDEPENDENT_CODE ON)

if(BUILD_TESTING)
    add_test(NAME cua_hyprland_cmake_api_test
        COMMAND ${CMAKE_COMMAND}
            -S ${CMAKE_CURRENT_SOURCE_DIR}/tests/cmake-api
            -B ${CMAKE_CURRENT_BINARY_DIR}/cmake-api-test
            -DCMAKE_CXX_COMPILER=${CMAKE_CXX_COMPILER})
    add_executable(cua_hyprland_protocol_test tests/protocol_test.cpp)
    target_link_libraries(cua_hyprland_protocol_test PRIVATE cua_hyprland_protocol)
    target_compile_options(cua_hyprland_protocol_test PRIVATE -Wall -Wextra -Wpedantic -Werror)
    add_test(NAME cua_hyprland_protocol_test COMMAND cua_hyprland_protocol_test)

    add_executable(cua_hyprland_status_test tests/status_test.cpp)
    target_link_libraries(cua_hyprland_status_test PRIVATE cua_hyprland_protocol)
    target_compile_options(cua_hyprland_status_test PRIVATE -Wall -Wextra -Wpedantic -Werror)
    add_test(NAME cua_hyprland_status_test COMMAND cua_hyprland_status_test)

    foreach(api_variant IN ITEMS socket1 legacy)
        if(api_variant STREQUAL "socket1")
            set(api_value 1)
        else()
            set(api_value 0)
        endif()
        add_library(cua_hyprland_plugin_api_${api_variant}_compile_test
            OBJECT src/plugin.cpp)
        target_compile_features(cua_hyprland_plugin_api_${api_variant}_compile_test
            PRIVATE cxx_std_26)
        target_compile_definitions(cua_hyprland_plugin_api_${api_variant}_compile_test
            PRIVATE
            CUA_HYPRLAND_PLUGIN_VERSION="${PROJECT_VERSION}"
            CUA_HYPRLAND_SOCKET1_API=${api_value})
        target_compile_options(cua_hyprland_plugin_api_${api_variant}_compile_test
            PRIVATE -Wall -Wextra -Wpedantic -Werror -Wno-return-type-c-linkage)
        target_include_directories(cua_hyprland_plugin_api_${api_variant}_compile_test
            PRIVATE tests/mock-hyprland include src)
    endforeach()
endif()

if(UNIX)
    find_package(Threads REQUIRED)
    add_library(cua_hyprland_transport STATIC src/inject_server.cpp)
    target_compile_features(cua_hyprland_transport PUBLIC cxx_std_26)
    target_compile_options(cua_hyprland_transport PRIVATE -Wall -Wextra -Wpedantic -Werror)
    if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
        target_compile_definitions(cua_hyprland_transport PRIVATE _GNU_SOURCE)
    endif()
    target_include_directories(cua_hyprland_transport PUBLIC include src)
    target_link_libraries(cua_hyprland_transport PUBLIC
        cua_hyprland_protocol
        Threads::Threads)
    cua_hyprland_harden(cua_hyprland_transport)
    set_target_properties(cua_hyprland_transport PROPERTIES POSITION_INDEPENDENT_CODE ON)

    if(BUILD_TESTING)
        add_executable(cua_hyprland_transport_test tests/transport_test.cpp)
        target_compile_options(cua_hyprland_transport_test PRIVATE -Wall -Wextra -Wpedantic -Werror)
        target_link_libraries(cua_hyprland_transport_test PRIVATE cua_hyprland_transport)
        add_test(NAME cua_hyprland_transport_test COMMAND cua_hyprland_transport_test)

        foreach(api_variant IN ITEMS socket1 legacy)
            if(api_variant STREQUAL "socket1")
                set(api_value 1)
            else()
                set(api_value 0)
            endif()
            add_executable(cua_hyprland_plugin_api_${api_variant}_test
                tests/plugin_api_test.cpp
                $<TARGET_OBJECTS:cua_hyprland_plugin_api_${api_variant}_compile_test>)
            target_compile_features(cua_hyprland_plugin_api_${api_variant}_test
                PRIVATE cxx_std_26)
            target_compile_definitions(cua_hyprland_plugin_api_${api_variant}_test
                PRIVATE CUA_HYPRLAND_SOCKET1_API=${api_value})
            target_compile_options(cua_hyprland_plugin_api_${api_variant}_test PRIVATE
                -Wall -Wextra -Wpedantic -Werror -Wno-return-type-c-linkage)
            target_include_directories(cua_hyprland_plugin_api_${api_variant}_test PRIVATE
                tests tests/mock-hyprland include src)
            target_link_libraries(cua_hyprland_plugin_api_${api_variant}_test PRIVATE
                cua_hyprland_transport)
            add_test(NAME cua_hyprland_plugin_api_${api_variant}_test
                COMMAND cua_hyprland_plugin_api_${api_variant}_test)
        endforeach()
    endif()
endif()

if(CUA_HYPRLAND_BUILD_PLUGIN)
    if(NOT CMAKE_SYSTEM_NAME STREQUAL "Linux")
        message(FATAL_ERROR "The Hyprland plugin can only be built on Linux")
    endif()

    find_package(PkgConfig REQUIRED)
    if(CUA_HYPRLAND_EXPECTED_VERSION)
        pkg_check_modules(HYPRLAND REQUIRED IMPORTED_TARGET
            "hyprland=${CUA_HYPRLAND_EXPECTED_VERSION}")
    else()
        pkg_check_modules(HYPRLAND REQUIRED IMPORTED_TARGET hyprland)
    endif()
    message(STATUS "Building against Hyprland headers ${HYPRLAND_VERSION}")

    cua_detect_hyprland_api(cua_hyprland_socket1_api
        "${HYPRLAND_INCLUDE_DIRS}" "${HYPRLAND_CFLAGS_OTHER}")
    if(cua_hyprland_socket1_api STREQUAL "1")
        message(STATUS "Using Hyprland Socket1 status-command API")
    elseif(cua_hyprland_socket1_api STREQUAL "0")
        message(STATUS "Using Hyprland legacy status-command API")
    else()
        message(FATAL_ERROR
            "Hyprland headers expose no supported hyprctl plugin command API")
    endif()

    add_library(cua_hyprland_plugin MODULE src/plugin.cpp)
    target_compile_features(cua_hyprland_plugin PRIVATE cxx_std_26)
    target_compile_definitions(cua_hyprland_plugin PRIVATE
        CUA_HYPRLAND_PLUGIN_VERSION="${PROJECT_VERSION}"
        CUA_HYPRLAND_SOCKET1_API=${cua_hyprland_socket1_api}
        _GNU_SOURCE)
    target_compile_options(cua_hyprland_plugin PRIVATE
        -Wall -Wextra -Wpedantic -Werror -Wno-return-type-c-linkage)
    cua_hyprland_harden(cua_hyprland_plugin)
    target_link_options(cua_hyprland_plugin PRIVATE
        -Wl,-z,relro
        -Wl,-z,now
        -Wl,-z,noexecstack)
    target_link_libraries(cua_hyprland_plugin PRIVATE
        cua_hyprland_transport
        PkgConfig::HYPRLAND
        Threads::Threads)
    set_target_properties(cua_hyprland_plugin PROPERTIES
        CXX_VISIBILITY_PRESET hidden
        PREFIX ""
        OUTPUT_NAME "cua-hyprland-plugin")

    if(NOT CMAKE_READELF)
        message(FATAL_ERROR "readelf is required to verify the plugin C++ runtime")
    endif()
    add_custom_command(TARGET cua_hyprland_plugin POST_BUILD
        COMMAND ${CMAKE_COMMAND}
            -DREADELF=${CMAKE_READELF}
            -DMODULE=$<TARGET_FILE:cua_hyprland_plugin>
            -P ${CMAKE_CURRENT_SOURCE_DIR}/cmake/VerifyRuntime.cmake
        VERBATIM)

    install(TARGETS cua_hyprland_plugin
        LIBRARY DESTINATION lib/cua/hyprland)
endif()
