import React from "react"
import ReactDOM from "react-dom/client"
import "@cua/design/dashboard.css"
import "@cloudscape-design/global-styles/index.css"
import { applyMode, Mode } from "@cloudscape-design/global-styles"
import { applyTheme } from "@cloudscape-design/components/theming"
import urbanistFont from "@cua/design/assets/fonts/urbanist-normal-latin.woff2"
import monoFont from "@cua/design/assets/fonts/jetbrains-mono-normal-latin.woff2"
import displayFont from "@cua/design/assets/fonts/instrument-serif-normal-latin.woff2"
import { App } from "./App"
import { AuthProvider } from "./auth/AuthProvider"
import { DeviceAuthorization } from "./pages/DeviceAuthorization"
import "./shell.css"

applyMode(Mode.Dark)
document.documentElement.classList.add("cua-dashboard-theme")
document.body.id = "cua-dashboard-root"
applyTheme({
  theme: {
    tokens: {
      colorBackgroundLayoutMain: "#000000",
      colorBackgroundLayoutToolbar: "#000000",
      colorBackgroundContainerContent: "#181818",
      colorBackgroundContainerHeader: "#181818",
      colorBackgroundButtonPrimaryDefault: "#9fd7ff",
      colorBackgroundButtonPrimaryHover: "#b7dcff",
      colorBackgroundButtonPrimaryActive: "#ecf6ff",
      colorBorderButtonPrimaryDefault: "#9fd7ff",
      colorBorderButtonPrimaryHover: "#b7dcff",
      colorBorderButtonPrimaryActive: "#ecf6ff",
      colorTextButtonPrimaryDefault: "#000000",
      colorTextButtonPrimaryHover: "#000000",
      colorTextButtonPrimaryActive: "#000000",
      colorTextBodyDefault: "#f6f8fb",
      colorTextBodySecondary: "rgba(224, 230, 238, 0.84)",
      colorTextHeadingDefault: "#f6f8fb",
      colorTextHeadingSecondary: "rgba(224, 230, 238, 0.84)",
      colorTextLinkDefault: "#9fd7ff",
      colorTextLinkHover: "#ecf6ff",
      colorBorderDividerDefault: "rgba(255, 255, 255, 0.12)",
      colorBorderItemFocused: "#9fd7ff",
      fontFamilyBase: '"Urbanist", -apple-system, BlinkMacSystemFont, sans-serif',
    },
  },
})

for (const href of [urbanistFont, monoFont, displayFont]) {
  const link = document.createElement("link")
  link.rel = "preload"
  link.as = "font"
  link.type = "font/woff2"
  link.crossOrigin = "anonymous"
  link.href = href
  document.head.appendChild(link)
}

const isDeviceAuthorization = window.location.pathname === "/device"

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {isDeviceAuthorization ? (
      <DeviceAuthorization />
    ) : (
      <AuthProvider>
        <App />
      </AuthProvider>
    )}
  </React.StrictMode>,
)
