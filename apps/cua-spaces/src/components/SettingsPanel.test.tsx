// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as appCore from "../core";
import type { TelemetryBridge, TelemetryView } from "../native/telemetry";
import { fakeDrive, MOUNT_OFF, MOUNTED, NEEDS_APPROVAL } from "../test/fakeDrive";
import { SettingsPanel, SIGN_IN_URL } from "./SettingsPanel";

beforeEach(() => {
  window.localStorage.clear();
});

function renderPanel(overrides: Partial<Parameters<typeof SettingsPanel>[0]> = {}) {
  const props = {
    menuBar: false,
    onMenuBarMode: vi.fn(),
    fleetLive: false,
    clientId: undefined,
    onOpenExternal: vi.fn(),
    onClose: vi.fn(),
    ...overrides,
  };
  render(<SettingsPanel {...props} />);
  return props;
}

describe("SettingsPanel notch tab control", () => {
  it("reflects notch mode by default and drives setMenuBarMode", async () => {
    const user = userEvent.setup();
    const props = renderPanel({ menuBar: false });

    const menuBar = screen.getByRole("radio", { name: "Hide" });
    const notch = screen.getByRole("radio", { name: "Show" });
    expect(notch).toBeChecked();
    expect(menuBar).not.toBeChecked();

    await user.click(menuBar);
    expect(props.onMenuBarMode).toHaveBeenCalledWith(true);

    await user.click(notch);
    expect(props.onMenuBarMode).toHaveBeenCalledWith(false);
  });

  it("reflects menu-bar mode when enabled", () => {
    renderPanel({ menuBar: true });
    expect(screen.getByRole("radio", { name: "Hide" })).toBeChecked();
    expect(screen.getByRole("radio", { name: "Show" })).not.toBeChecked();
  });
});

describe("SettingsPanel account section", () => {
  it("shows a sign-in button that opens Cua when not live", async () => {
    const user = userEvent.setup();
    const props = renderPanel({ fleetLive: false });
    const button = screen.getByRole("button", { name: "Sign in to Cua" });
    await user.click(button);
    expect(props.onOpenExternal).toHaveBeenCalledWith(SIGN_IN_URL);
  });

  it("shows the API-key state but still offers account sign-in when env creds are live", () => {
    renderPanel({ fleetLive: true, clientId: "client-abc" });
    // The env machine key reads as "signed in via API key" with the client id…
    expect(screen.getByText("Signed in via API key")).toBeInTheDocument();
    expect(screen.getByText("client-abc")).toBeInTheDocument();
    // …but it does NOT masquerade as a Cua account, and account sign-in stays available.
    expect(screen.queryByText("Signed in to Cua")).toBeNull();
    expect(screen.getByRole("button", { name: "Sign in to Cua" })).toBeInTheDocument();
  });
});

describe("SettingsPanel default location", () => {
  it("offers no default-location choice while the apps do not offer Cua Cloud", () => {
    renderPanel({
      defaultLocation: { value: "cloud", source: "env", env: "CUA_DEFAULT_ON", path: "/tmp/cua/config.toml" },
      onDefaultLocation: vi.fn(),
    });
    expect(screen.queryByRole("radiogroup", { name: "New Spaces run on" })).toBeNull();
    expect(screen.queryByText(/Cua Cloud/)).toBeNull();
  });
});

