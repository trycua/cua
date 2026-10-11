// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import list from "../../../../../libs/images/sandbox-images.json";
import {
  type ConnectedCloud,
  friendlyAddError,
  looksLikeAddress,
  NewSpaceWizard,
  type NewSpaceWizardProps,
  type SpaceHost,
} from "./NewSpaceWizard";

function renderWizard(overrides: Partial<NewSpaceWizardProps> = {}) {
  const props: NewSpaceWizardProps = {
    cloudAvailable: true,
    localAvailable: true,
    onCreate: vi.fn(),
    onAddByAddress: vi.fn(async () => {}),
    onCancel: vi.fn(),
    maxCpus: 8,
    ...overrides,
  };
  render(<NewSpaceWizard {...props} />);
  return props;
}

const next = () => fireEvent.click(screen.getByRole("button", { name: "Continue" }));

const AWS: ConnectedCloud = {
  name: "aws",
  title: "AWS",
  label: "AWS \u00b7 us-west-2",
  isDefault: true,
  ttlHours: 8,
  offers: [
    { image: "linux", kind: "container", supported: true, reason: "", machineType: "t4g.medium", usdPerHour: 0.0368 },
    { image: "windows", kind: "vm", supported: false, reason: "Windows on AWS is not offered yet.", machineType: "", usdPerHour: 0 },
  ],
};

const imageField = () => screen.getByRole("combobox", { name: "Image" });
/** The "Run on" menu (one entry per machine that can host the Space). */
const runOnMenu = () => screen.getByRole("combobox", { name: "Run on" });
const runOn = (id: string) => fireEvent.change(runOnMenu(), { target: { value: id } });
const entries = () =>
  within(runOnMenu())
    .getAllByRole("option")
    .map((o) => `${o.textContent}${(o as HTMLOptionElement).disabled ? " (disabled)" : ""}`);
const YOUR_CLOUD = { cuaVolume: false, yourCloud: true, sharing: false };
const MINI: SpaceHost = { id: "m-mini", name: "Mac mini", via: "relay", online: true, os: "macos", limits: [] };
const STUDIO: SpaceHost = { id: "m-studio", name: "Studio", via: "relay", online: false, os: "", limits: [] };
const suggestionRefs = () =>
  within(screen.getByRole("listbox", { name: "Image" }))
    .getAllByRole("option")
    .map((option) => option.querySelector(".wz-combo-ref")!.textContent);

