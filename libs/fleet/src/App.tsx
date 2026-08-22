import {
  BrowserRouter,
  Navigate,
  Outlet,
  Route,
  Routes,
  useLocation,
  useNavigate,
} from "react-router-dom";
import AppLayout from "@cloudscape-design/components/app-layout";
import Badge from "@cloudscape-design/components/badge";
import Button from "@cloudscape-design/components/button";
import SideNavigation from "@cloudscape-design/components/side-navigation";
import TopNavigation from "@cloudscape-design/components/top-navigation";
import Flashbar, {
  type FlashbarProps,
} from "@cloudscape-design/components/flashbar";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { PoolsList } from "./pages/PoolsList";
import { ClaimDetail } from "./pages/ClaimDetail";
import { PoolDetail } from "./pages/PoolDetail";
import { PoolNew } from "./pages/PoolNew";
import { UserApiKeys } from "./pages/UserApiKeys";
import { Settings } from "./pages/Settings";
import { AgentChat } from "./pages/AgentChat";
import { ArchivedThreads } from "./pages/ArchivedThreads";
import { BillingUsagePage } from "./pages/BillingUsage";
import { PageShell } from "./components/PageShell";
import { FeatureFlags } from "./pages/FeatureFlags";
import {
  FeatureFlagProvider,
  useFeatureFlags,
} from "./components/FeatureFlagContext";
import { AdminRoute } from "./components/AdminRoute";
import { FlashContext, type FlashMsg } from "./components/FlashContext";
import { logout, userInfo } from "./auth/keycloak";
import cuaLockup from "@cua/design/assets/brand/cua-lockup-white.svg";
import designTokens from "@cua/design/tokens.json";
import { localVisualPreviewPath } from "./local-visual-preview";
import { ChatThreadsProvider } from "./chat/ChatThreadsContext";
import {
  isThreadNavigationHref,
  useThreadNavigation,
} from "./components/ThreadNavigation";

const VERSION_CHECK_INTERVAL_MS = 60_000;

