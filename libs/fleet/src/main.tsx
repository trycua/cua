import React from "react"
import ReactDOM from "react-dom/client"
import "@cloudscape-design/global-styles/index.css"
import { applyMode, Mode } from "@cloudscape-design/global-styles"
import { App } from "./App"
import { AuthProvider } from "./auth/AuthProvider"
import { DeviceAuthorization } from "./pages/DeviceAuthorization"

applyMode(Mode.Dark)

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
