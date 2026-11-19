// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.
#ifndef CUA_CORE_H
#define CUA_CORE_H
#ifdef __cplusplus
extern "C" {
#endif
/// Calls a core method. Returns the result JSON (free with cua_core_free), or
/// NULL with *error set (free with cua_core_free).
char *cua_core_call(const char *method, const char *args, char **error);
char *cua_core_methods(void);
char *cua_core_flows(void);
void cua_core_free(char *s);
#ifdef __cplusplus
}
#endif
#endif
