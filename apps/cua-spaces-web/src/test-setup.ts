// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// waitFor/findBy wait up to 5 s instead of Testing Library's 1 s: the
// app-level tests render the real routes over the wasm core, which a CI
// runner under the whole suite's load can take more than a second to
// settle. Passing tests return as soon as their condition holds.

import { configure } from "@testing-library/react";
import { afterEach } from "vitest";

import { resetAddressing } from "@/lib/addressing";

configure({ asyncUtilTimeout: 5_000 });

// Who each Space's composer is talking to is kept per Space; a test never inherits another's.
afterEach(resetAddressing);
