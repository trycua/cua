# SPDX-License-Identifier: MIT
# KWin 6.3's package config looks up epoxy, but KWin installs no module to find
# it (Debian 13). Find it through pkg-config, as KWin's own build does.
find_package(PkgConfig QUIET)
pkg_check_modules(PC_epoxy QUIET epoxy)
find_path(epoxy_INCLUDE_DIR NAMES epoxy/gl.h HINTS ${PC_epoxy_INCLUDE_DIRS})
find_library(epoxy_LIBRARY NAMES epoxy HINTS ${PC_epoxy_LIBRARY_DIRS})
include(FindPackageHandleStandardArgs)
find_package_handle_standard_args(epoxy
    REQUIRED_VARS epoxy_LIBRARY epoxy_INCLUDE_DIR
    VERSION_VAR PC_epoxy_VERSION)
if(epoxy_FOUND AND NOT TARGET epoxy::epoxy)
    add_library(epoxy::epoxy UNKNOWN IMPORTED)
    set_target_properties(epoxy::epoxy PROPERTIES
        IMPORTED_LOCATION "${epoxy_LIBRARY}"
        INTERFACE_INCLUDE_DIRECTORIES "${epoxy_INCLUDE_DIR}")
endif()
mark_as_advanced(epoxy_INCLUDE_DIR epoxy_LIBRARY)