describe("SettingsPanel device-flow sign-in", () => {
  /** A mock `AccountAuth` whose event handlers can be fired by the test. */
  function mockAuth() {
    let signedIn: ((identity?: string) => void) | undefined;
    let failed: ((reason: string) => void) | undefined;
    let signedOut: (() => void) | undefined;
    const auth = {
      beginSignIn: vi.fn(async () => ({
        userCode: "WXYZ-1234",
        verificationUri: "https://auth.cua.ai/device?user_code=WXYZ-1234",
      })),
      signOut: vi.fn(async () => {}),
      onSignedIn: (h: (identity?: string) => void) => {
        signedIn = h;
        return () => {};
      },
      onSignInFailed: (h: (reason: string) => void) => {
        failed = h;
        return () => {};
      },
      onSignedOut: (h: () => void) => {
        signedOut = h;
        return () => {};
      },
    };
    return {
      auth,
      emitSignedIn: (id?: string) => signedIn?.(id),
      emitFailed: (reason: string) => failed?.(reason),
      emitSignedOut: () => signedOut?.(),
    };
  }

  it("shows the user code prominently after starting sign-in", async () => {
    const user = userEvent.setup();
    const { auth } = mockAuth();
    renderPanel({ auth });

    await user.click(screen.getByRole("button", { name: "Sign in to Cua" }));

    expect(auth.beginSignIn).toHaveBeenCalled();
    expect(await screen.findByText("WXYZ-1234")).toBeInTheDocument();
    expect(screen.getByText("Enter this code in your browser")).toBeInTheDocument();
  });

  it("asks to finish in the browser when the SDK used browser sign-in", async () => {
    const user = userEvent.setup();
    const { auth } = mockAuth();
    auth.beginSignIn.mockResolvedValueOnce({
      method: "browser",
      verificationUri: "https://auth.cua.ai/realms/x/auth?client_id=cua-cli",
    } as never);
    renderPanel({ auth });

    await user.click(screen.getByRole("button", { name: "Sign in to Cua" }));

    expect(await screen.findByText("Finish signing in in your browser.")).toBeInTheDocument();
    expect(screen.queryByText(/Enter this code/)).toBeNull();
  });

  it("flips to the signed-in view when the signed-in event arrives", async () => {
    const user = userEvent.setup();
    const { auth, emitSignedIn } = mockAuth();
    renderPanel({ auth });

    await user.click(screen.getByRole("button", { name: "Sign in to Cua" }));
    await screen.findByText("WXYZ-1234");

    await act(async () => emitSignedIn("user@cua.ai"));

    expect(screen.getByText("user@cua.ai")).toBeInTheDocument();
    expect(screen.queryByText("WXYZ-1234")).toBeNull();
    expect(screen.getByRole("button", { name: "Sign out" })).toBeInTheDocument();
  });

  it("signs out and returns to the signed-out view", async () => {
    const user = userEvent.setup();
    const { auth, emitSignedOut } = mockAuth();
    renderPanel({ auth, signedInIdentity: "user@cua.ai" });

    expect(screen.getByText("user@cua.ai")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Sign out" }));
    expect(auth.signOut).toHaveBeenCalled();

    await act(async () => emitSignedOut());
    expect(screen.getByRole("button", { name: "Sign in to Cua" })).toBeInTheDocument();
    expect(screen.queryByText("user@cua.ai")).toBeNull();
  });

  it("surfaces a sign-in failure with a retry", async () => {
    const user = userEvent.setup();
    const { auth, emitFailed } = mockAuth();
    renderPanel({ auth });

    await user.click(screen.getByRole("button", { name: "Sign in to Cua" }));
    await screen.findByText("WXYZ-1234");

    await act(async () => emitFailed("access denied"));

    expect(screen.getByRole("alert")).toHaveTextContent("access denied");
    expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
  });
});

describe("SettingsPanel dismissal", () => {
  it("closes on Escape (the window's toolbar carries the title)", async () => {
    const user = userEvent.setup();
    const props = renderPanel();
    expect(screen.queryByRole("button", { name: "Back" })).toBeNull();
    await user.keyboard("{Escape}");
    expect(props.onClose).toHaveBeenCalled();
  });
});

describe("SettingsPanel AI agents", () => {
  // The rows come from the SDK's agent onboarding (cua-agent-setup) in one
  // read-only call on mount; actions return the rows they changed.
  function rowOf(agent: string, name: string, over: Partial<AgentRowT> = {}): AgentRowT {
    return {
      agent,
      name,
      installed: true,
      configured: false,
      detail: "skills 0/4, MCP not configured",
      skillsInstalled: 0,
      skillsTotal: 4,
      mcpConfig: `/home/u/.${agent}/mcp.json`,
      skillsDir: "/home/u/.agents/skills",
      ...over,
    };
  }
  type AgentRowT = import("../native/agentConfig").AgentRow;

  const rows = (): AgentRowT[] => [
    rowOf("codex", "OpenAI Codex", { configured: true, detail: "skills and MCP configured", skillsInstalled: 4 }),
    rowOf("claude-code", "Claude Code"),
    rowOf("antigravity", "Google Antigravity", { installed: false, detail: "not installed" }),
  ];

  let mocks: Record<string, ReturnType<typeof vi.fn>>;

  function mockAgentConfig(over: Record<string, unknown> = {}) {
    // Drop the statically-imported copy first, so the dynamic import in
    // renderFresh picks up this mock rather than the module already loaded at
    // the top of this file.
    vi.resetModules();
    mocks = {
      detectAgents: vi.fn(async () => rows()),
      configureAgents: vi.fn(async (agents?: string[]) =>
        rows()
          .filter((r) => r.installed && (!agents || agents.includes(r.agent)))
          .map((r) => ({ ...r, configured: true, detail: "skills and MCP configured", skillsInstalled: 4 })),
      ),
      removeAgents: vi.fn(async (agents: string[]) =>
        rows()
          .filter((r) => agents.includes(r.agent))
          .map((r) => ({ ...r, configured: false, detail: "skills 0/4, MCP not configured" })),
      ),
      listTeleportableApps: vi.fn(async () => []),
      setTeleportPolicy: vi.fn(async () => {}),
    };
    vi.doMock("../native/agentConfig", () => ({ ...mocks, ...over }));
    // The freshly imported panel shares the app core already loaded.
    vi.doMock("../core", () => appCore);
  }

  afterEach(() => {
    vi.resetModules();
    vi.doUnmock("../native/agentConfig");
    vi.doUnmock("../core");
  });

  async function renderFresh() {
    const { SettingsPanel: Panel } = await import("./SettingsPanel");
    await act(async () => {
      render(
        <Panel
          menuBar={false}
          onMenuBarMode={vi.fn()}
          fleetLive={false}
          clientId={undefined}
          onOpenExternal={vi.fn()}
          onClose={vi.fn()}
        />,
      );
    });
  }

  it("detects every agent on mount, with no button press", async () => {
    mockAgentConfig();
    await renderFresh();

    expect(await screen.findByText("OpenAI Codex")).toBeInTheDocument();
    expect(screen.getByText("Claude Code")).toBeInTheDocument();
    expect(screen.getByText("Google Antigravity")).toBeInTheDocument();
    // Not installed: dimmed, no status text.
    expect(screen.queryByText("not installed")).toBeNull();
    expect(screen.getByText("skills and MCP configured")).toBeInTheDocument();
    expect(mocks.detectAgents).toHaveBeenCalledTimes(1);
  });

  it("offers Configure for an installed, unconfigured agent and Remove for a configured one", async () => {
    mockAgentConfig();
    await renderFresh();
    await screen.findByText("OpenAI Codex");

    expect(screen.getAllByRole("button", { name: "Configure" })).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: "Remove" })).toHaveLength(1);
  });

  it("configures one agent through the SDK and reflects it in that row", async () => {
    const user = userEvent.setup();
    mockAgentConfig();
    await renderFresh();
    await screen.findByText("OpenAI Codex");

    await act(async () => {
      await user.click(screen.getByRole("button", { name: "Configure" }));
    });

    expect(mocks.configureAgents).toHaveBeenCalledWith(["claude-code"]);
    expect(screen.queryByRole("button", { name: "Configure" })).toBeNull();
    expect(screen.getAllByRole("button", { name: "Remove" })).toHaveLength(2);
  });

  it("removes what cua added for one agent", async () => {
    const user = userEvent.setup();
    mockAgentConfig();
    await renderFresh();
    await screen.findByText("OpenAI Codex");

    await act(async () => {
      await user.click(screen.getByRole("button", { name: "Remove" }));
    });

    expect(mocks.removeAgents).toHaveBeenCalledWith(["codex"]);
    expect(screen.getAllByRole("button", { name: "Configure" })).toHaveLength(2);
  });

  it("configures all detected agents in one call", async () => {
    const user = userEvent.setup();
    mockAgentConfig();
    await renderFresh();
    const all = await screen.findByRole("button", { name: "Configure all detected agents" });

    await act(async () => {
      await user.click(all);
    });

    expect(mocks.configureAgents).toHaveBeenCalledWith();
    expect(screen.queryByRole("button", { name: "Configure" })).toBeNull();
  });

  it("surfaces an SDK error instead of hiding it", async () => {
    const user = userEvent.setup();
    mockAgentConfig({
      configureAgents: vi.fn(async () => {
        throw new Error("~/.claude.json was left unchanged: JSON parse error");
      }),
    });
    await renderFresh();
    await screen.findByText("OpenAI Codex");

    await act(async () => {
      await user.click(screen.getByRole("button", { name: "Configure" }));
    });

    expect(await screen.findByRole("alert")).toHaveTextContent("left unchanged");
  });
});

