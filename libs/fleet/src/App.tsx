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
import {
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Helmet } from "react-helmet-async";
import { PoolsList } from "./pages/PoolsList";
import { ClaimDetail } from "./pages/ClaimDetail";
import { InstanceDetail } from "./pages/InstanceDetail";
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
import cuaMark from "@cua/design/assets/brand/cua-mark-white.svg";
import designTokens from "@cua/design/tokens.json";
import { localVisualPreviewPath } from "./local-visual-preview";
import { ChatThreadsProvider } from "./chat/ChatThreadsContext";
import {
  isThreadNavigationHref,
  useThreadNavigation,
} from "./components/ThreadNavigation";
import { useI18n } from "./i18n/I18nProvider";

const VERSION_CHECK_INTERVAL_MS = 60_000;

// Cloudscape icon paths (@cloudscape-design/components/icon), inlined as
// real DOM nodes so they inherit `currentColor` from the nav link's text
// color — matches hover/active states without a second theming pass.
const SVG_OPEN =
  '<svg viewBox="0 0 16 16" xmlns="http://www.w3.org/2000/svg" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round">';
const NAV_ICONS: Record<string, string> = {
  "grid-view": `${SVG_OPEN}<path d="M6 10H2v4h4v-4ZM14 10h-4v4h4v-4ZM6 2H2v4h4V2ZM14 2h-4v4h4V2Z"/></svg>`,
  history: `${SVG_OPEN}<path d="M1 0v5l5-.04"/><path d="M1 8c0 3.87 3.13 7 7 7s7-3.13 7-7-3.13-7-7-7C5.21 1 2.8 2.63 1.67 5"/><path d="M9 4v5H5"/></svg>`,
  key: `${SVG_OPEN}<path d="M10 1a5.002 5.002 0 0 0-4.6 6.96L1 12.36v2.65h4v-2h3v-2.42c.61.27 1.29.42 2 .42 2.76 0 5-2.24 5-5s-2.24-5-5-5V1Z"/><path d="M10.5 7a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3Z" fill="currentColor" stroke="none"/></svg>`,
  settings: `${SVG_OPEN}<path d="M6.11 1.729c.07-.42.44-.729.86-.729h2.02c.43 0 .79.31.86.729l.17.999c.05.29.24.529.5.679.06.03.11.06.17.1.25.15.56.2.84.1l.95-.35c.4-.15.85 0 1.07.38l1.01 1.747c.21.37.13.839-.2 1.108l-.78.64c-.23.189-.34.479-.33.768v.2c0 .29.11.579.33.769l.78.639c.33.27.42.739.2 1.108l-1.01 1.748c-.21.37-.66.529-1.06.38l-.95-.35a.966.966 0 0 0-.84.1c-.06.03-.11.07-.17.1-.26.14-.45.389-.5.679l-.17.998A.878.878 0 0 1 9 15H6.98a.87.87 0 0 1-.86-.729l-.17-.998a.988.988 0 0 0-.5-.68c-.06-.03-.11-.06-.17-.1a.996.996 0 0 0-.84-.1l-.95.35c-.4.15-.85 0-1.06-.38l-1.01-1.747a.873.873 0 0 1 .2-1.108l.78-.64c.23-.189.34-.479.33-.768v-.2c0-.3-.11-.579-.33-.769l-.78-.639a.861.861 0 0 1-.2-1.108l1.01-1.748c.21-.37.66-.529 1.07-.38l.95.35c.28.1.58.06.84-.1.06-.03.11-.07.17-.1.26-.14.45-.379.5-.678l.15-1Z"/><path d="M10 8c0 1.1-.9 2-2 2s-2-.9-2-2 .9-2 2-2 2 .9 2 2Z"/></svg>`,
  flag: `${SVG_OPEN}<path d="M1.99 16V1M2 2.14c4 2.71 8-2.99 12-.28v7.28c-4-2.89-8 2.61-12-.28"/></svg>`,
  "add-plus": `${SVG_OPEN}<path d="M2.01 8h12M8 14l.01-12"/></svg>`,
  folder: `${SVG_OPEN}<path d="M15 5v9H2V2h6l1 2h5c.55 0 1 .45 1 1Z"/></svg>`,
};