function useMediaQuery(query: string): boolean {
  const [matches, setMatches] = useState(
    () => window.matchMedia(query).matches,
  );
  useEffect(() => {
    const media = window.matchMedia(query);
    const update = () => setMatches(media.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, [query]);
  return matches;
}

function useStaleCheck(): { stale: boolean } {
  const [stale, setStale] = useState(false);
  const currentEntry = useRef<string | null>(null);

  useEffect(() => {
    if (stale) return;

    // The entry script is what's actually executing; lazy chunk hashes
    // changing don't make the running app stale on their own.
    const entry = Array.from(
      document.querySelectorAll<HTMLScriptElement>(
        'script[type="module"][src]',
      ),
    )
      .map((s) => s.src)
      .find((src) => src.includes("/assets/"));
    if (!entry) return;
    currentEntry.current = entry;

    const check = async () => {
      try {
        // High-signal: if the CDN no longer serves our entry chunk,
        // the deploy invalidated us — next dynamic import would fail anyway.
        const head = await fetch(currentEntry.current!, {
          method: "HEAD",
          cache: "no-cache",
        });
        if (head.status === 404) {
          setStale(true);
          return;
        }

        const res = await fetch("/", { cache: "no-cache" });
        const html = await res.text();
        const match = html.match(
          /<script[^>]+type="module"[^>]+src="(\/assets\/[^"']+)"/,
        );
        const latestEntry = match?.[1];
        if (latestEntry && !currentEntry.current!.endsWith(latestEntry)) {
          setStale(true);
        }
      } catch {
        // Network error — ignore
      }
    };

    const id = window.setInterval(check, VERSION_CHECK_INTERVAL_MS);
    return () => window.clearInterval(id);
  }, [stale]);

  return { stale };
}

function Shell() {
  const [flashes, setFlashes] = useState<FlashbarProps.MessageDefinition[]>([]);
  const pushFlash = useCallback((msg: FlashMsg) => {
    const id = crypto.randomUUID();
    const dismiss = () =>
      setFlashes((prev) => prev.filter((flash) => flash.id !== id));
    setFlashes((prev) => [
      ...prev,
      { ...msg, id, dismissible: true, onDismiss: dismiss },
    ]);
    if (msg.type === "success") window.setTimeout(dismiss, 5000);
  }, []);
  const flashContext = useMemo(() => ({ push: pushFlash }), [pushFlash]);

  return (
    <FlashContext.Provider value={flashContext}>
      <ChatThreadsProvider>
        <ShellLayout flashes={flashes} />
      </ChatThreadsProvider>
    </FlashContext.Provider>
  );
}

function ShellLayout({
  flashes,
}: {
  flashes: FlashbarProps.MessageDefinition[];
}) {
  const location = useLocation();
  const navigate = useNavigate();
  const [staleDismissed, setStaleDismissed] = useState(false);
  const { stale } = useStaleCheck();
  const [navigationOpen, setNavigationOpen] = useState(
    () => window.innerWidth >= designTokens.layout.breakpoint.tablet,
  );
  const tabletOrWider = useMediaQuery(
    `(min-width: ${designTokens.layout.breakpoint.tablet}px)`,
  );
  const mobile = useMediaQuery(
    `(max-width: ${designTokens.layout.breakpoint.mobile - 1}px)`,
  );
  const previousPath = useRef(location.pathname);
  const user = userInfo();
  const { admin, chat } = useFeatureFlags();
  const threadNavigation = useThreadNavigation();

  useEffect(() => {
    setNavigationOpen(tabletOrWider);
  }, [tabletOrWider]);

  useEffect(() => {
    const titlesByHref = new Map(
      threadNavigation.threadTitles.map(({ href, title }) => [href, title]),
    );
    for (const link of document.querySelectorAll<HTMLAnchorElement>(
      ".cua-shell__nav a",
    )) {
      const title = titlesByHref.get(link.getAttribute("href") ?? "");
      if (title) link.title = title;
    }
  }, [threadNavigation.threadTitles]);

  useEffect(() => {
    const changed = previousPath.current !== location.pathname;
    if (changed && mobile && location.pathname.startsWith("/agent")) {
      setNavigationOpen(false);
    }
    previousPath.current = location.pathname;
  }, [location.pathname, mobile]);

  useEffect(() => {
    const path = location.pathname;
    const pageTitle =
      path === "/usage"
        ? "Usage"
        : path.startsWith("/agent")
          ? "Threads"
          : path === "/user-keys"
            ? "User API keys"
            : path === "/settings"
              ? "Settings"
              : path === "/pools/new"
                ? "New pool"
                : path.includes("/claims/")
                  ? "Claim details"
                  : path.startsWith("/pools/")
                    ? "Pool details"
                    : "Pools";
    document.title = `${pageTitle} · Cua`;
  }, [location.pathname]);

  return (
    <div className="cua-dashboard-theme cua-shell">
      <div id="cua-shell-topnav" className="cua-shell__topnav">
        <TopNavigation
          identity={{
            href: "#/",
            logo: { src: cuaLockup, alt: "Cua" },
          }}
          utilities={[
            {
              type: "menu-dropdown",
              text: user.name ?? user.email ?? "Account",
              iconName: "user-profile",
              items: [{ id: "signout", text: "Sign out" }],
              onItemClick: (event) => {
                if (event.detail.id === "signout") logout();
              },
            },
          ]}
        />
      </div>
      <AppLayout
        disableBodyScroll={location.pathname.startsWith("/agent")}
        headerSelector="#cua-shell-topnav"
        ariaLabels={{
          navigation: "Main navigation",
          navigationClose: "Close navigation",
          navigationToggle: "Open navigation",
        }}
        toolsHide
        navigationOpen={navigationOpen}
        onNavigationChange={({ detail }) => setNavigationOpen(detail.open)}
        navigation={
          <div className="cua-shell__nav">
            <SideNavigation
              header={mobile ? undefined : { href: "#/pools", text: "Cua" }}
              activeHref={`#${location.pathname}`}
              onFollow={(event) => {
                if (event.detail.external) return;
                event.preventDefault();
                const path = event.detail.href.replace(/^#/, "");
                if (
                  isThreadNavigationHref(event.detail.href) &&
                  threadNavigation.generationLocked
                )
                  return;
                if (path === "/agent/new") {
                  if (threadNavigation.isCreatePending()) return;
                  void threadNavigation.createThread();
                  return;
                }
                if (path === "/agent/retry") return;
                navigate(localVisualPreviewPath(path));
              }}
              items={[
                { type: "link", text: "Pools", href: "#/pools" },
                { type: "link", text: "Usage", href: "#/usage" },
                { type: "link", text: "User API keys", href: "#/user-keys" },
                { type: "link", text: "Settings", href: "#/settings" },
                ...(admin
                  ? [
                      {
                        type: "link" as const,
                        text: "Feature flags",
                        href: "#/admin/feature-flags",
                        info: <Badge color="blue">Admin</Badge>,
                      },
                    ]
                  : []),
                ...(chat
                  ? [{ type: "divider" as const }, ...threadNavigation.items]
                  : []),
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
    </div>
  );
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
            <Route
              path="/pools/templates"
              element={<Navigate to="/pools" replace />}
            />
            <Route path="/pools/:namespace/:name" element={<PoolDetail />} />
            <Route path="/user-keys" element={<UserApiKeys />} />
            <Route path="/settings" element={<Settings />} />
            <Route path="/usage" element={<BillingUsagePage />} />
            <Route path="/agent" element={<ChatRoute />} />
            <Route path="/agent/archived" element={<ArchivedThreadsRoute />} />
            <Route path="/agent/:threadId" element={<ChatRoute />} />
            <Route
              path="/billing"
              element={<Navigate to="/settings" replace />}
            />
            <Route element={<AdminRoute />}>
              <Route path="/admin/feature-flags" element={<FeatureFlags />} />
            </Route>
            <Route
              path="/pools/:namespace/:poolName/claims/:claimName"
              element={<ClaimDetail />}
            />
            <Route path="/modules" element={<Navigate to="/pools" replace />} />
            <Route
              path="/modules/new"
              element={<Navigate to="/pools/new" replace />}
            />
            <Route path="/modules/:name" element={<RedirectModule />} />
            <Route path="*" element={<Navigate to="/pools" replace />} />
            <Route
              path="/pools/:name"
              element={<Navigate to="/pools" replace />}
            />
          </Route>
        </Routes>
      </FeatureFlagProvider>
    </BrowserRouter>
  );
}

function RedirectModule() {
  const path = window.location.pathname.replace(/^\/modules/, "/pools");
  return <Navigate to={path} replace />;
}

function ChatRoute() {
  const { chat, resolved } = useFeatureFlags();
  if (!resolved) {
    return (
      <PageShell eyebrow="Agent" title="Chat">
        <div className="agent-chat-page" />
      </PageShell>
    );
  }
  return chat ? <AgentChat /> : <Navigate to="/pools" replace />;
}

function ArchivedThreadsRoute() {
  const { chat, resolved } = useFeatureFlags();
  if (!resolved)
    return (
      <PageShell eyebrow="Agent" title="Archived threads">
        <div />
      </PageShell>
    );
  return chat ? <ArchivedThreads /> : <Navigate to="/pools" replace />;
}
