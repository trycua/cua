# Omarchy on Apple Silicon with Lume: X thread

## Post 1 of 8

We got the official Omarchy 4.0.1 source running in an ARM64 Lume VM on Apple
Silicon.

Hyprland boots. The Omarchy shell renders. Keyboard, mouse, networking, native
ARM64 Chromium, and transparent Rosetta translation all work. 🧵

**Attach:** https://github.com/user-attachments/assets/811018be-6ed8-4b94-b719-4d147affdde3

**Alt text:** Omarchy on Apple Silicon. A technical diagram shows an Omarchy
desktop running inside an ARM64 Lume VM, with official source and an Arch Linux
ARM base.

## Post 2 of 8

The stock Omarchy ISO cannot boot here: it contains an x86-64 kernel and
bootloader, while Apple Virtualization.framework creates an ARM64 Linux VM.

Rosetta translates user-space programs. It does not translate a kernel or make
an x86-64 ISO bootable.

## Post 3 of 8

So we built a different path:

Apple Silicon → Lume / Virtualization.framework → Arch Linux ARM → official
Omarchy v4.0.1 source

Rosetta is an optional sidecar for the remaining self-contained x86-64 user
applications—not the foundation of the system.

**Attach:** https://github.com/user-attachments/assets/352315ab-87d8-4b6c-acfa-dfc2be79b025

**Alt text:** Four-layer architecture from Apple Silicon through Lume and
Apple Virtualization.framework to Arch Linux ARM and official Omarchy 4.0.1
source. Rosetta appears beside the stack and points only to x86-64 user-space
applications.

## Post 4 of 8

What we verified end to end:

• two cold desktop boots and a guest reboot
• Hyprland + Quickshell
• keyboard and pointer input over VNC
• HTTPS networking and native ARM64 Chromium
• transparent execution of a verified x86-64 BusyBox binary through Rosetta

**Attach:** `blog/assets/omarchy-arm64-lume/desktop.png`

**Alt text:** The Omarchy desktop running in a Lume ARM64 Linux VM, with the
Omarchy shell visible around a terminal window.

## Post 5 of 8

The source is pinned, not approximated.

The compatibility installer checks out official Omarchy tag v4.0.1 at commit
`13f18b2cb7286fb54f87daf571a031aa6af3d8f0`, then installs the native ARM64
package subset and the desktop configuration onto Arch Linux ARM.

## Post 6 of 8

The gaps:

• software llvmpipe graphics; no Vulkan GPU
• the tested ARM kernel lacks `CONFIG_SND_VIRTIO`; audio is Dummy Output
• 27 package names are absent, but 16 look packaging-only and 7 source-portable
• dynamic x86-64 apps still need x86-64 libraries

**Attach:** https://github.com/user-attachments/assets/629c9343-5a27-4749-beca-3099ac3af313

**Alt text:** A capability matrix marks desktop boot, input, networking,
Chromium, and Rosetta user-space translation as tested; graphics acceleration
and audio are unavailable, while package parity is classified as recoverable.

## Post 7 of 8

This is not an official Omarchy ARM64 release or a replacement for the stock
ISO.

It's official Omarchy source on an ARM64 base, adapted for Lume's virtual
hardware—useful for experiments, computer-use work, and closing the remaining
compatibility gaps.

## Post 8 of 8

The full process is documented: verify signed Archboot, create the VM, install
the ARM64 base, pin Omarchy source, enable optional Rosetta, and validate the
result.

Guide: https://cua.ai/docs/how-to-guides/lume/run-omarchy-arm64

If you try it, tell us what works—and which ARM64 or Wayland gap we should
close next.