function TitledPage({ title, children }: { title: string; children: ReactNode }) {
  return (
    <>
      <Helmet>
        <title>{title} · Cua</title>
      </Helmet>
      {children}
    </>
  );
}

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
  const { t } = useI18n();
  const user = userInfo();
  const { admin, chat } = useFeatureFlags();
  const threadNavigation = useThreadNavigation();
  const tabletOrWider = useMediaQuery(
    `(min-width: ${designTokens.layout.breakpoint.tablet}px)`,
  );
  const [navigationOpen, setNavigationOpen] = useState(
    () => window.innerWidth >= designTokens.layout.breakpoint.tablet,
  );

  useEffect(() => {
    if (tabletOrWider) setNavigationOpen(true);
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
    const iconsByHrefPrefix: Array<[string, string]> = [
      ["#/pools", NAV_ICONS["grid-view"]],
      ["#/usage", NAV_ICONS.history],
      ["#/user-keys", NAV_ICONS.key],
      ["#/settings", NAV_ICONS.settings],
      ["#/admin/feature-flags", NAV_ICONS.flag],
      ["#/agent/new", NAV_ICONS["add-plus"]],
      ["#/agent/archived", NAV_ICONS.folder],
    ];
    for (const link of document.querySelectorAll<HTMLAnchorElement>(
      ".cua-shell__nav a",
    )) {
      if (link.querySelector(".cua-nav-icon")) continue;
      const href = link.getAttribute("href") ?? "";
      const icon = iconsByHrefPrefix.find(([prefix]) => href.startsWith(prefix))?.[1];
      if (!icon) continue;
      const span = document.createElement("span");
      span.className = "cua-nav-icon";
      span.innerHTML = icon;
      link.insertBefore(span, link.firstChild);
    }
  });

  return (
    <div className="cua-dashboard-theme cua-shell">
      <div id="cua-shell-topnav" className="cua-shell__topnav">
        <TopNavigation
          identity={{
            href: "#/",
            title: "Cua Fleets",
            logo: { src: cuaMark, alt: "Cua" },
          }}
          utilities={[
            {
              type: "menu-dropdown",
              text: user.name ?? user.email ?? t("account.fallback"),
              iconName: "user-profile",
              items: [{ id: "signout", text: t("common.signOut") }],
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
          navigation: t("navigation.main"),
          navigationClose: t("navigation.close"),
          navigationToggle: t("navigation.open"),
        }}
        toolsHide
        navigationOpen={navigationOpen}
        onNavigationChange={({ detail }) => {
          // Desktop/tablet stays pinned open; only mobile's drawer toggle
          // (shown via CSS below designTokens.layout.breakpoint.tablet)
          // can actually close it.
          if (tabletOrWider) return;
          setNavigationOpen(detail.open);
        }}
        navigation={
          <div className="cua-shell__nav">
            <SideNavigation
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
                { type: "link", text: t("navigation.pools"), href: "#/pools" },
                { type: "link", text: t("navigation.usage"), href: "#/usage" },
                { type: "link", text: t("navigation.apiKeys"), href: "#/user-keys" },
                { type: "link", text: t("navigation.settings"), href: "#/settings" },
                ...(admin
                  ? [
                      {
                        type: "link" as const,
                        text: t("navigation.featureFlags"),
                        href: "#/admin/feature-flags",
                        info: <Badge color="blue">{t("common.admin")}</Badge>,
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
                      header: t("stale.header"),
                      content: t("stale.content"),
                      action: (
                        <Button onClick={() => window.location.reload()}>
                          {t("common.refreshNow")}
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
  const { t } = useI18n();
  return (
    <BrowserRouter>
      <FeatureFlagProvider>
        <Routes>
          <Route element={<Shell />}>
            <Route index element={<Navigate to="/pools" replace />} />
            <Route
              path="/pools"
              element={
                <TitledPage title={t("page.pools")}>
                  <PoolsList />
                </TitledPage>
              }
            />
            <Route
              path="/pools/new"
              element={
                <TitledPage title={t("page.newPool")}>
                  <PoolNew />
                </TitledPage>
              }
            />
            <Route
              path="/pools/templates"
              element={<Navigate to="/pools" replace />}
            />
            <Route
              path="/pools/:namespace/:name"
              element={
                <TitledPage title={t("page.poolDetails")}>
                  <PoolDetail />
                </TitledPage>
              }
            />
            <Route
              path="/user-keys"
              element={
                <TitledPage title={t("navigation.apiKeys")}>
                  <UserApiKeys />
                </TitledPage>
              }
            />
            <Route
              path="/settings"
              element={
                <TitledPage title={t("page.settings")}>
                  <Settings />
                </TitledPage>
              }
            />
            <Route
              path="/usage"
              element={
                <TitledPage title={t("page.usage")}>
                  <BillingUsagePage />
                </TitledPage>
              }
            />
            <Route
              path="/agent"
              element={
                <TitledPage title={t("page.threads")}>
                  <ChatRoute />
                </TitledPage>
              }
            />
            <Route
              path="/agent/archived"
              element={
                <TitledPage title={t("page.threads")}>
                  <ArchivedThreadsRoute />
                </TitledPage>
              }
            />
            <Route
              path="/agent/:threadId"
              element={
                <TitledPage title={t("page.threads")}>
                  <ChatRoute />
                </TitledPage>
              }
            />
            <Route
              path="/billing"
              element={<Navigate to="/settings" replace />}
            />
            <Route element={<AdminRoute />}>
              <Route
                path="/admin/feature-flags"
                element={
                  <TitledPage title={t("navigation.featureFlags")}>
                    <FeatureFlags />
                  </TitledPage>
                }
              />
            </Route>
            <Route
              path="/pools/:namespace/:poolName/claims/:claimName"
              element={
                <TitledPage title={t("page.claimDetails")}>
                  <ClaimDetail />
                </TitledPage>
              }
            />
            <Route
              path="/pools/:namespace/:poolName/instances/:instanceName"
              element={<InstanceDetail />}
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
  const { t } = useI18n();
  if (!resolved) {
    return (
      <PageShell eyebrow="Agent" title={t("page.chat")}>
        <div className="agent-chat-page" />
      </PageShell>
    );
  }
  return chat ? <AgentChat /> : <Navigate to="/pools" replace />;
}

function ArchivedThreadsRoute() {
  const { chat, resolved } = useFeatureFlags();
  const { t } = useI18n();
  if (!resolved)
    return (
      <PageShell eyebrow="Agent" title={t("page.archivedThreads")}>
        <div />
      </PageShell>
    );
  return chat ? <ArchivedThreads /> : <Navigate to="/pools" replace />;
}
