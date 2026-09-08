cmake_minimum_required(VERSION 3.21)
project(cua_kwin_target_helper LANGUAGES CXX)

set(CMAKE_AUTOMOC ON)
set(BUILD_SHARED_LIBS ON)
set(CMAKE_LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}")

find_package(Qt6 REQUIRED COMPONENTS Core DBus Gui Widgets)
find_package(KF6CoreAddons REQUIRED)
find_package(KWin REQUIRED)
include(KDEInstallDirs)
include(KDECMakeSettings)
set(KDE_INSTALL_PLUGINDIR "${CMAKE_INSTALL_LIBDIR}/qt6/plugins")
set(KDE_INSTALL_QTPLUGINDIR "${CMAKE_INSTALL_LIBDIR}/qt6/plugins")

kcoreaddons_add_plugin(cua_kwin_target_helper
    SOURCES kwin_target_helper.cpp
    INSTALL_NAMESPACE "kwin/effects/plugins")
target_compile_features(cua_kwin_target_helper PRIVATE cxx_std_20)
target_link_libraries(cua_kwin_target_helper PRIVATE KWin::kwin Qt6::Core Qt6::DBus)
