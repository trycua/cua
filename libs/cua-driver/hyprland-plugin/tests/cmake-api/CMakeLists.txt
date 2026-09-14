cmake_minimum_required(VERSION 3.30)
project(cua_hyprland_api_detection_test LANGUAGES CXX)
include(../../cmake/DetectHyprlandAPI.cmake)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_REQUIRED_FLAGS "-DCUA_CALLER_SETTING=1")
unset(CMAKE_TRY_COMPILE_TARGET_TYPE)

foreach(tier IN ITEMS socket1 legacy unsupported socket1)
    cua_detect_hyprland_api(actual "${CMAKE_CURRENT_SOURCE_DIR}/include"
        "-DCUA_PROBE_${tier}=1;-DCUA_PROBE_FLAG=1")
    if(tier STREQUAL "socket1")
        set(expected 1)
    elseif(tier STREQUAL "legacy")
        set(expected 0)
    else()
        set(expected unsupported)
    endif()
    if(NOT actual STREQUAL expected)
        message(FATAL_ERROR "Expected ${tier} (${expected}), found ${actual}")
    endif()
endforeach()

if(NOT CMAKE_CXX_STANDARD STREQUAL "17" OR
   NOT CMAKE_REQUIRED_FLAGS STREQUAL "-DCUA_CALLER_SETTING=1" OR
   DEFINED CMAKE_TRY_COMPILE_TARGET_TYPE)
    message(FATAL_ERROR "API detection changed caller settings")
endif()
