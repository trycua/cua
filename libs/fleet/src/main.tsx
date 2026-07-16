import React from "react"
import ReactDOM from "react-dom/client"
import "@cloudscape-design/global-styles/index.css"
import { applyMode, Mode } from "@cloudscape-design/global-styles"
import { App } from "./App"
import { AuthProvider } from "./auth/AuthProvider"

applyMode(Mode.Dark)

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <AuthProvider>
      <App />
    </AuthProvider>
  </React.StrictMode>,
)
