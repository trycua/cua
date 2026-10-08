// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createRouter, RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { BridgeProvider } from "@/bridge";
import { ToastProvider } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { routeTree } from "./routeTree.gen";
import "./index.css";

const router = createRouter({ routeTree, defaultPreload: "intent", scrollRestoration: true });

// The Electron shell's picture-in-picture windows show only the stream
// (apps/cua-spaces-desktop/src/pip.ts), without the app around it.
const pip = location.pathname === "/pip";

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

const root = createRoot(document.getElementById("root")!);
if (pip) {
  void import("./components/pip/pip-view").then(({ PipView, pipParams }) =>
    root.render(
      <StrictMode>
        <PipView params={pipParams(location.search)} />
      </StrictMode>,
    ),
  );
} else {
  root.render(
    <StrictMode>
      <BridgeProvider>
        <TooltipProvider delay={500}>
          <ToastProvider>
            <RouterProvider router={router} />
          </ToastProvider>
        </TooltipProvider>
      </BridgeProvider>
    </StrictMode>,
  );
}
