// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { SpaceRow } from "../../model/spaces";
import { createFakeHostBridge } from "../../native/host";
import { createFakeInstallerBridge } from "../../native/installer";
import type { KeyvaultBridge, KeyvaultOverview } from "../../native/keyvault";
import { fakeFleetBridge } from "../../test/fakeFleet";
import { createFakeKeyvaultBridge, fixtureOverview, NOW } from "../../test/fakeKeyvault";
import { MainWindow } from "./MainWindow";

const ROWS: SpaceRow[] = [
  { id: "cloud:aurora", name: "aurora", provider: "cloud", spacesdVersion: "0.4.0", features: [], os: "linux", reachable: true },
];

function setup(keyvault: KeyvaultBridge = createFakeKeyvaultBridge()) {
  const fleet = fakeFleetBridge({
    isNative: true,
    listSpaces: async () => ROWS,
    status: async () => ({ configured: true, authMode: "user", baseUrl: "", tokenUrl: "", identity: "ada@example.com" }),
    screenshot: async () => "data:image/png;base64,AAAA",
  });
  const host = createFakeHostBridge({ onboarding: { completed: true, mode: "client" } });
  const installer = createFakeInstallerBridge({ plan: { installed: true, upToDate: true, onPath: true } });
  render(<MainWindow fleet={fleet} host={host} installer={installer} keyvault={keyvault} now={() => NOW} />);
  return { keyvault };
}

async function openKeyvault(category = "All") {
  const kv = await screen.findByRole("listbox", { name: "Keyvault" });
  fireEvent.click(await within(kv).findByRole("option", { name: new RegExp(`^${category}`) }));
  return screen.findByRole("heading", { level: 1, name: category });
}

function switches(scope: HTMLElement = document.body) {
  return within(scope).getAllByRole("switch");
}

