# Inside Linux computer-use: AT-SPI, XTEST, and background agents

_Published on June 18 by Francesco Bonacci_

<img width="960" height="540" alt="screen-recording-2026-06-17" src="https://github.com/user-attachments/assets/2a8a131c-4ce4-451e-bcc6-f80b4bdfa8eb" />

**TL;DR:**

- Cua Driver now drives real Linux desktop apps in the background for any agent (Claude Code, Codex, your own loop), over MCP or the CLI, available today on the release tag 0.5.7.
- It uses AT-SPI 2 over D-Bus for the accessibility tree, XTEST for input synthesis, and a painted agent cursor kept separate from your physical pointer.
- The supported backend is X11 and XWayland. Native Wayland is still in preview behind an opt-in flag, and contributions are more than welcome.
- Tested on Debian 12, Ubuntu 22.04, Ubuntu 24.04, Rocky 9, and Fedora 41, with release binaries built for glibc 2.31 and newer.
- Source: https://github.com/trycua/cua Docs: https://cua.ai/docs/cua-driver

---

After shipping background computer-use on macOS and then Windows, the obvious next question was Linux, and I want to tell you what that took and where the sharp edges still are. The macOS post was about one private framework and one routing trick. Windows was about a router across a dozen toolkit shapes. Linux turned out to be its own kind of hard, and not the kind I expected going in.

Let me back up. The reason any of this matters is that a lot of real Linux work still lives in GTK, Qt, Electron, Chromium, Tk, and native desktop tools that will never expose an agent API. If you want an agent to use those apps, it eventually has to use the same interface a person uses: open the window, read the controls, click, type, drag, verify, and go back to code. That loop already works reasonably well for web apps. On the desktop it is harder, and on Linux it is harder in a way that has nothing to do with the apps themselves and everything to do with how many moving parts sit underneath them.

What I came out the other side with is the Linux backend for cua-driver, a driver that lets any agent (Claude Code, Codex, your own loop) drive a real Linux desktop app while you keep working in the foreground. The cursor does not move. Focus does not need to change for most operations. The agent gets its own painted cursor, and the tools are available through both MCP and the CLI. It ships today on the same install path as macOS and Windows.

## Linux is hard in a different way

The Windows post had a line I keep coming back to: Windows has a lot of Windows inside it. Linux has the same problem, it just wears it somewhere else. There is no single Linux desktop API to target. There are sessions, compositors, display servers, accessibility services, D-Bus buses, toolkit-specific focus behavior, and apps that only partly agree on what "click this element" should mean. GTK3 behaves differently from GTK4. Qt5 has its own shape. Tk is older and more direct. Electron, Chrome, VS Code, Slack, Discord, and Obsidian all sit on Chromium, which drags in its own accessibility model. Some apps expose enough through AT-SPI to be addressable by name. Some expose a tree but reject direct activation. Some accept synthetic input only when it arrives through the same route a real pointer would use.

So the Linux backend ended up as a router rather than a single recipe, the same shape Windows forced on us. It first asks what structure the app exposes. If the element is reachable through AT-SPI it gets addressed by accessible name, role, index, and bounds instead of by pixels, and if an action can be invoked through accessibility the driver tries that before anything else. When the app rejects that path it falls back to X11 input. Keyboard input moved from XSendEvent to XTEST injection because more apps treat XTEST as real enough to accept, and there are still XSendEvent and X11 fallbacks for the cases where that is the best available route. The reason all of this matters is honesty: when an app is not visible, when AT-SPI is disabled, when the session is native-Wayland-only, or when an app bypasses XWayland, the driver returns an explicit error instead of pretending the action happened. An agent can recover from an error it can read. It cannot recover from a driver that quietly does nothing and reports success, which is the failure mode I spent the most effort designing out.

## First, the easy way (it doesn't get you far)

The path of least resistance is to drive X11 directly: list the windows, read their titles, move the mouse, send key events, and fall back to image matching when there is no structure. That works for a demo and then breaks down fast. X11 can tell you a window exists. It cannot reliably tell you that a sidebar holds a button named "Run", that a text field has placeholder text, or that a dialog has a destructive action sitting behind it. Plenty of modern apps expose no meaningful UI structure through X11 hints at all, so you end up back at pixel-matching, which is exactly the brittle thing computer-use is supposed to get past.

That is where AT-SPI 2 comes in. It is the Linux accessibility layer over D-Bus, the closest analogue to macOS Accessibility and Windows UI Automation, and it is what screen readers and other assistive technologies already use. Through it the driver sees a tree of accessible objects: applications, windows, panels, buttons, entries, text areas, menu items, tabs, and so on. The tree is not always complete, but it is far better than treating the desktop as a bitmap. This is the real difference between pixel automation and computer-use. Pixel automation says click around coordinate 842, 513. Element automation says click the button named Save in this window, which is something an agent can reason about and re-find after the layout shifts. Linux still needs pixels for bounds, screenshots, and dragging, but AT-SPI gives the driver a semantic place to start instead of a screenshot and a prayer.

