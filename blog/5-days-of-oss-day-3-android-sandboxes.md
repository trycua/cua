# Day 3 of 5 Days of OSS Releases: QEMU Android Sandboxes

_Published on January 31, 2026 by the Cua Team_

We're halfway through 5 Days of OSS Releases! Today we're releasing QEMU Android Sandboxes running in Docker, available for self-hosting and MIT-licensed.

## Full Android Emulator

Real Android 11 system with Google APIs running via QEMU/KVM. A complete Android device with touch input, app installation, and system services. Run any app your agent needs to test or automate.

![androidqemu_1](https://github.com/user-attachments/assets/3c9fb76f-5e0b-4031-a152-6ff0f9301dfd)


```bash
docker run -p 8006:8006 -p 8000:8000 ghcr.io/trycua/cua-android:latest
```

## MCP Server Included

Connect Claude Code, Claude Desktop, or any MCP-compatible client directly to the Android device over HTTP. Screenshots, touch, keyboard, shell commands — all exposed as MCP tools. Ask Claude to test your app and watch it happen.

![androidqemu_2](https://github.com/user-attachments/assets/66cdb74f-8c5b-4515-b4f6-8fae29830b26)


```json
{
  "mcpServers": {
    "cua-android": {
      "type": "url",
      "url": "http://localhost:8000/mcp"
    }
  }
}
```

## Device Profiles

Emulate real hardware. Samsung Galaxy S6 through S10, Nexus 4, Nexus One, tablets like Pixel C. Configure via EMULATOR_DEVICE environment variable.

![androidqemu_3](https://github.com/user-attachments/assets/af8f6588-949a-40ad-94ef-d77c6755392c)


```bash
docker run -e EMULATOR_DEVICE="Samsung Galaxy S10" ghcr.io/trycua/cua-android:latest
```

## ADB Built-in

Full Android Debug Bridge access. Install APKs, run shell commands, capture logcat, fire intents, inspect memory. Everything you'd do with a physical device, all from the container.

![androidqemu_4](https://github.com/user-attachments/assets/9b0b3295-f8c3-4642-9740-4d8b050cd6e5)


```bash
adb shell pm list packages
adb shell am start -a android.intent.action.VIEW -d "https://example.com"
adb shell dumpsys meminfo com.android.chrome
adb shell logcat -d | grep -i error
```

## Computer Server Pre-installed

Same HTTP API on port 8000 as macOS, Windows, and Linux sandboxes. Screenshots, touch, keyboard, shell commands — all unified. Write one agent, run it across every platform.

![androidqemu_5](https://github.com/user-attachments/assets/edd56ec5-3832-47e6-b052-985611edd4b4)


## noVNC on Port 8006

Watch your agent navigate Android in the browser. See the screen, observe touch events, debug visually. No VNC client needed.

![androidqemu_6](https://github.com/user-attachments/assets/8c0c539f-7ee9-499b-837c-a4d333f0885b)


## Intent-based Automation

Launch apps, open URLs, send broadcasts via Android intents. More reliable than coordinate-based tapping — let the OS handle navigation.

![androidqemu_7](https://github.com/user-attachments/assets/79ee200d-b9f4-4b06-9b2f-a3d91649c008)


```bash
# Launch Settings app
adb shell am start -a android.settings.SETTINGS

# Open URL in Chrome
adb shell am start -a android.intent.action.VIEW -d "https://example.com" com.android.chrome
```

## Full Diagnostic Access

logcat for real-time logs, dumpsys for system state, pm for package management, top for resource monitoring. Debug apps, track performance, catch crashes automatically.

![androidqemu_8](https://github.com/user-attachments/assets/2f07c93e-48ef-4e4c-b3a7-120a682b2ce3)


## Built on docker-android

Based on budtmo/docker-android with cua-computer-server added. Proven emulator stability with our unified agent API on top.

![androidqemu_9](https://github.com/user-attachments/assets/2254c159-388b-4845-95a2-002237fdd930)

---

**Requires Docker and KVM support.**

- [GitHub Repository](https://github.com/trycua/cua)
- [Desktop Sandbox Documentation](https://cua.ai/docs/cua/guide/get-started/what-is-desktop-sandbox)
