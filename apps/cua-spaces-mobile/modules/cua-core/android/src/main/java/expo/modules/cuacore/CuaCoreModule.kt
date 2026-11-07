// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

package expo.modules.cuacore

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition

/**
 * Android: the Rust core is not linked yet (build it with cargo-ndk, see
 * docs/DESIGN.md "Android"). Until then `available` is false and the app
 * says so; nothing here pretends to answer.
 */
class CuaCoreModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("CuaCore")
    Constants("available" to false)
  }
}
