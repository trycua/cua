/* SPDX-License-Identifier: MIT */
#ifndef CUA_DRIVER_ABI_H
#define CUA_DRIVER_ABI_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#if defined(CUA_DRIVER_ABI_BUILD)
#define CUA_DRIVER_API __declspec(dllexport)
#else
#define CUA_DRIVER_API __declspec(dllimport)
#endif
#else
#define CUA_DRIVER_API __attribute__((visibility("default")))
#endif

#ifdef __cplusplus
extern "C" {
#endif

#define CUA_DRIVER_ABI_MAJOR 1
#define CUA_DRIVER_ABI_MINOR 0
#define CUA_DRIVER_ABI_PATCH 0

typedef struct CuaDriverHandle CuaDriverHandle;
typedef struct CuaDriverOperation CuaDriverOperation;

typedef enum CuaDriverStatus {
  CUA_DRIVER_STATUS_OK = 0,
  CUA_DRIVER_STATUS_INVALID_ARGUMENT = 1,
  CUA_DRIVER_STATUS_NULL_POINTER = 2,
  CUA_DRIVER_STATUS_RUNTIME_UNAVAILABLE = 3,
  CUA_DRIVER_STATUS_SHUTDOWN = 4,
  CUA_DRIVER_STATUS_CANCELLED = 5,
  CUA_DRIVER_STATUS_INTERNAL = 6,
  CUA_DRIVER_STATUS_PANIC = 7,
} CuaDriverStatus;

typedef struct CuaDriverAbiVersion {
  uint32_t struct_size;
  uint16_t major;
  uint16_t minor;
  uint16_t patch;
  uint16_t reserved;
} CuaDriverAbiVersion;

/* Buffers returned by this ABI are owned by the caller. Pass the same struct
 * to cua_driver_buffer_free_v1. The function clears the struct and is
 * idempotent for an already-cleared buffer. */
typedef struct CuaDriverBuffer {
  uint8_t *data;
  size_t len;
  size_t capacity;
} CuaDriverBuffer;

typedef void (*CuaDriverCompletionV1)(void *context, CuaDriverStatus status,
                                     CuaDriverBuffer result,
                                     CuaDriverBuffer error);

CUA_DRIVER_API CuaDriverStatus
cua_driver_abi_version_v1(CuaDriverAbiVersion *out_version);
CUA_DRIVER_API bool cua_driver_abi_is_compatible_v1(uint16_t major,
                                                    uint16_t minor);
CUA_DRIVER_API void cua_driver_buffer_free_v1(CuaDriverBuffer *buffer);

/* options_json is either empty or a UTF-8 JSON object. The current v1 option
 * is {"claude_code_compatibility": boolean}. */
CUA_DRIVER_API CuaDriverStatus cua_driver_create_v1(
    const uint8_t *options_json, size_t options_len,
    CuaDriverHandle **out_handle, CuaDriverBuffer *out_error);

/* Destruction and operation release accept pointer-to-handle so they can set
 * it to NULL. Repeating either call with the cleared pointer is harmless. */
CUA_DRIVER_API void cua_driver_destroy_v1(CuaDriverHandle **handle);
CUA_DRIVER_API CuaDriverStatus cua_driver_is_available_v1(
    CuaDriverHandle *handle, bool *out_available, CuaDriverBuffer *out_error);
CUA_DRIVER_API CuaDriverStatus cua_driver_metadata_json_v1(
    CuaDriverHandle *handle, CuaDriverBuffer *out_json,
    CuaDriverBuffer *out_error);
CUA_DRIVER_API CuaDriverStatus cua_driver_list_tools_json_v1(
    CuaDriverHandle *handle, CuaDriverBuffer *out_json,
    CuaDriverBuffer *out_error);

/* Completion is delivered exactly once unless process termination prevents
 * execution. result or error is an owned UTF-8 JSON/text buffer. Cancelling an
 * operation requests cancellation and reports CUA_DRIVER_STATUS_CANCELLED via
 * the same callback. The callback may run before this function returns. */
CUA_DRIVER_API CuaDriverStatus cua_driver_invoke_v1(
    CuaDriverHandle *handle, const uint8_t *name, size_t name_len,
    const uint8_t *arguments_json, size_t arguments_len,
    CuaDriverCompletionV1 callback, void *context,
    CuaDriverOperation **out_operation, CuaDriverBuffer *out_error);
CUA_DRIVER_API CuaDriverStatus cua_driver_shutdown_v1(
    CuaDriverHandle *handle, CuaDriverCompletionV1 callback, void *context,
    CuaDriverOperation **out_operation, CuaDriverBuffer *out_error);
CUA_DRIVER_API void
cua_driver_operation_cancel_v1(CuaDriverOperation *operation);
CUA_DRIVER_API void
cua_driver_operation_release_v1(CuaDriverOperation **operation);

#ifdef __cplusplus
}
#endif

#endif /* CUA_DRIVER_ABI_H */
