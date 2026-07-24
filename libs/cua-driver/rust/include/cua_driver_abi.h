/* SPDX-License-Identifier: MIT */

#ifndef CUA_DRIVER_ABI_H
#define CUA_DRIVER_ABI_H

/* Generated from cua-driver-sdk/src/abi.rs by cua-driver-abi-header. Do not edit. */

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#if defined(_WIN32)
#if defined(CUA_DRIVER_ABI_BUILD)
#define CUA_DRIVER_API __declspec(dllexport)
#else
#define CUA_DRIVER_API __declspec(dllimport)
#endif
#else
#define CUA_DRIVER_API __attribute__((visibility("default")))
#endif


#define CUA_DRIVER_ABI_MAJOR 1

#define CUA_DRIVER_ABI_MINOR 1

#define CUA_DRIVER_ABI_PATCH 0

#define DRIVER_ENVELOPE_VERSION 1

#define PRIVATE_WORKER_PROTOCOL_VERSION 1

/**
 * Stable status codes returned across the C ABI.
 */
enum CuaDriverStatus
#if defined(__cplusplus) || __STDC_VERSION__ >= 202311L
  : int32_t
#endif // defined(__cplusplus) || __STDC_VERSION__ >= 202311L
 {
  CUA_DRIVER_STATUS_OK = 0,
  CUA_DRIVER_STATUS_INVALID_ARGUMENT = 1,
  CUA_DRIVER_STATUS_NULL_POINTER = 2,
  CUA_DRIVER_STATUS_RUNTIME_UNAVAILABLE = 3,
  CUA_DRIVER_STATUS_SHUTDOWN = 4,
  CUA_DRIVER_STATUS_CANCELLED = 5,
  CUA_DRIVER_STATUS_INTERNAL = 6,
  CUA_DRIVER_STATUS_PANIC = 7,
  CUA_DRIVER_STATUS_RUNTIME_CONFLICT = 8,
};
#ifndef __cplusplus
#if __STDC_VERSION__ >= 202311L
typedef enum CuaDriverStatus CuaDriverStatus;
#else
typedef int32_t CuaDriverStatus;
#endif // __STDC_VERSION__ >= 202311L
#endif // __cplusplus

/**
 * Opaque driver runtime handle.
 */
typedef struct CuaDriverHandle CuaDriverHandle;

/**
 * Opaque token for one asynchronous operation.
 */
typedef struct CuaDriverOperation CuaDriverOperation;

/**
 * Opaque session handle whose actions are already bound to one immutable
 * authorization context.
 */
typedef struct CuaDriverSessionHandle CuaDriverSessionHandle;

/**
 * Version of the implementation-neutral Cua Driver ABI.
 */
typedef struct {
  uint32_t struct_size;
  uint16_t major;
  uint16_t minor;
  uint16_t patch;
  uint16_t reserved;
} CuaDriverAbiVersion;

/**
 * Caller-owned byte buffer returned by the ABI.
 * Pass it to `cua_driver_buffer_free_v1`; freeing an empty buffer is harmless.
 */
typedef struct {
  uint8_t *data;
  size_t len;
  size_t capacity;
} CuaDriverBuffer;

/**
 * Completion callback for asynchronous operations. It is called exactly once
 * unless the process terminates. Result and error buffers are caller-owned.
 */
typedef void (*CuaDriverCompletionV1)(void *context,
                                      CuaDriverStatus status,
                                      CuaDriverBuffer result,
                                      CuaDriverBuffer error);

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * Read the runtime ABI version into `out_version`.
 */
CUA_DRIVER_API CuaDriverStatus cua_driver_abi_version_v1(CuaDriverAbiVersion *out_version);

/**
 * Return whether this runtime supports a caller compiled for `major.minor`.
 */
CUA_DRIVER_API bool cua_driver_abi_is_compatible_v1(uint16_t major, uint16_t minor);

/**
 * Free and clear a buffer returned by this ABI. Repeated calls are harmless.
 */
CUA_DRIVER_API void cua_driver_buffer_free_v1(CuaDriverBuffer *buffer);

