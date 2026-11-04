// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import "./styles.css";
import { Viewer } from "./viewer";

const root = document.getElementById("app");
if (root) void new Viewer(root).start();
