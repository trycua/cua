import React from "react"
import ReactDOM from "react-dom/client"
import "@cua/design/dashboard.css"
import "@cloudscape-design/global-styles/index.css"
import { applyMode, Mode } from "@cloudscape-design/global-styles"
import { applyTheme } from "@cloudscape-design/components/theming"
import urbanistFont from "@cua/design/assets/fonts/urbanist-normal-latin.woff2"
import monoFont from "@cua/design/assets/fonts/jetbrains-mono-normal-latin.woff2"
import displayFont from "@cua/design/assets/fonts/instrument-serif-normal-latin.woff2"
import { HelmetProvider } from "react-helmet-async"
import { App } from "./App"
import { AuthProvider } from "./auth/AuthProvider"
import { I18nProvider } from "./i18n/I18nProvider"
import "./shell.css"
import { captureFleetAttribution } from "./auth/fleet-attribution"

// Capture before React and Keycloak bootstrap so login-required preserves first touch.
if (typeof window !== "undefined") captureFleetAttribution(window.location.href)

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
      colorBorderDividerSecondary: "rgba(255, 255, 255, 0.12)",
      colorBorderItemFocused: "#9fd7ff",
      colorBackgroundInputDefault: "#181818",
      colorBackgroundInputDisabled: "#181818",
      colorBackgroundControlDefault: "#181818",
      colorBackgroundControlChecked: "#9fd7ff",
      colorBackgroundControlDisabled: "#181818",
      colorBorderInputDefault: "#343434",
      colorBorderInputFocused: "#9fd7ff",
      colorBorderControlDefault: "#343434",
      // 40px control height, matching --cua-control-height-compact
      // (CuaButton): border(2px) + padding-block(2x) + line-height(20px).
      spaceFieldVertical: "9px",
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

// Inter (--cua-font-docs, sidebar/header text) is loaded from the same
// Google Fonts CDN URL the docs site uses (shell.css @font-face) rather
// than a bundled asset — see src/website/app/components/document/DocumentFonts.tsx.
const interPreconnect = document.createElement("link")
interPreconnect.rel = "preconnect"
interPreconnect.href = "https://fonts.gstatic.com"
interPreconnect.crossOrigin = "anonymous"
document.head.appendChild(interPreconnect)

const interPreload = document.createElement("link")
interPreload.rel = "preload"
interPreload.as = "font"
interPreload.type = "font/woff2"
interPreload.crossOrigin = "anonymous"
interPreload.href =
  "https://fonts.gstatic.com/s/inter/v20/UcC73FwrK3iLTeHuS_nVMrMxCp50SjIa1ZL7W0Q5nw.woff2"
document.head.appendChild(interPreload)

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <HelmetProvider>
      <I18nProvider>
        <AuthProvider>
          <App />
        </AuthProvider>
      </I18nProvider>
    </HelmetProvider>
  </React.StrictMode>,
)
