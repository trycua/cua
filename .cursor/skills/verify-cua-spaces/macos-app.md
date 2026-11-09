# macOS app lane: Cua Spaces Local on a real Mac

Use this lane for the macOS app's first run and its Space pages (`apps/cua-spaces-macos`, `apps/cua-spaces-web`, the notch and the Local Network step). You drive it with the installed cua-driver, the way a person at the Mac would. It has no fixture journal. The proof is a screenshot plus an accessibility read for each step, and the app's own state (`cua spaces ls`, its daemon log).

## Isolate

- **Run the Local channel, never the installed app.** `Cua Spaces Local.app` has its own bundle id, `~/.cua-local`, Keychain items, login items, ports and Local Network row (`libs/cua/crates/cua-home/src/channel.rs`). Setting only `CUA_HOME` on the Stable app still shares its bundle id, helper label and port 3211.
- **Build it.** For a PR head, use `apps/cua-spaces-macos/scripts/install-local.sh --release`. It needs cmake, pnpm and Node 22. For a shipped release, copy the DMG's app and apply the Info.plist and login-item renames that `build-app.sh --channel local` makes, then sign it ad hoc. Ad hoc signing is a stand-in, so name it in the report.
- **Local defaults to staging.** A production test account needs `CuaEnv` `prod` in the bundle (or `CUA_ENV=prod`), and its home is then `~/.cua-local/envs/prod`.
- **One Local per Mac user.** The bundle id is fixed, so two sessions can't run this lane at once. Check `pgrep -fl "Cua Spaces Local"` first.
- **Answer "Not now" to onboarding's agent setup and Terminal install.** They write `~/.claude.json`, pi config and `~/.zshrc`, which belong to the user, not the channel. Record the sha256 of those files before and after.
- **macOS remembers the Local Network answer per user** ([TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)). A first-run Local Network result counts only on a run where the prompt actually appeared.

## Drive

- **Press by `element_token`.** Take `get_window_state` with the accessibility tree, find the control by role and title, and `click` its `element_token`. Foreground pixel clicks missed Safari's Save Password sheet and a web-view menu row on this host, and the AX press worked. Use pixels only when no element exists, and confirm each one with a screenshot.
- **Before you call a step failed, list the system prompt windows.** Local Network, Keychain and accessibility prompts are separate processes' windows (`UserNotificationCenter`, `SecurityAgent`, `universalAccessAuthWarn`). Screenshots of the app's own window don't show them. Call `list_windows` for those pids, or take a `get_desktop_state` screenshot. In one first run the Local Network prompt had been on screen since launch while the app already showed its failure, and the run lost minutes because it read only the app's window.
- **Bring the app to the front before you judge the inline viewer.** The app stops its video sessions while its window is covered (`WebHost/WebUIVideoSurfaces.swift`, `setPaused`). A covered window gives "No picture arrived from the desktop within 20 seconds", and a foreground pixel click into Safari is enough to cover it. Call `bring_to_front` on the app, then open the Space.
- **Never print the account password.** Pipe it to `type_text` as JSON on stdin from a 0600 file.

## Clean up

1. Delete the Spaces you made from the app (More actions, then Delete Space).
2. Unregister the driver login item while the app is still installed: `CUA_CHANNEL=local ~/Applications/"Cua Spaces Local.app"/Contents/MacOS/CuaSpacesMac --driver-helper unregister`. `uninstall-local.sh` unregisters only the helper (`_local-common.sh`), and a driver item left loaded re-creates `~/.cua-local`.
3. Run `apps/cua-spaces-macos/scripts/uninstall-local.sh --yes` from the same checkout. If `launchctl list` still shows `com.trycua.spaces.local.driver`, run `launchctl bootout gui/$(id -u)/com.trycua.spaces.local.driver` and remove `~/.cua-local`.
4. Check with `launchctl list | grep -i spaces.local`, `ls -d ~/.cua-local ~/Applications/"Cua Spaces Local.app"`, the Stable pids against your baseline, and the agent-config sha256s.

Don't run `sfltool dumpbtm` to check login items. Three calls in one cleanup each blocked for hours. `launchctl list` answers the same question at once.

## Lume guest limits

A stock Lume macOS guest can't create local Spaces, because it has no nested virtualization. Its 720x450 pt display hides onboarding and wizard buttons, and `type_text` into the guest's VZ window types the wrong characters (send one `press_key` per character). Use a guest only for what doesn't need those, and use the real Mac for first-run timing.

## Report

Name the channel, the build (PR head or a release with its DMG sha256), `CuaEnv`, and what you skipped: an agent with a real model key, macOS Spaces, desktop sharing, the Stable Keychain path. Timings stay out of PR text.
