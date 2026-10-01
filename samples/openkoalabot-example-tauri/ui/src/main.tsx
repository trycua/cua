// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { StrictMode } from "react"
import { createRoot } from "react-dom/client"
import { App } from "./App"
import { PipView, readPipRoute } from "./Pip"
import "./styles.css"

// A picture-in-picture window loads this page with a `#pip=…` route.
const pip = readPipRoute(location.hash)

createRoot(document.getElementById("root")!).render(
  <StrictMode>{pip ? <PipView source={pip.source} space={pip.space} /> : <App />}</StrictMode>,
)