describe("the image field", () => {
  // What pickers offer: published entries of groups without `picker: false`
  // (benchmark images are for cua-bench, not Spaces).
  const offeredGroups = new Set(list.groups.filter((g) => !("picker" in g) || g.picker !== false).map((g) => g.id));
  const offered = list.images.filter((image) => image.published && offeredGroups.has(image.group));
  const pressedOs = () =>
    ["Linux", "Windows", "macOS"].filter(
      (name) => screen.getByRole("button", { name: new RegExp(`^${name}`) }).getAttribute("aria-pressed") === "true",
    );

  it("suggests the selected OS's images of libs/images/sandbox-images.json, in order; never benchmarks", () => {
    renderWizard();
    expect(screen.queryByRole("listbox")).toBeNull();
    fireEvent.click(imageField());
    expect(suggestionRefs()).toEqual(offered.filter((i) => i.os === "linux").map((i) => i.ref));
    expect(pressedOs()).toEqual(["Linux"]);
    for (const hidden of list.images.filter((image) => !offered.includes(image))) {
      expect(suggestionRefs()).not.toContain(hidden.ref);
    }
  });

  it("typing a custom image unselects the OS and searches every OS", () => {
    renderWizard();
    fireEvent.change(imageField(), { target: { value: "ghcr.io/acme/desk" } });
    expect(pressedOs()).toEqual([]);
    fireEvent.keyDown(imageField(), { key: "Escape" });
    fireEvent.click(imageField());
    expect(suggestionRefs()).toEqual(offered.map((i) => i.ref));
  });

  it("groups the suggestions the way the file groups them, one line a row", () => {
    renderWizard();
    fireEvent.click(imageField());
    const groups = within(screen.getByRole("listbox", { name: "Image" })).getAllByRole("group");
    const used = new Set(offered.filter((i) => i.os === "linux").map((i) => i.group));
    expect(groups.map((g) => g.getAttribute("aria-label"))).toEqual(
      list.groups.filter((g) => used.has(g.id)).map((g) => g.label),
    );
    const first = within(groups[0]!).getAllByRole("option")[0]!;
    expect(first).toHaveTextContent("ghcr.io/trycua/linux:24.04");
    expect(first).toHaveTextContent("Ubuntu 24.04");
  });

  it("filters as you type; up, down and return pick; escape dismisses", () => {
    renderWizard();
    fireEvent.change(imageField(), { target: { value: "macos" } });
    // Every published macOS image in the catalog, in catalog order (tiers appear once published).
    expect(suggestionRefs()).toEqual(
      list.images.filter((i) => i.published && i.os === "macos").map((i) => i.ref),
    );
    fireEvent.keyDown(imageField(), { key: "ArrowDown" });
    fireEvent.keyDown(imageField(), { key: "ArrowDown" });
    fireEvent.keyDown(imageField(), { key: "ArrowUp" });
    expect(within(screen.getByRole("listbox")).getByRole("option", { selected: true })).toHaveTextContent(
      "ghcr.io/trycua/macos:26",
    );
    fireEvent.keyDown(imageField(), { key: "Enter" });
    expect(imageField()).toHaveValue("ghcr.io/trycua/macos:26");
    expect(screen.queryByRole("listbox")).toBeNull();
    fireEvent.change(imageField(), { target: { value: "ubuntu" } });
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    fireEvent.keyDown(imageField(), { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("clicking a suggestion fills the field", () => {
    renderWizard();
    fireEvent.click(imageField());
    fireEvent.mouseDown(screen.getByRole("option", { name: /linux:24\.04-disk/ }));
    expect(imageField()).toHaveValue("ghcr.io/trycua/linux:24.04-disk");
  });

  it("accepts a custom ref as typed and creates with it", () => {
    const { onCreate } = renderWizard();
    runOn("local");
    fireEvent.change(imageField(), { target: { value: "ghcr.io/acme/desktop:1.2" } });
    fireEvent.keyDown(imageField(), { key: "Enter" });
    expect(imageField()).toHaveValue("ghcr.io/acme/desktop:1.2");
    expect(screen.queryByRole("alert")).toBeNull();
    next();
    next();
    next();
    expect(screen.getByLabelText("Summary")).toHaveTextContent("ghcr.io/acme/desktop:1.2");
    fireEvent.click(screen.getByRole("button", { name: "Create Space" }));
    expect(vi.mocked(onCreate).mock.calls[0]![0].image.ref).toBe("ghcr.io/acme/desktop:1.2");
  });

  it("rejects a malformed ref inline in one line", () => {
    renderWizard();
    fireEvent.change(imageField(), { target: { value: "ghcr.io/Acme/App" } });
    expect(imageField()).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("alert")).toHaveTextContent("Use lowercase letters in the image name.");
    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
  });
});

describe("New Space wizard", () => {
  it("walks System, Resources, Options, Summary and creates a local VM with its resources", () => {
    const { onCreate } = renderWizard();
    expect(screen.getByRole("list", { name: "Steps" })).toHaveTextContent(
      "1System2Resources3Options4Summary",
    );
    fireEvent.change(imageField(), { target: { value: "ghcr.io/trycua/linux:24.04-disk" } });
    runOn("local");
    next();
    fireEvent.change(screen.getByRole("slider", { name: "CPU cores" }), { target: { value: "3" } });
    fireEvent.change(screen.getByRole("slider", { name: "Memory" }), { target: { value: "6" } });
    next();
    fireEvent.change(screen.getByPlaceholderText("Optional"), {
      target: { value: "cua-e2e-vm" },
    });
    next();
    const summary = screen.getByLabelText("Summary");
    expect(summary).toHaveTextContent("ghcr.io/trycua/linux:24.04-disk");
    expect(summary).toHaveTextContent("QEMU virtual machine on this Mac");
    expect(summary).toHaveTextContent("3 cores, 6 GB memory");
    fireEvent.click(screen.getByRole("button", { name: "Create Space" }));
    expect(onCreate).toHaveBeenCalledWith(
      expect.objectContaining({
        placement: "local",
        name: "cua-e2e-vm",
        cpus: 3,
        memoryMb: 6 * 1024,
        openWhenReady: true,
        image: expect.objectContaining({ ref: "ghcr.io/trycua/linux:24.04-disk", local: "qemu" }),
      }),
    );
  });

  it("lists This Mac and your machines; your clouds and Connect a cloud only with Your cloud", () => {
    const onConnectCloud = vi.fn();
    renderWizard({ clouds: [AWS], hosts: [MINI, STUDIO], onConnectCloud });
    expect(entries()).toEqual(["This Mac", "Mac mini", "Studio (offline) (disabled)"]);
    expect(runOnMenu()).toHaveValue("local");
    expect(screen.queryByText(/Cua Cloud/)).toBeNull();
    expect(screen.queryByRole("button", { name: /Connect a cloud/ })).toBeNull();
    cleanup();
    renderWizard({ clouds: [AWS], hosts: [MINI, STUDIO], onConnectCloud, experiments: YOUR_CLOUD });
    expect(entries()).toEqual(["This Mac", "Mac mini", "Studio (offline) (disabled)", "AWS \u00b7 us-west-2"]);
    fireEvent.click(screen.getByRole("button", { name: /Connect a cloud/ }));
    expect(onConnectCloud).toHaveBeenCalled();
  });

  it("creates on one of your machines with its size and no price", () => {
    const { onCreate } = renderWizard({ hosts: [MINI] });
    runOn("host:m-mini");
    expect(runOnMenu()).toHaveValue("host:m-mini");
    next();
    expect(screen.getByRole("slider", { name: "CPU cores" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Cost")).toBeNull();
    next();
    next();
    expect(screen.getByLabelText("Summary")).toHaveTextContent("Mac mini");
    fireEvent.click(screen.getByRole("button", { name: "Create Space" }));
    expect(onCreate).toHaveBeenCalledWith(expect.objectContaining({ placement: "host", host: "m-mini" }));
  });

  it("creates in a connected cloud with its machine, cost and lifetime", () => {
    const props = renderWizard({ clouds: [AWS], experiments: YOUR_CLOUD });
    runOn("aws");
    expect(runOnMenu()).toHaveValue("aws");
    // Windows cannot run there: its tile is greyed out.
    expect(screen.getByRole("button", { name: /^Windows/ })).toBeDisabled();
    next();
    expect(screen.queryByRole("slider")).toBeNull();
    expect(screen.getByLabelText("Cost")).toHaveTextContent("About $0.04/hour");
    expect(screen.getByLabelText("Resources")).toHaveTextContent("t4g.medium");
    expect(screen.getByLabelText("Resources")).toHaveTextContent("After 8 hours");
    next();
    next();
    fireEvent.click(screen.getByRole("button", { name: "Create Space" }));
    expect(props.onCreate).toHaveBeenCalledWith(expect.objectContaining({ placement: "yours", cloud: "aws" }));
  });

  it("shows no price on this Mac or a machine by address, through the review", () => {
    renderWizard();
    next();
    expect(screen.queryByLabelText("Cost")).toBeNull();
    next();
    next();
    expect(screen.queryByLabelText("Cost")).toBeNull();
    expect(screen.queryByText(/Free/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    fireEvent.click(screen.getByRole("button", { name: /Connect by address/ }));
    expect(screen.queryByLabelText("Cost")).toBeNull();
    expect(screen.queryByText(/Free/)).toBeNull();
  });

  it("moves macOS to This Mac (no cloud variant) and blocks Continue without a local runtime", () => {
    renderWizard({ localAvailable: false, localReason: "Docker is not running" });
    fireEvent.change(imageField(), {
      target: { value: "ghcr.io/trycua/macos:26" },
    });
    expect(runOnMenu()).toHaveValue("local");
    expect(screen.getByRole("alert")).toHaveTextContent("Docker is not running");
    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
  });

  it("the OS tiles pick that system's first image", () => {
    renderWizard();
    fireEvent.click(screen.getByRole("button", { name: /^Windows/ }));
    expect(imageField()).toHaveValue("ghcr.io/trycua/windows:2022");
  });

  it("rejects names that are not DNS labels", () => {
    renderWizard();
    next();
    next();
    fireEvent.change(screen.getByPlaceholderText("Optional"), { target: { value: "Bad Name" } });
    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
  });

  // From the catalog: a published image without cua-spacesd, when there is one.
  const noStream = list.images.find((image) => image.published && !image.spacesd);
  it.skipIf(!noStream)("warns that images without cua-spacesd have no stream", () => {
    renderWizard();
    fireEvent.change(imageField(), {
      target: { value: noStream!.ref },
    });
    next();
    next();
    expect(screen.getByRole("note")).toHaveTextContent("No desktop stream for this image yet.");
  });
});

describe("the Resources step", () => {
  const GB = 2 ** 30;
  const vol = (gb: number, name: string) => ({ availableBytes: gb * GB, totalBytes: 500 * GB, name });
  const storage = (container = 40) => ({
    reserveBytes: 5 * GB,
    lume: vol(212, "Macintosh HD"),
    qemu: vol(212, "Macintosh HD"),
    container: vol(container, "Colima"),
    pulled: [] as string[],
  });
  const facts = () =>
    within(screen.getByLabelText("Resources"))
      .getAllByRole("definition")
      .map((d) => d.textContent);
  const size = (ref: string, arch: string) =>
    list.images.find((i) => i.ref === ref)!.sizes!.platforms.find((p) => p.arch === arch)!;

  it("shows kind, architecture, the disk with its download and the room on the engine's disk", () => {
    renderWizard({ hostArch: "arm64", storage: storage() });
    runOn("local");
    next();
    const p = size("ghcr.io/trycua/linux:24.04", "arm64");
    const gb = (b: number) => (b / GB < 10 ? (b / GB).toFixed(1) : Math.round(b / GB)) + " GB";
    expect(facts()).toEqual(["Container", "ARM", `${gb(p.disk)} (download ${gb(p.download)})`, "40 GB on Colima"]);
    expect(screen.queryByRole("slider", { name: "Disk" })).toBeNull();
    expect(screen.getByRole("button", { name: "Continue" })).toBeEnabled();
  });

  it("blocks Continue in one line when the Space does not fit", () => {
    renderWizard({ hostArch: "arm64", storage: storage(3) });
    runOn("local");
    next();
    expect(screen.getByRole("alert")).toHaveTextContent(/^Not enough space on Colima: needs .*, 3\.0 GB available\.$/);
    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
  });

  it("grows a Linux VM's disk with a slider and creates with it", () => {
    const { onCreate } = renderWizard({ hostArch: "arm64", storage: storage() });
    fireEvent.change(imageField(), { target: { value: "ghcr.io/trycua/linux:24.04-disk" } });
    runOn("local");
    next();
    const disk = screen.getByRole("slider", { name: "Disk" });
    expect(disk).toHaveAttribute("min", "20");
    expect(disk).toHaveAttribute("max", "500");
    // The tooltip, and one quiet note under the slider; no Reset at the image's size.
    expect(disk.closest(".wz-slider")).toHaveAttribute("title", "A VM disk cannot shrink below the image's 20 GB.");
    const note = () => document.querySelector(".wz-disk-note .wz-help");
    expect(note()!.textContent).toMatch(/^Uses about .+ on this Mac at first, up to 20 GB as the Space fills it\.$/);
    expect(screen.queryByRole("button", { name: "Reset" })).toBeNull();
    expect(facts()).toEqual(["Virtual machine", "ARM", expect.stringMatching(/GB$/), "212 GB on Macintosh HD"]);
    fireEvent.change(disk, { target: { value: "64" } });
    expect(screen.getByText("64 GB")).toBeInTheDocument();
    expect(note()!.textContent).toMatch(/^The disk will be resized to 64 GB after downloading\. Uses about /);
    // Reset goes back to the image's disk, then comes back on a change.
    fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    expect(screen.getByRole("slider", { name: "Disk" })).toHaveValue("20");
    expect(screen.queryByRole("button", { name: "Reset" })).toBeNull();
    fireEvent.change(screen.getByRole("slider", { name: "Disk" }), { target: { value: "64" } });
    expect(screen.getByRole("button", { name: "Reset" })).toBeInTheDocument();
    next();
    next();
    expect(screen.getByLabelText("Summary")).toHaveTextContent("2 cores, 4 GB memory, 64 GB disk");
    fireEvent.click(screen.getByRole("button", { name: "Create Space" }));
    expect(vi.mocked(onCreate).mock.calls[0]![0]).toMatchObject({ placement: "local", diskGb: 64 });
  });

  it("warns that an image without a build for this Mac runs emulated, locally only", () => {
    renderWizard({ hostArch: "arm64", storage: storage() });
    fireEvent.change(imageField(), { target: { value: "ghcr.io/trycua/omarchy:edge" } });
    runOn("local");
    next();
    const warning = screen.getByRole("img", { name: /Emulated on this Mac’s ARM processor/ });
    expect(warning).toHaveAttribute("title", "Emulated on this Mac’s ARM processor. Performance may be degraded.");
    expect(facts()[1]).toBe("x64");
  });
});

describe("Connect by address", () => {
  it("adds host:port with a token and dismisses on success", async () => {
    const onAddByAddress = vi.fn(async () => {});
    const { onCancel } = renderWizard({ onAddByAddress });
    fireEvent.click(screen.getByRole("button", { name: "Connect by address…" }));
    expect(screen.getByRole("button", { name: "Add Space" })).toBeDisabled();
    fireEvent.change(screen.getByPlaceholderText("10.0.0.5:3211"), {
      target: { value: "127.0.0.1:3211" },
    });
    fireEvent.change(screen.getByPlaceholderText("CUA_ENV_TOKEN"), {
      target: { value: "tok" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add Space" }));
    await waitFor(() => expect(onCancel).toHaveBeenCalled());
    expect(onAddByAddress).toHaveBeenCalledWith("127.0.0.1:3211", "tok", undefined);
  });

  it("shows the SDK's handshake error inline and stays open", async () => {
    const onAddByAddress = vi.fn(async () => {
      throw new Error("cua-spacesd is not available at 127.0.0.1:1");
    });
    const { onCancel } = renderWizard({ onAddByAddress });
    fireEvent.click(screen.getByRole("button", { name: "Connect by address…" }));
    fireEvent.change(screen.getByPlaceholderText("10.0.0.5:3211"), {
      target: { value: "127.0.0.1:1" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add Space" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("not available");
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("shape-checks addresses only", () => {
    expect(looksLikeAddress("10.0.0.5:3211")).toBe(true);
    expect(looksLikeAddress("http://host.example:3211")).toBe(true);
    expect(looksLikeAddress("[::1]:3211")).toBe(true);
    expect(looksLikeAddress("")).toBe(false);
    expect(looksLikeAddress("a b:1")).toBe(false);
  });
});

describe("friendlyAddError", () => {
  it("names a rejected token and keeps the raw detail", () => {
    const text = friendlyAddError("unauthenticated: missing or invalid bearer token (Unauthenticated)");
    expect(text).toMatch(/^The Space rejected the token/);
    expect(text).toContain("invalid bearer token");
  });
  it("passes other errors through", () => {
    expect(friendlyAddError("boom")).toBe("boom");
  });
});

describe("local runtime readiness", () => {
  it("needs the backend the image's runtime uses", () => {
    renderWizard({ localBackends: ["container"] });
    runOn("local");
    expect(screen.getByRole("button", { name: "Continue" })).not.toBeDisabled();
    fireEvent.change(imageField(), { target: { value: "ghcr.io/trycua/macos:26" } });
    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
    expect(screen.getByRole("alert")).toHaveTextContent("cannot run Lume Spaces");
  });
});

describe("default placement", () => {
  it("starts on This Mac when not signed in but a local runtime is ready, even if that is known late", () => {
    const props: NewSpaceWizardProps = {
      cloudAvailable: false,
      localAvailable: false,
      onCreate: vi.fn(),
      onAddByAddress: vi.fn(async () => {}),
      onCancel: vi.fn(),
    };
    const { rerender } = render(<NewSpaceWizard {...props} />);
    rerender(<NewSpaceWizard {...props} localAvailable />);
    expect(runOnMenu()).toHaveValue("local");
    expect(screen.getByRole("button", { name: "Continue" })).not.toBeDisabled();
  });
});

describe("the GPU row", () => {
  const LEARN = "https://cua.ai/docs/lume/guides/gpu-passthrough";
  const lume = (supported: boolean) => ({
    runtime: "lume",
    id: "paravirtual",
    label: "GPU acceleration",
    experimental: true,
    supported,
    reason: supported ? null : "Needs macOS 15 or later",
    learnMore: LEARN,
  });
  const gpuBox = () => screen.queryByRole("checkbox", { name: "GPU acceleration (Experimental)" });
  const toMacResources = () => {
    fireEvent.change(imageField(), { target: { value: "ghcr.io/trycua/macos:26" } });
    next();
  };

  it("offers GPU acceleration for a macOS VM on Lume; checked, the create passes it", () => {
    const onOpenExternal = vi.fn();
    const { onCreate } = renderWizard({ localBackends: ["docker", "lume"], gpus: [lume(true)], onOpenExternal });
    toMacResources();
    const box = gpuBox()!;
    expect(box).toBeEnabled();
    expect(box).not.toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Learn more" }));
    expect(onOpenExternal).toHaveBeenCalledWith(LEARN);
    fireEvent.click(box);
    expect(gpuBox()).toBeChecked();
    next();
    next();
    expect(screen.getByLabelText("Summary")).toHaveTextContent("GPUGPU acceleration (Experimental)");
    fireEvent.click(screen.getByRole("button", { name: "Create Space" }));
    expect(onCreate).toHaveBeenCalledWith(expect.objectContaining({ gpu: "paravirtual" }));
  });

  it("is disabled with the reason where this Mac cannot, and passes nothing", () => {
    const { onCreate } = renderWizard({ localBackends: ["docker", "lume"], gpus: [lume(false)] });
    toMacResources();
    const box = gpuBox()!;
    expect(box).toBeDisabled();
    expect(box.closest("label")).toHaveAttribute("title", "Needs macOS 15 or later");
    expect(screen.getByText("Needs macOS 15 or later")).toBeInTheDocument();
    next();
    next();
    fireEvent.click(screen.getByRole("button", { name: "Create Space" }));
    expect(onCreate).toHaveBeenCalledWith(expect.not.objectContaining({ gpu: expect.anything() }));
  });

  it("shows nothing where the runtime has no GPU or none is known", () => {
    renderWizard({ localBackends: ["docker", "lume"], gpus: [lume(true)] });
    runOn("local");
    next();
    expect(gpuBox()).toBeNull();
    expect(screen.queryByRole("button", { name: "Learn more" })).toBeNull();
    cleanup();
    renderWizard({ localBackends: ["docker", "lume"], gpus: null });
    toMacResources();
    expect(gpuBox()).toBeNull();
  });
});
