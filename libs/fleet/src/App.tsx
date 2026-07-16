import {
  BrowserRouter,
  Navigate,
  Outlet,
  Route,
  Routes,
  useLocation,
  useNavigate,
} from "react-router-dom"
import AppLayout from "@cloudscape-design/components/app-layout"
import Button from "@cloudscape-design/components/button"
import SideNavigation from "@cloudscape-design/components/side-navigation"
import TopNavigation, {
  type TopNavigationProps,
} from "@cloudscape-design/components/top-navigation"
import Flashbar, {
  type FlashbarProps,
} from "@cloudscape-design/components/flashbar"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { PoolsList } from "./pages/PoolsList"
import { ClaimDetail } from "./pages/ClaimDetail"
import { PoolDetail } from "./pages/PoolDetail"
import { PoolNew } from "./pages/PoolNew"
import { PoolReplicaDetail } from "./pages/PoolReplicaDetail"
import { NodesList } from "./pages/NodesList"
import { OperatorEvents } from "./pages/OperatorEvents"
import { ApiKeys } from "./pages/ApiKeys"
import { UserApiKeys } from "./pages/UserApiKeys"
import { Settings } from "./pages/Settings"
import { FlashContext, type FlashMsg } from "./components/FlashContext"
import { FeatureFlagProvider, useFeatureFlags } from "./components/FeatureFlagContext"
import { logout, userInfo } from "./auth/keycloak"

const VERSION_CHECK_INTERVAL_MS = 60_000