/**
 * Create an in-process driver. `options_json` is empty or a UTF-8 JSON object.
 * It accepts `claude_code_compatibility` and an optional immutable
 * `authorization` ceiling. Unknown fields fail closed.
 */
CUA_DRIVER_API
CuaDriverStatus cua_driver_create_v1(const uint8_t *options_json,
                                     size_t options_len,
                                     CuaDriverHandle **out_handle,
                                     CuaDriverBuffer *out_error);

/**
 * Destroy and clear a driver handle. Repeated calls are harmless.
 */
CUA_DRIVER_API void cua_driver_destroy_v1(CuaDriverHandle **handle);

/**
 * Return whether the driver still accepts operations.
 */
CUA_DRIVER_API
CuaDriverStatus cua_driver_is_available_v1(CuaDriverHandle *handle,
                                           bool *out_available,
                                           CuaDriverBuffer *out_error);

/**
 * Return driver metadata as caller-owned UTF-8 JSON.
 */
CUA_DRIVER_API
CuaDriverStatus cua_driver_metadata_json_v1(CuaDriverHandle *handle,
                                            CuaDriverBuffer *out_json,
                                            CuaDriverBuffer *out_error);

/**
 * Return the canonical SDK/MCP tool inventory as caller-owned UTF-8 JSON.
 */
CUA_DRIVER_API
CuaDriverStatus cua_driver_list_tools_json_v1(CuaDriverHandle *handle,
                                              CuaDriverBuffer *out_json,
                                              CuaDriverBuffer *out_error);

/**
 * Invoke a named tool asynchronously. Cancellation is requested with
 * `cua_driver_operation_cancel_v1`; release the token after completion.
 */
CUA_DRIVER_API
CuaDriverStatus cua_driver_invoke_v1(CuaDriverHandle *handle,
                                     const uint8_t *name,
                                     size_t name_len,
                                     const uint8_t *arguments_json,
                                     size_t arguments_len,
                                     CuaDriverCompletionV1 callback,
                                     void *context,
                                     CuaDriverOperation **out_operation,
                                     CuaDriverBuffer *out_error);

/**
 * Create a trusted, immutable session binding below this runtime's ceiling.
 *
 * This is a host API, not an agent tool. The returned handle carries its
 * authority in memory and cannot be reconstructed from a public session ID.
 */
CUA_DRIVER_API
CuaDriverStatus cua_driver_session_create_v1(CuaDriverHandle *handle,
                                             const uint8_t *options_json,
                                             size_t options_len,
                                             CuaDriverSessionHandle **out_session,
                                             CuaDriverBuffer *out_error);

/**
 * Destroy a trusted session handle and revoke its connection-bound grants.
 */
CUA_DRIVER_API void cua_driver_session_destroy_v1(CuaDriverSessionHandle **handle);

/**
 * Invoke through an already-bound trusted session context.
 */
CUA_DRIVER_API
CuaDriverStatus cua_driver_session_invoke_v1(CuaDriverSessionHandle *handle,
                                             const uint8_t *name,
                                             size_t name_len,
                                             const uint8_t *arguments_json,
                                             size_t arguments_len,
                                             CuaDriverCompletionV1 callback,
                                             void *context,
                                             CuaDriverOperation **out_operation,
                                             CuaDriverBuffer *out_error);

/**
 * Stop admission, drain admitted calls, and finalize SDK-owned resources.
 */
CUA_DRIVER_API
CuaDriverStatus cua_driver_shutdown_v1(CuaDriverHandle *handle,
                                       CuaDriverCompletionV1 callback,
                                       void *context,
                                       CuaDriverOperation **out_operation,
                                       CuaDriverBuffer *out_error);

/**
 * Request cancellation of an asynchronous operation.
 */
CUA_DRIVER_API void cua_driver_operation_cancel_v1(CuaDriverOperation *operation);

/**
 * Release and clear an operation token. Repeated calls are harmless.
 */
CUA_DRIVER_API void cua_driver_operation_release_v1(CuaDriverOperation **operation);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* CUA_DRIVER_ABI_H */
