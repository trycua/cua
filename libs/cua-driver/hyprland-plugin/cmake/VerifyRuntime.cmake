execute_process(COMMAND "${CMAKE_COMMAND}" -E env LC_ALL=C "${READELF}" -d "${MODULE}"
    RESULT_VARIABLE result OUTPUT_VARIABLE dynamic ERROR_VARIABLE error)
if(NOT result EQUAL 0)
    message(FATAL_ERROR "Cannot inspect module runtime dependencies: ${error}")
endif()
# A partially staged GCC toolchain can silently select libstdc++.a. Its C++
# globals must not be embedded into a module loaded by another C++ process.
if(NOT dynamic MATCHES "Shared library: \\[(libstdc\\+\\+|libc\\+\\+)\\.so")
    message(FATAL_ERROR
        "Plugin lacks a shared C++ runtime dependency. Check the compiler's runtime search paths; do not load this module.")
endif()
