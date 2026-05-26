#ifndef CUA_DRIVER_EMBEDDED_H
#define CUA_DRIVER_EMBEDDED_H

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct CuaDriver CuaDriver;

// Create an embedded cua-driver instance.
//
// claude_code_compat mirrors:
//   cua-driver mcp --claude-code-computer-use-compat
//
// Returns NULL if the embedded Tokio runtime cannot be created.
CuaDriver *cua_driver_embedded_new(bool claude_code_compat);

// Free an embedded driver returned by cua_driver_embedded_new.
void cua_driver_embedded_free(CuaDriver *driver);

// Handle one JSON-RPC MCP request object encoded as UTF-8 JSON.
// GUI hosts should call this from a worker queue rather than the app main
// thread, because some macOS tools interact with AppKit or system services.
//
// Returns:
// - an owned UTF-8 JSON response string for requests and parse errors;
// - NULL for JSON-RPC notifications, because notifications do not produce responses.
//
// Free non-NULL responses with cua_driver_embedded_string_free.
char *cua_driver_embedded_handle_mcp_json(
    CuaDriver *driver,
    const char *request_json
);

// Free a string returned by cua_driver_embedded_handle_mcp_json.
void cua_driver_embedded_string_free(char *value);

#ifdef __cplusplus
}
#endif

#endif
