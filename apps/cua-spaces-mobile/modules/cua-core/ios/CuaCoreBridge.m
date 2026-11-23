// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#import "CuaCoreBridge.h"
#include "cua_core.h"

NSString *_Nullable CuaCoreCall(NSString *method, NSString *argsJson, NSString *_Nullable *_Nullable error) {
  char *err = NULL;
  char *out = cua_core_call(method.UTF8String, argsJson.UTF8String, &err);
  if (out == NULL) {
    if (error) *error = err ? [NSString stringWithUTF8String:err] : @"the core returned nothing";
    if (err) cua_core_free(err);
    return nil;
  }
  NSString *s = [NSString stringWithUTF8String:out];
  cua_core_free(out);
  return s;
}

static NSString *take(char *p) {
  if (!p) return @"[]";
  NSString *s = [NSString stringWithUTF8String:p];
  cua_core_free(p);
  return s;
}

NSString *CuaCoreMethods(void) { return take(cua_core_methods()); }
NSString *CuaCoreFlows(void) { return take(cua_core_flows()); }