describe("Keyvault", () => {
  it("is a sidebar section after the Spaces: categories, then one row per site, with the Waiting badge", async () => {
    setup();
    const kv = await screen.findByRole("listbox", { name: "Keyvault" });
    await waitFor(() => expect(within(kv).getByLabelText("1 waiting")).toBeInTheDocument());
    const names = within(kv)
      .getAllByRole("option")
      .map((o) => o.textContent);
    expect(names.slice(0, 4)).toEqual(["All", "Waiting1", "Access", "Recent"]);
    expect(names.slice(4)).toEqual(["accounts.example-idp.test", "github.example.test", "Slack"]);
    await openKeyvault();
    expect(within(kv).getByRole("option", { name: /^All/ })).toHaveAttribute("aria-selected", "true");
    fireEvent.click(await screen.findByRole("option", { name: /Aurora/ }));
    expect(await screen.findByRole("heading", { level: 1, name: "Aurora" })).toBeInTheDocument();
  });

  it("All lists every site's accounts with nothing allowed unattended by default", async () => {
    setup();
    await openKeyvault();
    const list = await screen.findByRole("list", { name: "Saved items" });
    const gh = within(list).getByRole("listitem", { name: "github.example.test" });
    expect(within(gh).getByText("ada@example.test")).toBeInTheDocument();
    expect(within(gh).getByText("bob@example.test")).toBeInTheDocument();
    expect(within(list).getByRole("listitem", { name: "Slack" })).toHaveTextContent("Example Workspace");
    for (const t of switches(list)) expect(t).toHaveAttribute("aria-checked", "false");
    // The strongest consent state of each account; "asks" is the default and is not spelled out.
    expect(within(gh).getByText("Waiting: com.example.koalabot")).toBeInTheDocument();
    expect(within(gh).queryByText("Asks every time")).toBeNull();
    // Protection, stated as it is, at the bottom of All.
    const facts = screen.getByRole("region", { name: "Protection" });
    expect(facts).toHaveTextContent("Asked by the Cua daemon to widen access");
    expect(facts).toHaveTextContent("Not used: needs a provisioning-signed build");
    expect(facts).toHaveTextContent("Verified as Cua");
    expect(facts).toHaveTextContent("Signature verified");
  });

  it("toggles one account; a site's page toggles every account on it", async () => {
    const keyvault = createFakeKeyvaultBridge();
    setup(keyvault);
    await openKeyvault();
    fireEvent.click(
      await screen.findByRole("switch", { name: "Allow ada@example.test on github.example.test unattended" }),
    );
    await waitFor(() => expect(keyvault.calls).toContain("setUnattended:gh-ada:true"));
    const kv = screen.getByRole("listbox", { name: "Keyvault" });
    fireEvent.click(within(kv).getByRole("option", { name: "github.example.test" }));
    fireEvent.click(await screen.findByRole("switch", { name: "Allow every github.example.test account unattended" }));
    await waitFor(() => expect(keyvault.calls).toContain("setUnattended:gh-ada,gh-bob:true"));
    await waitFor(() =>
      expect(screen.getByRole("switch", { name: "Allow every github.example.test account unattended" })).toHaveAttribute(
        "aria-checked",
        "true",
      ),
    );
    // Identity providers always ask: their switch is off and disabled.
    fireEvent.click(within(kv).getByRole("option", { name: "accounts.example-idp.test" }));
    await screen.findByRole("heading", { level: 1, name: "accounts.example-idp.test" });
    expect(switches(screen.getByRole("list", { name: "Saved items" }))[0]).toBeDisabled();
  });

  it("shows a declined Touch ID prompt as an error and changes nothing", async () => {
    const keyvault = createFakeKeyvaultBridge();
    keyvault.state.presence = false;
    setup(keyvault);
    await openKeyvault();
    fireEvent.click(await screen.findByRole("switch", { name: "Allow bob@example.test on github.example.test unattended" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("user presence was not confirmed");
    expect(keyvault.state.overview.items.every((i) => !i.policy.unattended)).toBe(true);
  });

  it("Waiting approves exactly the ticked items, showing the caller's verified identity", async () => {
    const keyvault = createFakeKeyvaultBridge();
    setup(keyvault);
    await openKeyvault("Waiting");
    const pending = await screen.findByRole("list", { name: "Waiting" });
    expect(pending).toHaveTextContent("com.example.koalabot (signed, team ABCDE12345) · github.example.test → dev-1 · 15 min");
    expect(within(pending).getByTitle(/Says: "open the pull request" \(unverified\)/)).toBeInTheDocument();
    fireEvent.click(within(pending).getByRole("button", { name: "Review…" }));
    // Nothing is selected by default: Approve stays off until an item is ticked.
    expect(within(pending).getByRole("button", { name: "Approve" })).toBeDisabled();
    const box = within(pending).getByRole("checkbox", { name: "github.example.test ada@example.test" });
    expect(box).not.toBeChecked();
    fireEvent.click(box);
    fireEvent.click(within(pending).getByRole("button", { name: "Approve 1 item" }));
    await waitFor(() => expect(screen.getByText("Nothing is waiting.")).toBeInTheDocument());
    expect(keyvault.calls).toContain("approve:req-1");
    expect(keyvault.calls).toContain("approve-items:gh-ada");
    await openKeyvault("Access");
    expect(await screen.findByRole("list", { name: "Access" })).toHaveTextContent("com.example.koalabot → dev-1");
  });

  it("Waiting denies a request", async () => {
    const keyvault = createFakeKeyvaultBridge();
    setup(keyvault);
    await openKeyvault("Waiting");
    fireEvent.click(within(await screen.findByRole("list", { name: "Waiting" })).getByRole("button", { name: "Deny" }));
    await waitFor(() => expect(keyvault.calls).toContain("deny:req-1"));
  });

  it("Access revokes a grant", async () => {
    const keyvault = createFakeKeyvaultBridge();
    setup(keyvault);
    await openKeyvault("Access");
    const access = await screen.findByRole("list", { name: "Access" });
    fireEvent.click(within(access).getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(screen.getByText("No live access.")).toBeInTheDocument());
    expect(keyvault.calls).toEqual(["revokeGrant:grant-1"]);
  });

  it("the switch in the sidebar turns the Keyvault off and blocks approvals and toggles", async () => {
    const keyvault = createFakeKeyvaultBridge();
    setup(keyvault);
    await openKeyvault();
    const kill = await screen.findByRole("switch", { name: "Keyvault" });
    expect(kill).toHaveAttribute("aria-checked", "true");
    expect(kill).toHaveAttribute("title", "Stops every teleport, import and approval");
    fireEvent.click(kill);
    await waitFor(() => expect(kill).toHaveAttribute("aria-checked", "false"));
    expect(keyvault.calls).toEqual(["setDisabled:true"]);
    expect(screen.getByText("Keyvault is off. Nothing can be teleported.")).toBeInTheDocument();
    expect(kill).toHaveAttribute("title", "Turning it back on asks for Touch ID");
    for (const t of switches(screen.getByRole("list", { name: "Saved items" }))) expect(t).toBeDisabled();
    // Turning it back on goes through presence; a declined prompt keeps it off.
    keyvault.state.presence = false;
    fireEvent.click(kill);
    expect(await screen.findByRole("alert")).toHaveTextContent("user presence was not confirmed");
    expect(kill).toHaveAttribute("aria-checked", "false");
  });

  it("Recent shows decisions from the audit log with the chain check", async () => {
    setup();
    await openKeyvault("Recent");
    const log = await screen.findByRole("list", { name: "Audit log" });
    const rows = within(log).getAllByRole("listitem");
    expect(rows[0]).toHaveTextContent("Refused");
    expect(rows[1]).toHaveTextContent("Approved");
    expect(screen.getByText("Log verified")).toBeInTheDocument();
  });

  it("flags a tampered audit log", async () => {
    setup(createFakeKeyvaultBridge(fixtureOverview({ auditVerification: { ok: false, entries: 5, unauthenticated: 0, tampered_line: 3 } })));
    await openKeyvault("Recent");
    expect(await screen.findByText("Log tampered at #3")).toBeInTheDocument();
  });

  it.each<[Partial<KeyvaultOverview>, string]>([
    [{ availability: "not_first_party", message: "The Keyvault only shows items to apps signed by Cua." }, "This app is not signed by Cua"],
    [{ availability: "not_running", message: "The Cua daemon is not running." }, "The Keyvault is not running"],
    [{ availability: "locked", message: "The Keyvault is locked." }, "The Keyvault is locked"],
  ])("explains why it cannot show items (%o)", async (over, title) => {
    setup(createFakeKeyvaultBridge(fixtureOverview({ ...over, items: [], pending: [] })));
    const kv = await screen.findByRole("listbox", { name: "Keyvault" });
    fireEvent.click(await within(kv).findByRole("option", { name: /^All/ }));
    expect(await screen.findByRole("heading", { level: 2, name: title })).toBeInTheDocument();
    expect(screen.getByText(over.message!)).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Saved items" })).not.toBeInTheDocument();
  });

  it("sets up a new Keyvault and shows the recovery key once", async () => {
    const over = fixtureOverview({ availability: "no_vault", message: "No Keyvault yet.", items: [], pending: [] });
    over.status = { ...over.status!, initialized: false, unlocked: false, os_protector_available: true };
    const keyvault = createFakeKeyvaultBridge(over);
    setup(keyvault);
    const kv = await screen.findByRole("listbox", { name: "Keyvault" });
    fireEvent.click(await within(kv).findByRole("option", { name: /^All/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Set up Keyvault" }));
    expect(
      await screen.findByText("Recovery key, shown once: ABCDE-FGHIJ-KLMNO-PQRST-UVWXY-Z2345-67ABC-DEFGH"),
    ).toBeInTheDocument();
    expect(keyvault.calls).toEqual(["setup"]);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText(/ABCDE-FGHIJ/)).not.toBeInTheDocument();
  });

  it("unlocks a locked Keyvault", async () => {
    const over = fixtureOverview({ availability: "locked", message: "The Keyvault is locked.", items: [] });
    over.status = { ...over.status!, unlocked: false, unlock_protectors: ["macos-keychain", "recovery"] };
    const keyvault = createFakeKeyvaultBridge(over);
    setup(keyvault);
    const kv = await screen.findByRole("listbox", { name: "Keyvault" });
    fireEvent.click(await within(kv).findByRole("option", { name: /^All/ }));
    expect(screen.queryByLabelText("Passphrase")).not.toBeInTheDocument();
    fireEvent.click(await screen.findByRole("button", { name: "Unlock" }));
    expect(await screen.findByRole("list", { name: "Saved items" })).toBeInTheDocument();
    expect(keyvault.calls).toEqual(["unlock"]);
  });

  it("sets up with a passphrase when the daemon has no OS key store", async () => {
    const over = fixtureOverview({ availability: "no_vault", message: "No Keyvault yet.", items: [], pending: [] });
    over.status = { ...over.status!, initialized: false, unlocked: false, os_protector_available: false };
    const keyvault = createFakeKeyvaultBridge(over);
    setup(keyvault);
    const kv = await screen.findByRole("listbox", { name: "Keyvault" });
    fireEvent.click(await within(kv).findByRole("option", { name: /^All/ }));
    const pass = await screen.findByLabelText("Passphrase");
    const again = screen.getByLabelText("Confirm passphrase");
    expect(pass).toHaveAttribute("type", "password");
    expect(again).toHaveAttribute("type", "password");
    const button = screen.getByRole("button", { name: "Set up Keyvault" });
    fireEvent.change(pass, { target: { value: "orbit" } });
    expect(screen.getByText("At least 12 characters")).toBeInTheDocument();
    expect(button).toBeDisabled();
    fireEvent.change(pass, { target: { value: "orbit lantern pickle harbor" } });
    fireEvent.change(again, { target: { value: "orbit lantern" } });
    expect(screen.getByText("The passphrases don't match")).toBeInTheDocument();
    expect(button).toBeDisabled();
    fireEvent.change(again, { target: { value: "orbit lantern pickle harbor" } });
    expect(button).toBeEnabled();
    fireEvent.click(button);
    expect(await screen.findByText(/^Recovery key, shown once/)).toBeInTheDocument();
    expect(keyvault.calls).toEqual(["setupWithPassphrase"]);
    expect(keyvault.passphrases).toEqual(["orbit lantern pickle harbor"]);
  });

  it("unlocks with the passphrase", async () => {
    const over = fixtureOverview({ availability: "locked", message: "The Keyvault is locked.", items: [] });
    over.status = { ...over.status!, unlocked: false, unlock_protectors: ["passphrase", "recovery"] };
    const keyvault = createFakeKeyvaultBridge(over);
    setup(keyvault);
    const kv = await screen.findByRole("listbox", { name: "Keyvault" });
    fireEvent.click(await within(kv).findByRole("option", { name: /^All/ }));
    const pass = await screen.findByLabelText("Passphrase");
    expect(screen.queryByLabelText("Confirm passphrase")).not.toBeInTheDocument();
    fireEvent.change(pass, { target: { value: "orbit lantern pickle harbor" } });
    fireEvent.click(screen.getByRole("button", { name: "Unlock" }));
    expect(await screen.findByRole("list", { name: "Saved items" })).toBeInTheDocument();
    expect(keyvault.calls).toEqual(["unlockWithPassphrase"]);
  });
});