describe("SettingsPanel Privacy", () => {
  function fakeTelemetry(over: Partial<TelemetryView> = {}): TelemetryBridge & { calls: string[] } {
    let view: TelemetryView = {
      enabled: true,
      source: "default",
      sourceKind: "default",
      noticeShown: true,
      noticeText: "",
      docsUrl: "https://cua.ai/docs/cua-sdk/concepts/telemetry",
      ...over,
    };
    const calls: string[] = [];
    return {
      isNative: true,
      calls,
      status: async () => view,
      acknowledgeNotice: async () => {
        calls.push("ack");
        return view;
      },
      setEnabled: async (enabled) => {
        calls.push(`set:${enabled}`);
        view = { ...view, enabled, source: "config /tmp/cua/config.toml", sourceKind: "config" };
        return view;
      },
      recordFeature: () => {},
      recordStep: () => {},
      recordStream: () => {},
      recordSignals: () => {},
      welcomeLeft: async () => view,
    };
  }

  it("acknowledges the notice, turns usage telemetry off and links the docs", async () => {
    const telemetry = fakeTelemetry();
    const props = renderPanel({ telemetry });
    await screen.findByText("Share anonymous usage data");
    expect(telemetry.calls).toEqual(["ack"]);
    await userEvent.click(screen.getByRole("radio", { name: "Off" }));
    expect(telemetry.calls).toContain("set:false");
    expect(await screen.findByRole("radio", { name: "Off" })).toHaveAttribute("aria-checked", "true");
    await userEvent.click(screen.getByRole("button", { name: "What is collected" }));
    expect(props.onOpenExternal).toHaveBeenCalledWith("https://cua.ai/docs/cua-sdk/concepts/telemetry");
  });

  it("locks the switch when the environment decides", async () => {
    renderPanel({ telemetry: fakeTelemetry({ enabled: false, source: "env DO_NOT_TRACK", sourceKind: "do_not_track" }) });
    const on = await screen.findByRole("radio", { name: "On" });
    expect(on).toBeDisabled();
    expect(screen.getByTitle("Set by env DO_NOT_TRACK")).toBeInTheDocument();
  });

  it("lists Account, General, Privacy, AI agents and Experiments in order, with Show again", async () => {
    const onShowWelcome = vi.fn();
    renderPanel({ telemetry: fakeTelemetry(), onShowWelcome });
    await screen.findByText("Share anonymous usage data");
    const titles = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(titles).toEqual(["Account", "General", "Privacy", "AI agents", "Experiments"]);
    await userEvent.click(screen.getByRole("button", { name: "Show again" }));
    expect(onShowWelcome).toHaveBeenCalled();
  });
});

