// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

/// Calls the Rust app core. Returns the result JSON, or nil with `error` set.
FOUNDATION_EXPORT NSString *_Nullable CuaCoreCall(NSString *method, NSString *argsJson, NSString *_Nullable *_Nullable error);
/// Every method name as a JSON array.
FOUNDATION_EXPORT NSString *CuaCoreMethods(void);
/// The parity flows as JSON.
FOUNDATION_EXPORT NSString *CuaCoreFlows(void);

NS_ASSUME_NONNULL_END