## The Chromium accessibility switch

Chromium-based apps have one especially sharp edge, and it cost me an afternoon before I understood it. Chrome, VS Code, Slack, Discord, Obsidian, and most Electron or CEF apps do not keep their full AT-SPI tree built by default. Chromium waits until it detects an assistive technology and only then starts building the tree. That is reasonable for performance and genuinely confusing when you are writing a driver, because the app is open and the window is right there, but the tree is simply missing until the session announces that an assistive technology is present.

On Linux that signal lives under the session accessibility status. When `cua-driver serve` starts, the daemon flips the session's `org.a11y.Status` accessibility flags on, which is exactly the signal a screen reader sets. The useful part is that Chromium responds retroactively: already-open Chromium and Electron apps build their AT-SPI trees without a relaunch and without any per-app flag, and GTK and Qt apps read the same signal to warm up their accessibility paths too. That one detail is most of why the backend stopped feeling brittle. Instead of telling people to relaunch Chrome with special flags or configure every Electron app on its own, the driver sets the session-level signal and lets the toolkits do what they already know how to do. You still need AT-SPI reachable on the session bus, and on GNOME you may also need:

```bash
gsettings set org.gnome.desktop.interface toolkit-accessibility true
```

`cua-driver doctor` checks this path and reports what it sees.

## Writing without stealing focus

Clicking is only half the problem. Text input is usually where background automation falls apart, because if the agent has to focus the target app before every write it interrupts you: the foreground cursor jumps, the active window changes, and the whole premise of a background agent gets weaker. So the Linux backend has focus-free write paths for the common toolkit families, and getting there meant handling GTK3, GTK4, Qt5, and Tk each differently. Some widgets accept text through accessibility actions. Some need `GrabFocus` without raising the whole app. Some need synthetic focus events, some need a widget-click fallback, and some are best handled through toolkit IPC when it is available. I would love to say there was one clean abstraction underneath all of it, but there wasn't; it was a pile of per-toolkit special cases that each had to be found the slow way.

This is the Linux version of the lesson from Windows, that every app is shaped differently and the driver has to pick the least disruptive valid path for the target widget, then return a clear error when there is no honest path at all. The goal is not to pretend focus never matters, because it still does, and some apps will only accept text when the toolkit believes the editable widget is focused. The goal is to avoid stealing your foreground focus whenever the platform gives us a way to write without it. In practice an agent can fill fields, type into dialogs, and exercise simple desktop apps without constantly taking over your session.

## Synthetic cursors

The Linux backend uses the same cursor model as macOS and Windows: the agent cursor is not your physical pointer. The driver paints an overlay cursor for the agent, each agent gets its own `cursor_id`, and the physical pointer stays wherever your hand left it while the agent cursor moves somewhere else to click, drag, hold a button, and release. If the agent shared your real pointer you would lose the desktop while it works. With a separate painted cursor you can watch it act without surrendering the session, and cursor ownership stays explicit, which is what makes multi-agent and background workflows tractable to reason about. The backend also ships background drag and held-button tools, which is enough for moving items around a desktop, dragging selected text, testing sliders, or exercising custom widgets that do not expose a good accessibility action. The overlay is a visual trace of what the driver is doing, not the input path itself; the actual input still goes through the supported backend, which today means X11 and XWayland through XTEST and the related X11 routes.

## The daemon problem is smaller on Linux

Windows needed a daemon proxy because of Session 0 isolation: a service cannot reach into the interactive desktop, and that constraint shaped a lot of the Windows driver. Linux does not have that problem in the same way. If you SSH into a machine and the session has a display server, `sshd` can inherit `DISPLAY` cleanly, and as long as the session bus and display are reachable, `cua-driver serve` connects to the desktop straight from the SSH session, with no Session 0 dance. That makes remote development simpler than it was on Windows: SSH into a Linux desktop or a runner with a desktop session, start the driver, and point your MCP client at it. For persistent sessions, use a `systemd --user` unit. For headless or CI-style machines, enable lingering so the service survives logout:

```bash
loginctl enable-linger "$USER"
```

Then run `cua-driver serve` under the user session. The `cua-driver autostart` verb family is currently Windows-only; on Linux, systemd is the supported path.

## What I've been using it for

The first demos are deliberately small. I wanted to show the primitive things that have to be solid before stacking more abstraction on top.

**1. Multi-cursor drag on XFCE.**

<div align="center"><video src="https://github.com/user-attachments/assets/8cd1d8d9-2533-4c3e-88c3-d1fad4971dc2" width="600" controls></video></div>