describe("SettingsPanel Experiments", () => {
  it("turns Cua Volume on: Storage shows after General, the switch is saved and recorded", async () => {
    const user = userEvent.setup();
    const view: TelemetryView = {
      enabled: true,
      source: "default",
      sourceKind: "default",
      noticeShown: true,
      noticeText: "",
      docsUrl: "https://cua.ai/docs/cua-sdk/concepts/telemetry",
    };
    const telemetry: TelemetryBridge = {
      isNative: true,
      status: async () => view,
      acknowledgeNotice: async () => view,
      setEnabled: async () => view,
      welcomeLeft: async () => view,
      recordFeature: () => {},
      recordStep: () => {},
      recordStream: () => {},
      recordSignals: vi.fn(),
    };
    renderPanel({ telemetry });
    await screen.findByText("Share anonymous usage data");
    const experiments = within(screen.getByRole("region", { name: "Experiments" }));
    for (const name of ["Cua Volume", "Your cloud", "Sharing"]) {
      expect(experiments.getByRole("switch", { name })).toHaveAttribute("aria-checked", "false");
    }
    expect(screen.queryByRole("region", { name: "Storage" })).toBeNull();
    await user.click(experiments.getByRole("switch", { name: "Cua Volume" }));
    expect(await screen.findByRole("region", { name: "Storage" })).toBeInTheDocument();
    const titles = screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(titles.slice(0, 3)).toEqual(["Account", "General", "Storage"]);
    expect(JSON.parse(window.localStorage.getItem("cua.settings.experiments") ?? "{}")).toMatchObject({
      cuaVolume: true,
    });
    expect(telemetry.recordSignals).toHaveBeenCalledWith([
      { type: "experiment", action: "experiment_on", experiment: "cua_volume" },
      { type: "experiments-on", experiments: ["cua_volume"] },
    ]);
  });
});