function useStaleCheck(): { stale: boolean } {
  const [stale, setStale] = useState(false)
  const currentEntry = useRef<string | null>(null)

  useEffect(() => {
    if (stale) return

    // The entry script is what's actually executing; lazy chunk hashes
    // changing don't make the running app stale on their own.
    const entry = Array.from(
      document.querySelectorAll<HTMLScriptElement>('script[type="module"][src]'),
    )
      .map(s => s.src)
      .find(src => src.includes('/assets/'))
    if (!entry) return
    currentEntry.current = entry

    const check = async () => {
      try {
        // High-signal: if the CDN no longer serves our entry chunk,
        // the deploy invalidated us — next dynamic import would fail anyway.
        const head = await fetch(currentEntry.current!, { method: 'HEAD', cache: 'no-cache' })
        if (head.status === 404) {
          setStale(true)
          return
        }

        const res = await fetch('/', { cache: 'no-cache' })
        const html = await res.text()
        const match = html.match(/<script[^>]+type="module"[^>]+src="(\/assets\/[^"']+)"/)
        const latestEntry = match?.[1]
        if (latestEntry && !currentEntry.current!.endsWith(latestEntry)) {
          setStale(true)
        }
      } catch {
        // Network error — ignore
      }
    }

    const id = window.setInterval(check, VERSION_CHECK_INTERVAL_MS)
    return () => window.clearInterval(id)
  }, [stale])

  return { stale }
}

function Shell() {
  const location = useLocation()
  const navigate = useNavigate()
  const [flashes, setFlashes] = useState<FlashbarProps.MessageDefinition[]>([])
  const { stale } = useStaleCheck()
  const [staleDismissed, setStaleDismissed] = useState(false)
  const { admin } = useFeatureFlags()
  const user = userInfo()
  const pushFlash = useCallback((msg: FlashMsg) => {
    const id = crypto.randomUUID()
    const dismiss = () =>
      setFlashes(prev => prev.filter(f => f.id !== id))
    setFlashes(prev => [
      ...prev,
      {
        ...msg,
        id,
        dismissible: true,
        onDismiss: dismiss,
      },
    ])
    if (msg.type === "success") {
      window.setTimeout(dismiss, 5000)
    }
  }, [])
  const flashContext = useMemo(() => ({ push: pushFlash }), [pushFlash])

  return (
    <FlashContext.Provider value={flashContext}>
      <TopNavigation
        identity={{ href: "#/", title: "Pools" }}
        utilities={[
          {
            type: "button",
            text: "Pools",
            href: "#/pools",
            onClick: e => {
              e.preventDefault()
              navigate("/pools")
            },
          },
          ...(admin
            ? ([
                {
                  type: "button",
                  text: "Nodes",
                  href: "#/nodes",
                  onClick: e => {
                    e.preventDefault()
                    navigate("/nodes")
                  },
                },
                {
                  type: "button",
                  text: "Operator events",
                  href: "#/operator-events",
                  onClick: e => {
                    e.preventDefault()
                    navigate("/operator-events")
                  },
                },
              ] satisfies TopNavigationProps.Utility[])
            : []),
          {
            type: "menu-dropdown",
            text: user.name ?? user.email ?? "Account",
            description: user.email,
            iconName: "user-profile",
            items: [{ id: "signout", text: "Sign out" }],
            onItemClick: e => {
              if (e.detail.id === "signout") logout()
            },
          },
        ]}
      />
      <AppLayout
        toolsHide
        navigation={
          <div style={{ fontSize: "1.25rem" }}>
            <SideNavigation
              header={{ href: "#/", text: "Pools" }}
              activeHref={`#${location.pathname}`}
              onFollow={e => {
                if (!e.detail.external) {
                  e.preventDefault()
                  navigate(e.detail.href.replace(/^#/, ""))
                }
              }}
              items={[
                { type: "link", text: "Pools", href: "#/pools" },
                ...(admin
                  ? [
                      { type: "link" as const, text: "Nodes", href: "#/nodes" },
                      {
                        type: "link" as const,
                        text: "Operator events",
                        href: "#/operator-events",
                      },
                    ]
                  : []),
                { type: "link", text: "User API keys", href: "#/user-keys" },
                { type: "link", text: "Settings", href: "#/settings" },
              ]}
            />
          </div>
        }
        notifications={
          <Flashbar
            items={[
              ...(stale && !staleDismissed
                ? [
                    {
                      type: "info" as const,
                      header: "A new version is available",
                      content: "Refresh to get the latest features and fixes.",
                      action: (
                        <Button onClick={() => window.location.reload()}>
                          Refresh now
                        </Button>
                      ),
                      dismissible: true,
                      onDismiss: () => setStaleDismissed(true),
                      id: "__stale__",
                    },
                  ]
                : []),
              ...flashes,
            ]}
            stackItems
          />
        }
        content={<Outlet />}
        contentType="default"
      />
    </FlashContext.Provider>
  )
}

export function App() {
  return (
    <BrowserRouter>
      <FeatureFlagProvider>
        <Routes>
          <Route element={<Shell />}>
            <Route index element={<Navigate to="/pools" replace />} />
            <Route path="/pools" element={<PoolsList />} />
            <Route path="/pools/new" element={<PoolNew />} />
            {/* Templates feature removed — keep old links working. */}
            <Route
              path="/pools/templates"
              element={<Navigate to="/pools" replace />}
            />
            <Route path="/pools/:namespace/:name" element={<PoolDetail />} />
            <Route path="/nodes" element={<NodesList />} />
            <Route path="/operator-events" element={<OperatorEvents />} />
            <Route
              path="/operator-logs"
              element={<Navigate to="/operator-events" replace />}
            />
            <Route path="/api-keys" element={<ApiKeys />} />
            <Route path="/user-keys" element={<UserApiKeys />} />
            <Route path="/settings" element={<Settings />} />
            <Route
              path="/pools/:namespace/:poolName/claims/:claimName"
              element={<ClaimDetail />}
            />
            <Route
              path="/pools/:namespace/:poolName/replicas/:vmName"
              element={<PoolReplicaDetail />}
            />
            {/* Back-compat for old bookmarks. */}
            <Route path="/modules" element={<Navigate to="/pools" replace />} />
            <Route
              path="/modules/new"
              element={<Navigate to="/pools/new" replace />}
            />
            <Route path="/modules/:name" element={<RedirectModule />} />
            {/* Back-compat: old /pools/:name URLs (no namespace) redirect to list. */}
            <Route path="/pools/:name" element={<Navigate to="/pools" replace />} />
          </Route>
        </Routes>
      </FeatureFlagProvider>
    </BrowserRouter>
  )
}

function RedirectModule() {
  const path = window.location.pathname.replace(/^\/modules/, "/pools")
  return <Navigate to={path} replace />
}