An agent cursor drags the word "Cua" around on an XFCE desktop while I keep working. The whole point is the separation: the painted agent cursor moves on its own, and my physical pointer never twitches.

**2. Build, launch, act, verify.**

<div align="center"><video src="https://github.com/user-attachments/assets/6562aa37-8077-453d-8ebf-549bd75b5021" width="600" controls></video></div>

A coding agent builds a calculator desktop app, launches it, then uses Cua Driver to QA the app through its own Linux UI. This is the same loop that made the Windows WPF demo worth watching, where the agent does not stop at code generation but runs the thing and checks the thing. For desktop apps a diff is not evidence; the agent has to launch the app and show what happened.

These are not meant to imply every Linux app is solved. They show the supported shape: real desktop apps, element-aware where possible, pointer-aware where needed, driven through the same contract as macOS and Windows.

## What's still broken

Native Wayland is still in preview. The supported path today is X11 or XWayland, and on a Wayland desktop the driver talks to XWayland and treats the session as X11. That works for apps running through XWayland, but native-Wayland-only apps that bypass it, including some modern Firefox and GTK4 builds, may not be visible to the backend at all. There is a native Wayland path behind `CUA_DRIVER_RS_ENABLE_WAYLAND=1`, off by default, with screen capture and full AT-SPI parity still landing. This is the part where outside help goes furthest: if you run a Wayland-only setup, flip the flag, file what breaks, and send a PR. The issues and the backend are open, and contributions are more than welcome.

AT-SPI also has to be enabled and reachable on the session bus. If the accessibility bus is missing or the toolkit accessibility setting is off, the driver still sees some X11-level information, but it loses the element tree that makes the backend useful in the first place.

Headless is still early. The broader private-display story has not shipped. A pure-headless `Xvfb` recipe like `xvfb-run -a cua-driver serve` can be handy for CI experiments, but it is not the same thing as a fully supported background desktop product, and the community fork that explored private Xvfb and Hyprland background capture was closed rather than merged. The glibc floor has been lowered and the binary is built in a Debian 11 container, but I still expect distro, toolkit, and compositor gaps to surface. `cua-driver doctor` exists so you can report those as facts instead of guesses.

## Install

The same installer used by macOS now detects Linux and routes to the Rust backend:

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/trycua/cua/main/libs/cua-driver/scripts/install.sh)"
```

On Linux, the script downloads the `x86_64 linux-gnu` binary and symlinks it into `~/.local/bin/cua-driver`. Make sure `~/.local/bin` is on your `PATH`.

Prerequisites:

```bash
# Debian / Ubuntu
sudo apt-get install -y at-spi2-core

# Fedora / Rocky / RHEL family
sudo dnf install -y at-spi2-core
```

Enable toolkit accessibility where needed:

```bash
gsettings set org.gnome.desktop.interface toolkit-accessibility true
```

Then check the session:

```bash
cua-driver doctor
cua-driver status
cua-driver list_apps
```

The release binary targets glibc 2.31 and newer. It is built in a `debian:11` container and should run on Debian 11+, Ubuntu 20.04+, and RHEL/Rocky/Alma 8+. It has been verified live on Debian 12 with glibc 2.36 and Rocky 9 with glibc 2.34, and tested across Debian 12, Ubuntu 22.04, Ubuntu 24.04, Rocky 9, and Fedora 41.

Start the driver:

```bash
cua-driver serve
```

For a persistent Linux user service, use systemd directly:

```bash
mkdir -p ~/.config/systemd/user

cat > ~/.config/systemd/user/cua-driver.service <<'EOF'
[Unit]
Description=Cua Driver

[Service]
ExecStart=%h/.local/bin/cua-driver serve
Restart=on-failure

[Install]
WantedBy=default.target
EOF

systemctl --user daemon-reload
systemctl --user enable --now cua-driver
```

For headless or CI-style users that need the service to keep running after logout:

```bash
loginctl enable-linger "$USER"
```

Wire it into Claude Code:

```bash
claude mcp add --transport stdio cua-driver -- cua-driver mcp
```

For other MCP clients:

```bash
cua-driver mcp-config
```

Every MCP tool is also exposed as a top-level CLI subcommand, so the same operations can be scripted directly:

```bash
cua-driver list_apps
cua-driver doctor
cua-driver status
```

Repo: https://github.com/trycua/cua  
Docs: https://cua.ai/docs/cua-driver

If you try it against a Linux app that exposes a strange tree, rejects input, or behaves differently across toolkits, please open an issue with the output from `cua-driver doctor`, the distro, the desktop session, and the app name. Those reports are the fastest way to turn early coverage into boring, reliable platform support. ***The toolkit corners I haven't hit yet are the ones I most want to hear about.***