describe("SettingsPanel billing and Teams", () => {
  it("hides Cua Cloud billing and opens the Teams waitlist", async () => {
    const auth = {
      beginSignIn: vi.fn(),
      signOut: vi.fn(),
      onSignedIn: vi.fn(async () => () => {}),
      onSignInFailed: vi.fn(async () => () => {}),
      onSignedOut: vi.fn(async () => () => {}),
    };
    const props = renderPanel({
      fleetLive: true,
      signedInIdentity: "you@example.com",
      auth,
      billingStatus: async () => ({
        billingEnabled: true,
        card: null,
        credit: { balanceUsdCents: 742 },
        billingUrl: "https://run.cua.ai/billing",
      }),
    });
    expect(await screen.findByText("Coming soon")).toBeInTheDocument();
    expect(screen.queryByText("$7.42 credit left")).toBeNull();
    expect(screen.queryByRole("button", { name: "Manage billing" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Join the waitlist" }));
    expect(props.onOpenExternal).toHaveBeenCalledWith("https://cua.ai/teams");
  });
});

describe("SettingsPanel Storage", () => {
  const GIB = 1024 * 1024 * 1024;
  const FS = { backend: "fs", fs_path: "/Users/maya/.cua/volume/data", s3: null, has_keys: false };
  const CACHE = { size_bytes: 1288490188, capacity_bytes: 10 * GIB };
  const storageGroup = () => within(screen.getByRole("region", { name: "Storage" }));
  const CHECK_OK = { ok: true, reachable: true, authorized: true, versioning: true, detail: null, applied: true };
  // Storage shows with the Cua Volume experiment on.
  beforeEach(() => window.localStorage.setItem("cua.settings.experiments", JSON.stringify({ cuaVolume: true })));

  it("says it is not available when the daemon does not answer, and offers nothing", async () => {
    const drive = fakeDrive({});
    renderPanel({ drive: drive.bridge, os: "macos" });
    const group = storageGroup();
    expect(await group.findByText("Not available on this Mac yet")).toBeInTheDocument();
    expect(group.queryByRole("button")).toBeNull();
    expect(group.queryByRole("radio")).toBeNull();
    expect(group.queryByRole("textbox")).toBeNull();
    expect(new Set(drive.tools())).toEqual(new Set(["volume_storage", "volume_mount_status", "volume_cache_stats"]));
  });

  it("switches to an S3-compatible bucket: tests, then saves, the keys only in volume_storage_set", async () => {
    const user = userEvent.setup();
    let saved: object = FS;
    let test = { ok: false, reachable: true, authorized: true, versioning: false, detail: null, applied: false };
    const drive = fakeDrive({
      volume_storage: () => saved,
      volume_mount_status: MOUNT_OFF,
      volume_cache_stats: CACHE,
      volume_storage_set: (args) => {
        if (args.dry_run) return test;
        saved = { backend: "s3", fs_path: FS.fs_path, s3: args.s3, has_keys: true };
        return CHECK_OK;
      },
    });
    renderPanel({ drive: drive.bridge, os: "macos" });
    const group = storageGroup();
    const backend = await group.findByRole("radiogroup", { name: "Store files on" });
    expect(within(backend).getAllByRole("radio").map((r) => r.textContent)).toEqual(["This Mac", "S3-compatible"]);
    expect(within(backend).getByRole("radio", { name: "This Mac" })).toHaveAttribute("aria-checked", "true");
    // "Stored in": the path, shortened in the middle, the full path as its tooltip.
    expect(group.getByText("Stored in")).toBeInTheDocument();
    const stored = group.getByLabelText("~/.cua/volume/data");
    expect(stored).toHaveAttribute("title", "/Users/maya/.cua/volume/data");
    expect(stored.querySelector(".mid-text-tail")!.textContent).toBe("/data");
    expect(group.queryByText(/cloud/i)).toBeNull();

    await user.click(within(backend).getByRole("radio", { name: "S3-compatible" }));
    // The agent prompt first; the fields on request.
    expect(group.getByRole("textbox", { name: "Ask your agent to set it up:" })).toBeInTheDocument();
    await user.click(group.getByRole("button", { name: "Enter details manually" }));
    const endpoint = group.getByRole("textbox", { name: "Endpoint" });
    expect(endpoint).toHaveAttribute("placeholder", "AWS");
    expect(group.getByRole("textbox", { name: "Region" })).toHaveValue("us-east-1");
    await user.type(endpoint, "http://127.0.0.1:9000");
    await user.type(group.getByRole("textbox", { name: "Bucket" }), "cua-volume");
    await user.click(within(group.getByRole("radiogroup", { name: "Path-style URLs" })).getByRole("radio", { name: "On" }));
    await user.type(group.getByRole("textbox", { name: "Access key ID" }), "maya-drive");
    const secret = group.getByLabelText("Secret access key");
    expect(secret).toHaveAttribute("type", "password");
    await user.type(secret, "fixture-secret");

    await user.click(group.getByRole("button", { name: "Test connection" }));
    expect(await group.findByRole("alert")).toHaveTextContent("Bucket versioning is off");
    test = { ...test, ok: true, versioning: true };
    await user.click(group.getByRole("button", { name: "Test connection" }));
    expect(await group.findByText("Connected, versioning on")).toBeInTheDocument();

    await user.click(group.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(group.queryByRole("button", { name: "Save" })).toBeNull());
    const sets = drive.calls.filter((c) => c.tool === "volume_storage_set").map((c) => c.args);
    expect(sets).toHaveLength(3);
    expect(sets[2]).toEqual({
      backend: "s3",
      s3: { endpoint: "http://127.0.0.1:9000", region: "us-east-1", bucket: "cua-volume", root: "", path_style: true },
      access_key_id: "maya-drive",
      secret_access_key: "fixture-secret",
      dry_run: false,
    });
    expect(sets[0]).toMatchObject({ dry_run: true, secret_access_key: "fixture-secret" });
    // The secret went nowhere else: no other tool call, no settings file.
    for (const c of drive.calls.filter((c) => c.tool !== "volume_storage_set")) {
      expect(JSON.stringify(c.args)).not.toContain("fixture-secret");
    }
    for (let i = 0; i < window.localStorage.length; i++) {
      const key = window.localStorage.key(i)!;
      expect(`${key}=${window.localStorage.getItem(key)}`).not.toContain("fixture-secret");
    }
    // Saved: the form follows the daemon, the keys are cleared and saved.
    expect(group.getByLabelText("Secret access key")).toHaveValue("");
    expect(group.getByLabelText("Secret access key")).toHaveAttribute("placeholder", "Saved");
  });

  it("shows Cua Volume in Finder: mounts, opens System Settings for approval, reveals the volume", async () => {
    const user = userEvent.setup();
    let mount: object = MOUNT_OFF;
    const drive = fakeDrive({
      volume_storage: FS,
      volume_mount_status: () => mount,
      volume_cache_stats: CACHE,
      volume_mount: () => (mount = NEEDS_APPROVAL),
    });
    renderPanel({ drive: drive.bridge, os: "macos" });
    const group = storageGroup();
    const toggle = await group.findByRole("radiogroup", { name: "Add Cua Volume to Finder" });
    await user.click(within(toggle).getByRole("radio", { name: "On" }));
    expect(await group.findByText("Turn on Cua Volume in File System Extensions.")).toBeInTheDocument();
    expect(drive.tools()).toContain("volume_mount");
    await user.click(group.getByRole("button", { name: "Open System Settings" }));
    await waitFor(() =>
      expect(drive.opened).toEqual(["x-apple.systempreferences:com.apple.LoginItems-Settings.extension"]),
    );
    // Approved: the 2 s poll picks it up.
    mount = MOUNTED;
    await waitFor(() => expect(group.getByRole("button", { name: "Show in Finder" })).toBeEnabled(), { timeout: 3500 });
    expect(group.getByText("In Finder at")).toBeInTheDocument();
    expect(group.getByLabelText("/Volumes/Cua Volume")).toHaveAttribute("title", "/Volumes/Cua Volume");
    await user.click(group.getByRole("button", { name: "Show in Finder" }));
    await waitFor(() => expect(drive.revealed).toEqual(["/Volumes/Cua Volume"]));
  });

  it("clears the cache and sets its limit", async () => {
    const user = userEvent.setup();
    const drive = fakeDrive({
      volume_storage: FS,
      volume_mount_status: MOUNT_OFF,
      volume_cache_stats: CACHE,
      volume_cache_clear: { ...CACHE, size_bytes: 0 },
      volume_cache_set: (args) => ({ ...CACHE, capacity_bytes: args.capacity_bytes }),
    });
    renderPanel({ drive: drive.bridge, os: "macos" });
    const group = storageGroup();
    expect(await group.findByText("1.2 GB of 10 GB")).toBeInTheDocument();
    await user.click(group.getByRole("button", { name: "Clear cache" }));
    await user.click(within(group.getByRole("radiogroup", { name: "Cache size limit" })).getByRole("radio", { name: "5 GB" }));
    await waitFor(() =>
      expect(drive.calls.filter((c) => c.tool.startsWith("volume_cache_") && c.tool !== "volume_cache_stats")).toEqual([
        { tool: "volume_cache_clear", args: {} },
        { tool: "volume_cache_set", args: { capacity_bytes: 5 * GIB } },
      ]),
    );
  });

  it("shows the daemon's error when a change fails", async () => {
    const user = userEvent.setup();
    const drive = fakeDrive({
      volume_storage: FS,
      volume_mount_status: MOUNT_OFF,
      volume_cache_stats: CACHE,
      volume_mount: new Error("permission denied: the mount is user-only"),
    });
    renderPanel({ drive: drive.bridge, os: "macos" });
    const group = storageGroup();
    const toggle = await group.findByRole("radiogroup", { name: "Add Cua Volume to Finder" });
    await user.click(within(toggle).getByRole("radio", { name: "On" }));
    expect(await group.findByRole("alert")).toHaveTextContent("permission denied: the mount is user-only");
  });
});

describe("SettingsPanel Storage agent prompt", () => {
  const FS = { backend: "fs", fs_path: "/Users/maya/.cua/volume/data", s3: null, has_keys: false };
  const S3 = {
    backend: "s3",
    fs_path: FS.fs_path,
    s3: { endpoint: "http://127.0.0.1:9000", region: "us-east-1", bucket: "cua-volume", root: "", path_style: true },
    has_keys: true,
  };
  const OK = { ok: true, reachable: true, authorized: true, versioning: true, detail: null, applied: true };
  const storageGroup = () => within(screen.getByRole("region", { name: "Storage" }));
  // Storage shows with the Cua Volume experiment on.
  beforeEach(() => window.localStorage.setItem("cua.settings.experiments", JSON.stringify({ cuaVolume: true })));

  it("copies the prompt, toggles to the fields and back, and adopts the bucket the agent sets up", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    let storage: object = FS;
    const drive = fakeDrive({
      volume_storage: () => storage,
      volume_mount_status: MOUNT_OFF,
      volume_cache_stats: { size_bytes: 0, capacity_bytes: 1024 ** 3 },
      volume_storage_set: OK,
    });
    renderPanel({ drive: drive.bridge, os: "macos" });
    const group = storageGroup();
    await user.click(within(await group.findByRole("radiogroup", { name: "Store files on" })).getByRole("radio", { name: "S3-compatible" }));
    const prompt = group.getByRole("textbox", { name: "Ask your agent to set it up:" });
    expect(prompt).toHaveAttribute("readonly");
    const text = (prompt as HTMLTextAreaElement).value;
    expect(text.length).toBeGreaterThan(0);
    expect(group.queryByRole("textbox", { name: "Bucket" })).toBeNull();
    await user.click(group.getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith(text);

    await user.click(group.getByRole("button", { name: "Enter details manually" }));
    expect(group.getByRole("textbox", { name: "Bucket" })).toBeInTheDocument();
    expect(group.queryByRole("textbox", { name: "Ask your agent to set it up:" })).toBeNull();
    await user.click(group.getByRole("button", { name: "Use a prompt instead" }));
    expect(group.getByRole("textbox", { name: "Ask your agent to set it up:" })).toBeInTheDocument();

    // The agent saves the bucket and its keys; the 2 s poll notices it.
    storage = S3;
    expect(await group.findByText("Connected, versioning on", undefined, { timeout: 3500 })).toBeInTheDocument();
    const sets = drive.calls.filter((c) => c.tool === "volume_storage_set").map((c) => c.args);
    expect(sets).toEqual([
      { backend: "s3", s3: S3.s3, access_key_id: null, secret_access_key: null, dry_run: false },
    ]);
  });
});

describe("SettingsPanel launch at login", () => {
  it("shows no toggle outside the shell", () => {
    renderPanel();
    expect(screen.queryByRole("switch", { name: "Launch Cua Spaces at login" })).toBeNull();
  });

  it("shows what the system holds and turns it off and on through the login item", async () => {
    const { fakeLoginItemBridge } = await import("../native/loginItem");
    const { readLaunchChoice } = await import("../model/loginItem");
    const user = userEvent.setup();
    const loginItem = fakeLoginItemBridge("enabled");
    renderPanel({ loginItem });
    const toggle = await screen.findByRole("switch", { name: "Launch Cua Spaces at login" });
    expect(toggle).toHaveAttribute("aria-checked", "true");
    // Cua Volume is an experiment, off by default, so the note leaves it out.
    expect(screen.getByText("Keeps your Spaces and agents available after a restart.")).toBeInTheDocument();

    await user.click(toggle);
    await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "false"));
    expect(loginItem.calls).toEqual([false]);
    expect(readLaunchChoice()).toBe(false);

    await user.click(toggle);
    await waitFor(() => expect(toggle).toHaveAttribute("aria-checked", "true"));
    expect(loginItem.calls).toEqual([false, true]);
    expect(readLaunchChoice()).toBe(true);
  });

  it("says what stops after a restart when it is off on a machine that serves", async () => {
    const { fakeLoginItemBridge } = await import("../native/loginItem");
    renderPanel({ loginItem: fakeLoginItemBridge("notRegistered"), serves: { providesSpaces: true, runsAgents: false } });
    expect(
      await screen.findByText("This machine provides Spaces, which stop after a restart until you open Cua Spaces."),
    ).toBeInTheDocument();
  });

  it("shows a failed change and the status read back", async () => {
    const { fakeLoginItemBridge } = await import("../native/loginItem");
    const user = userEvent.setup();
    const loginItem = fakeLoginItemBridge("notRegistered", { fail: "Permission denied" });
    renderPanel({ loginItem });
    const toggle = await screen.findByRole("switch", { name: "Launch Cua Spaces at login" });
    await user.click(toggle);
    expect(await screen.findByText("Permission denied")).toBeInTheDocument();
    expect(toggle).toHaveAttribute("aria-checked", "false");
  });
});
