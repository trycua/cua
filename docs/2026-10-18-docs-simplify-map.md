# Docs simplification page map (18 Oct 2026, CUA-1863)

Approved by Francesco on 18 Oct 2026. Data: cua.ai/docs page views over the last 30 days (PostHog), counted after following existing redirects.

## Page counts

| Section | Before | After |
| --- | --- | --- |
| Cua Spaces | 46 | 23 (20 guides and pages, 3 reference) |
| Cua Driver | 45 | 22 (13 guides and pages, 9 reference) |
| Cua CLI | 40 | 13 (4 guides and pages, 9 reference) |
| Cua SDK | 136 | 56 (17 guides and pages, 39 reference; 53 once the Fleet removal PRs delete the 3 Fleet pages) |
| Lume | 17 | 9 |
| Cua Bench | 17 | 8 |
| Start here | 5 | 0 (merged into the products) |
| Home and Docs for agents | 2 | 2 |
| Total | 308 | 133 |

The gap to about 80 is the SDK reference: each method shows its signature in five languages, so the page budget (30 tab blocks, 150 code blocks) stops further merging. Next step: render signatures more compactly, then merge to one page per module.

## Sections

1. Cua Spaces: quickstart, task guides (install, connect your agent, create a Space, add a computer, plans and Teams, share, stream, teleport, Cua Volume, Keyvault, persistent agents, use Spaces from code, troubleshoot, uninstall), Advanced (what the relay can see), How Spaces works.
2. Cua Driver: quickstart, guides, tool reference.
3. Reference: Cua CLI (one page per command group), Cua SDK.
4. More tools: Lume, Cua Bench.

## Hand-written pages merged or removed (PR 1)

| Old path | New path | Views (30 days) |
| --- | --- | --- |
| `/docs/start-here` | `/docs/spaces/quickstart` |  |
| `/docs/start-here/drive-your-first-app` | `/docs/cua-driver/quickstart` | 1205 |
| `/docs/start-here/sandbox-images` | `/docs/cua-sdk/guides/images#every-published-image` | 45 |
| `/docs/start-here/run-in-github-actions` | `/docs/cua-sdk/guides/test-in-ci` | 9 |
| `/docs/start-here/create-a-local-macos-sandbox` | `/docs/cua-cli/quickstart#macos-sandbox` | 375 |
| `/docs/start-here/your-first-cua-bench-task` | `/docs/cua-bench/quickstart#create-your-own-task` | 179 |
| `/docs/cua-driver/guides/gemini` | `/docs/cua-driver/guides/connect-your-agent#gemini` | 11 |
| `/docs/cua-driver/guides/trajectories` | `/docs/cua-driver/guides/operate#record-a-trajectory` | 45 |
| `/docs/cua-driver/guides/test-in-a-sandbox` | `/docs/cua-driver/guides/vms-and-remote#test-a-driver-build-in-a-sandbox` | 9 |
| `/docs/cua-driver/guides/cursor-motion` | `/docs/cua-driver/guides/recipes#design-a-cursor-motion` | 46 |
| `/docs/cua-driver/concepts/perception-extension` | `/docs/cua-driver/concepts/how-cua-driver-works#the-optional-perception-extension` | 87 |
| `/docs/cua-sdk/guides/migrate-from-deprecated-packages` | `/docs/cua-sdk/guides/cua-cloud-ended#deprecated-packages` | 14 |
| `/docs/cua-sdk/guides/sidecars` | `/docs/cua-sdk/guides/services#sidecars` | 8 |
| `/docs/cua-sdk/guides/your-cloud` | `/docs/cua-sdk/guides/local-runtimes#your-own-cloud` | 59 |
| `/docs/cua-sdk/guides/secrets` | `/docs/cua-sdk/guides/services#secrets` | 8 |
| `/docs/cua-sdk/guides/coding-agent-options` | `/docs/cua-sdk/guides/run-a-coding-agent#give-the-agent-mcp-servers` | 19 |
| `/docs/cua-sdk/guides/browse-the-web` | `/docs/cua-sdk/guides/desktop#give-an-agent-a-browser` | 24 |
| `/docs/cua-sdk/guides/thumbnails` | `/docs/cua-sdk/guides/desktop#thumbnails` | 5 |
| `/docs/cua-sdk/guides/disk-usage` | `/docs/cua-sdk/guides/lifecycle#disk-usage-and-cleanup` | 7 |
| `/docs/cua-sdk/guides/your-cloud-resources` | `/docs/cua-sdk/guides/local-runtimes#what-cua-creates` | 8 |
| `/docs/cua-sdk/guides/contrib-providers` | `/docs/cua-sdk/guides/local-runtimes#other-sandbox-providers` | 10 |
| `/docs/lume/guides/api-and-mcp` | `/docs/lume/guides/manage-vms#drive-lume-from-scripts-and-ai-clients` | 21 |
| `/docs/cua-bench/guides/convert-benchmark-images` | `/docs/cua-bench/guides/benchmarks#build-a-benchmark-image` | 3 |
| `/docs/spaces/guides/add-a-machine` | `/docs/spaces/guides/add-a-computer` | 0 |
| `/docs/spaces/guides/unattended-access` | `/docs/spaces/guides/add-a-computer#add-a-computer-from-a-terminal` | 27 |
| `/docs/spaces/guides/host-spaces-on-your-spare-mac` | `/docs/spaces/guides/add-a-computer#use-a-spare-mac-to-host-spaces` | 64 |
| `/docs/spaces/guides/updates` | `/docs/spaces/guides/install#updates` | 4 |
| `/docs/spaces/guides/use-from-an-agent` | `/docs/spaces/guides/connect-your-agent#what-your-agent-can-do` | 171 |
| `/docs/spaces/guides/agent-keys` | `/docs/spaces/guides/connect-your-agent#agent-keys` | 2 |
| `/docs/spaces/guides/plans` | `/docs/spaces/guides/teams#plans` | 6 |
| `/docs/spaces/guides/teams/machines` | `/docs/spaces/guides/teams#team-computers` | 2 |
| `/docs/spaces/guides/teams/faq` | `/docs/spaces/guides/teams#questions` | 1 |
| `/docs/spaces/guides/send-files` | `/docs/spaces/guides/cua-volume#send-a-file-into-a-space` | 3 |
| `/docs/spaces/guides/site-login` | `/docs/spaces/guides/keyvault#let-an-agent-sign-in-to-a-site` | 11 |
| `/docs/spaces/guides/troubleshoot-create` | `/docs/spaces/guides/troubleshoot` | 0 |
| `/docs/spaces/guides/mesh` | `/docs/spaces/guides/relay-security#end-to-end-encrypted-connections-beta` | 2 |
| `/docs/spaces/examples/other-languages` | `/docs/spaces/guides/create-a-space-with-the-sdk#examples-in-other-languages` | 5 |

## Generated reference: CLI, Driver, Lume and Bench (PR 2)

| Old path | New path | Views (30 days) |
| --- | --- | --- |
| `/docs/cua-cli/reference/cli/spaces` | `/docs/cua-cli/reference/cli/spaces-and-computers#cua-spaces` | 12 |
| `/docs/cua-cli/reference/cli/host` | `/docs/cua-cli/reference/cli/spaces-and-computers#cua-host` | 6 |
| `/docs/cua-cli/reference/cli/devices` | `/docs/cua-cli/reference/cli/spaces-and-computers#cua-devices` | 3 |
| `/docs/cua-cli/reference/cli/team` | `/docs/cua-cli/reference/cli/spaces-and-computers#cua-team` | 3 |
| `/docs/cua-cli/reference/cli/usage` | `/docs/cua-cli/reference/cli/spaces-and-computers#cua-usage` | 2 |
| `/docs/cua-cli/reference/cli/spacesd` | `/docs/cua-cli/reference/cli/spaces-and-computers#cua-spacesd` | 4 |
| `/docs/cua-cli/reference/cli/volume` | `/docs/cua-cli/reference/cli/files-teleport-and-keys#cua-volume` | 2 |
| `/docs/cua-cli/reference/cli/teleport` | `/docs/cua-cli/reference/cli/files-teleport-and-keys#cua-teleport` | 2 |
| `/docs/cua-cli/reference/cli/keyvault` | `/docs/cua-cli/reference/cli/files-teleport-and-keys#cua-keyvault` | 4 |
| `/docs/cua-cli/reference/cli/sandbox` | `/docs/cua-cli/reference/cli/sandboxes#cua-sandbox` | 5 |
| `/docs/cua-cli/reference/cli/image` | `/docs/cua-cli/reference/cli/sandboxes#cua-image` | 3 |
| `/docs/cua-cli/reference/cli/runtime` | `/docs/cua-cli/reference/cli/sandboxes#cua-runtime` | 3 |
| `/docs/cua-cli/reference/cli/cache` | `/docs/cua-cli/reference/cli/sandboxes#cua-cache` | 3 |
| `/docs/cua-cli/reference/cli/cloud` | `/docs/cua-cli/reference/cli/sandboxes#cua-cloud` | 4 |
| `/docs/cua-cli/reference/cli/viewer` | `/docs/cua-cli/reference/cli/sandboxes#cua-viewer` | 2 |
| `/docs/cua-cli/reference/cli/do` | `/docs/cua-cli/reference/cli/computer-actions#cua-do` | 3 |
| `/docs/cua-cli/reference/cli/trajectory` | `/docs/cua-cli/reference/cli/computer-actions#cua-trajectory` | 2 |
| `/docs/cua-cli/reference/cli/agent` | `/docs/cua-cli/reference/cli/agents#cua-agent` | 8 |
| `/docs/cua-cli/reference/cli/mcp` | `/docs/cua-cli/reference/cli/agents#cua-mcp` | 2 |
| `/docs/cua-cli/reference/cli/skills` | `/docs/cua-cli/reference/cli/agents#cua-skills` | 5 |
| `/docs/cua-cli/reference/cli/auth` | `/docs/cua-cli/reference/cli/account-and-setup#cua-auth` | 5 |
| `/docs/cua-cli/reference/cli/config` | `/docs/cua-cli/reference/cli/account-and-setup#cua-config` | 2 |
| `/docs/cua-cli/reference/cli/daemon` | `/docs/cua-cli/reference/cli/account-and-setup#cua-daemon` | 3 |
| `/docs/cua-cli/reference/cli/doctor` | `/docs/cua-cli/reference/cli/account-and-setup#cua-doctor` | 3 |
| `/docs/cua-cli/reference/cli/telemetry` | `/docs/cua-cli/reference/cli/account-and-setup#cua-telemetry` | 2 |
| `/docs/cua-cli/reference/cli/sandbox-access` | `/docs/cua-cli/reference/cli/sandboxes#cua-sandbox-exec` | 5 |
| `/docs/cua-cli/reference/mcp-tools/computer` | `/docs/cua-cli/reference/mcp-tools#computer-tools` | 7 |
| `/docs/cua-cli/reference/mcp-tools/images` | `/docs/cua-cli/reference/mcp-tools#image-tools` | 1 |
| `/docs/cua-cli/reference/mcp-tools/sandbox` | `/docs/cua-cli/reference/mcp-tools#sandbox-tools` | 2 |
| `/docs/cua-cli/reference/mcp-tools/skills` | `/docs/cua-cli/reference/mcp-tools#skills-tools` | 3 |
| `/docs/cua-cli/reference/mcp-tools/spaces` | `/docs/cua-cli/reference/mcp-tools#spaces-tools` | 2 |
| `/docs/cua-cli/reference/mcp-tools/teleport` | `/docs/cua-cli/reference/mcp-tools#teleport-tools` | 2 |
| `/docs/cua-driver/reference/cli/mcp` | `/docs/cua-driver/reference/cli/serve-and-configure#cua-driver-mcp` | 26 |
| `/docs/cua-driver/reference/cli/daemon` | `/docs/cua-driver/reference/cli/serve-and-configure#cua-driver-serve` | 14 |
| `/docs/cua-driver/reference/cli/permissions` | `/docs/cua-driver/reference/cli/serve-and-configure#cua-driver-permissions` | 21 |
| `/docs/cua-driver/reference/cli/config` | `/docs/cua-driver/reference/cli/serve-and-configure#cua-driver-config` | 4 |
| `/docs/cua-driver/reference/cli/updates` | `/docs/cua-driver/reference/cli/serve-and-configure#cua-driver-check-update` | 8 |
| `/docs/cua-driver/reference/cli/extensions` | `/docs/cua-driver/reference/cli/skills-recording-and-history#cua-driver-skills` | 10 |
| `/docs/cua-driver/reference/cli/recording` | `/docs/cua-driver/reference/cli/skills-recording-and-history#cua-driver-recording` | 3 |
| `/docs/cua-driver/reference/cli/history` | `/docs/cua-driver/reference/cli/skills-recording-and-history#cua-driver-history` | 3 |
| `/docs/cua-driver/reference/mcp-tools/apps-and-windows` | `/docs/cua-driver/reference/mcp-tools/apps-windows-and-screen#app-and-window-tools` | 16 |
| `/docs/cua-driver/reference/mcp-tools/window-state` | `/docs/cua-driver/reference/mcp-tools/apps-windows-and-screen#window-state-tools` | 14 |
| `/docs/cua-driver/reference/mcp-tools/screen` | `/docs/cua-driver/reference/mcp-tools/apps-windows-and-screen#screen-tools` | 5 |
| `/docs/cua-driver/reference/mcp-tools/sessions` | `/docs/cua-driver/reference/mcp-tools/apps-windows-and-screen#session-tools` | 3 |
| `/docs/cua-driver/reference/mcp-tools/click` | `/docs/cua-driver/reference/mcp-tools/click-and-type#click-tool` | 16 |
| `/docs/cua-driver/reference/mcp-tools/double-and-right-click` | `/docs/cua-driver/reference/mcp-tools/click-and-type#double-click-and-right-click-tools` | 3 |
| `/docs/cua-driver/reference/mcp-tools/keyboard` | `/docs/cua-driver/reference/mcp-tools/click-and-type#typing-tools` | 7 |
| `/docs/cua-driver/reference/mcp-tools/keys-and-shortcuts` | `/docs/cua-driver/reference/mcp-tools/click-and-type#key-and-shortcut-tools` | 5 |
| `/docs/cua-driver/reference/mcp-tools/values-and-clipboard` | `/docs/cua-driver/reference/mcp-tools/click-and-type#value-and-clipboard-tools` | 6 |
| `/docs/cua-driver/reference/mcp-tools/pointer` | `/docs/cua-driver/reference/mcp-tools/pointer-cursor-and-browser#pointer-tools` | 12 |
| `/docs/cua-driver/reference/mcp-tools/agent-cursor` | `/docs/cua-driver/reference/mcp-tools/pointer-cursor-and-browser#agent-cursor-tools` | 154 |
| `/docs/cua-driver/reference/mcp-tools/page` | `/docs/cua-driver/reference/mcp-tools/pointer-cursor-and-browser#page-tools` | 6 |
| `/docs/cua-driver/reference/mcp-tools/browser-input` | `/docs/cua-driver/reference/mcp-tools/pointer-cursor-and-browser#browser-input-tools` | 8 |
| `/docs/cua-driver/reference/mcp-tools/batch` | `/docs/cua-driver/reference/mcp-tools/batch-recording-and-settings#batch-and-script-tools` | 14 |
| `/docs/cua-driver/reference/mcp-tools/recording` | `/docs/cua-driver/reference/mcp-tools/batch-recording-and-settings#recording-tools` | 11 |
| `/docs/cua-driver/reference/mcp-tools/maintenance` | `/docs/cua-driver/reference/mcp-tools/batch-recording-and-settings#configuration-and-maintenance-tools` | 7 |
| `/docs/lume/reference/cli/vms` | `/docs/lume/reference/cli#vms` | 10 |
| `/docs/lume/reference/cli/images` | `/docs/lume/reference/cli#images` | 4 |
| `/docs/lume/reference/cli/guest` | `/docs/lume/reference/cli#guest-access` | 5 |
| `/docs/lume/reference/cli/server` | `/docs/lume/reference/cli#server` | 3 |
| `/docs/lume/reference/cli/config` | `/docs/lume/reference/cli#configuration` | 4 |
| `/docs/lume/reference/cli/updates` | `/docs/lume/reference/cli#updates` | 1 |
| `/docs/lume/reference/mcp-tools` | `/docs/lume/reference/http-api#mcp-tools` | 2 |
| `/docs/cua-bench/reference/cli/run` | `/docs/cua-bench/reference/cli#cb-run` | 3 |
| `/docs/cua-bench/reference/cli/task` | `/docs/cua-bench/reference/cli#cb-task` | 5 |
| `/docs/cua-bench/reference/cli/dataset` | `/docs/cua-bench/reference/cli#cb-dataset` | 8 |
| `/docs/cua-bench/reference/cli/trace` | `/docs/cua-bench/reference/cli#cb-trace` | 3 |
| `/docs/cua-bench/reference/cli/env` | `/docs/cua-bench/reference/cli#sandboxes-and-account` | 3 |
| `/docs/cua-bench/reference/cli/agent` | `/docs/cua-bench/reference/cli#cb-agent` | 3 |
| `/docs/cua-bench/reference/cli/images` | `/docs/cua-bench/reference/cli#images-and-platforms` | 3 |
| `/docs/cua-bench/reference/results` | `/docs/cua-bench/reference/task-definition#result-files` | 6 |

## Generated reference: SDK and Spaces (PR 3)

| Old path | New path | Views (30 days) |
| --- | --- | --- |
| `/docs/cua-sdk/reference/agent-setup` | `/docs/cua-sdk/reference/agents#agent-setup` | 5 |
| `/docs/cua-sdk/reference/local` | `/docs/cua-sdk/reference/cua#local-runtimes` | 3 |
| `/docs/cua-sdk/reference/telemetry` | `/docs/cua-sdk/reference/auth#telemetry` | 1 |
| `/docs/cua-sdk/reference/sandbox/lifecycle` | `/docs/cua-sdk/reference/sandbox#lifecycle` | 4 |
| `/docs/cua-sdk/reference/sandbox/options` | `/docs/cua-sdk/reference/image#create-options` | 2 |
| `/docs/cua-sdk/reference/sandbox/without-spacesd` | `/docs/cua-sdk/reference/spacesd#without-cua-spacesd` | 13 |
| `/docs/cua-sdk/reference/spacesd/shell` | `/docs/cua-sdk/reference/spacesd#processes` | 2 |
| `/docs/cua-sdk/reference/spacesd/media` | `/docs/cua-sdk/reference/spacesd/computer#media` | 2 |
| `/docs/cua-sdk/reference/spaces/create` | `/docs/cua-sdk/reference/spaces#create-options` | 4 |
| `/docs/cua-sdk/reference/spaces/agents` | `/docs/cua-sdk/reference/spaces#agents-in-a-space` | 3 |
| `/docs/cua-sdk/reference/spaces/windows` | `/docs/cua-sdk/reference/spaces/space#windows-icons-and-displays` | 3 |
| `/docs/cua-sdk/reference/spaces/teleport` | `/docs/cua-sdk/reference/spaces/space#teleport` | 6 |
| `/docs/cua-sdk/reference/spaces/cloud` | `/docs/cua-sdk/reference/spaces/persistent-agents#your-cloud` | 2 |
| `/docs/cua-sdk/reference/spaces/volume-sync` | `/docs/cua-sdk/reference/spaces/volume#cua-volume-storage-and-sync` | 3 |
| `/docs/cua-sdk/reference/spaces/devices` | `/docs/cua-sdk/reference/spaces/host#this-device-on-the-relay` | 2 |
| `/docs/cua-sdk/reference/spaces/decoded-media` | `/docs/cua-sdk/reference/spacesd/computer#media` | 2 |
| `/docs/cua-sdk/reference/spaces/teleport-host` | `/docs/cua-sdk/reference/spaces/space#teleport` | 2 |
| `/docs/cua-sdk/reference/spaces/teleport-apps` | `/docs/cua-sdk/reference/spaces/space#teleport` | 5 |
| `/docs/cua-sdk/reference/python/configuration` | `/docs/cua-sdk/reference/python/runtimes#configuration-and-errors` | 1 |
| `/docs/cua-sdk/reference/python/image` | `/docs/cua-sdk/reference/python/sandbox#images-and-registries` | 15 |
| `/docs/cua-sdk/reference/python/services` | `/docs/cua-sdk/reference/python/sandbox#services-tunnels-and-mcp` | 9 |
| `/docs/cua-sdk/reference/python/pool` | `/docs/cua-sdk/guides/cua-cloud-ended` | 15 |
| `/docs/cua-sdk/reference/typescript/spaces-host` | `/docs/cua-sdk/reference/typescript#trycuacuaspaceshost` | 2 |
| `/docs/cua-sdk/reference/typescript/spaces-transport` | `/docs/cua-sdk/reference/typescript#trycuacuaspacestransport` | 1 |
| `/docs/cua-sdk/reference/protocol/env-common` | `/docs/cua-sdk/reference/protocol/env-system` | 2 |
| `/docs/cua-sdk/reference/protocol/env-diagnose` | `/docs/cua-sdk/reference/protocol/env-system` | 2 |
| `/docs/cua-sdk/reference/protocol/env-filesystem` | `/docs/cua-sdk/reference/protocol/env-process` | 2 |
| `/docs/cua-sdk/reference/protocol/env-windows` | `/docs/cua-sdk/reference/protocol/env-computer` | 3 |
| `/docs/cua-sdk/reference/protocol/env-teleport-tunnel` | `/docs/cua-sdk/reference/protocol/env-stream` | 1 |
| `/docs/cua-sdk/reference/protocol/env-host-spaces` | `/docs/cua-sdk/reference/protocol/env-stream` | 2 |
| `/docs/cua-sdk/reference/protocol/daemon-sandboxes` | `/docs/cua-sdk/reference/protocol/daemon` | 6 |
| `/docs/cua-sdk/reference/protocol/daemon-runtimes` | `/docs/cua-sdk/reference/protocol/daemon` | 3 |
| `/docs/cua-sdk/reference/protocol/daemon-spaces` | `/docs/cua-sdk/reference/protocol/daemon` | 1 |
| `/docs/cua-sdk/reference/protocol/daemon-service` | `/docs/cua-sdk/reference/protocol/daemon` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core` | `/docs/cua-sdk/reference/spaces` | 1 |
| `/docs/cua-sdk/reference/spaces/account-app-core` | `/docs/cua-sdk/reference/spaces` | 1 |
| `/docs/cua-sdk/reference/spaces/agents-app-core` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/machines-app-core` | `/docs/cua-sdk/reference/spaces` | 1 |
| `/docs/cua-sdk/reference/spaces/volume-app-core` | `/docs/cua-sdk/reference/spaces/volume` | 13 |
| `/docs/cua-sdk/reference/spaces/app-checklist-palette` | `/docs/cua-sdk/reference/spaces` | 1 |
| `/docs/spaces/reference/lifecycle` | `/docs/spaces/reference/spaces#space-lifecycle` | 5 |
| `/docs/spaces/reference/commands-and-files` | `/docs/spaces/reference/spaces#commands-and-files` | 3 |
| `/docs/spaces/reference/streams-and-display` | `/docs/spaces/reference/spaces#streams-and-display` | 4 |
| `/docs/spaces/reference/services` | `/docs/spaces/reference/spaces#services` | 4 |
| `/docs/spaces/reference/hotspot` | `/docs/spaces/reference/spaces#hotspot` | 4 |
| `/docs/spaces/reference/sharing` | `/docs/spaces/reference/spaces#sharing` | 4 |
| `/docs/spaces/reference/cloud` | `/docs/spaces/reference/spaces#your-cloud` | 146 |
| `/docs/spaces/reference/persistent-agents` | `/docs/spaces/reference/agents#persistent-agents` | 9 |
| `/docs/spaces/reference/teleport` | `/docs/spaces/reference/agents#session-teleport` | 325 |
| `/docs/spaces/reference/keyvault` | `/docs/spaces/reference/agents#keyvault` | 4 |
| `/docs/spaces/reference/volume` | `/docs/spaces/reference/agents#cua-volume` | 4 |
| `/docs/spaces/reference/errors` | `/docs/spaces/reference#errors` | 3 |
| `/docs/cua-sdk/reference/spaces/app-core/about` | `/docs/cua-sdk/reference/spaces` | 3 |
| `/docs/cua-sdk/reference/spaces/app-core/agent-keys` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core/cloud` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core/creates` | `/docs/cua-sdk/reference/spaces` | 3 |
| `/docs/cua-sdk/reference/spaces/app-core/devices` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core/drag-trigger` | `/docs/cua-sdk/reference/spaces` | 3 |
| `/docs/cua-sdk/reference/spaces/app-core/experiments` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core/images` | `/docs/cua-sdk/reference/spaces` | 5 |
| `/docs/cua-sdk/reference/spaces/app-core/keyvault-screens` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core/keyvault` | `/docs/cua-sdk/reference/spaces` | 3 |
| `/docs/cua-sdk/reference/spaces/app-core/new-space` | `/docs/cua-sdk/reference/spaces` | 3 |
| `/docs/cua-sdk/reference/spaces/app-core/notch` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core/onboarding-settings` | `/docs/cua-sdk/reference/spaces` | 3 |
| `/docs/cua-sdk/reference/spaces/app-core/power` | `/docs/cua-sdk/reference/spaces` | 1 |
| `/docs/cua-sdk/reference/spaces/app-core/run-on` | `/docs/cua-sdk/reference/spaces` | 3 |
| `/docs/cua-sdk/reference/spaces/app-core/share` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core/spaces` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core/stream` | `/docs/cua-sdk/reference/spaces` | 3 |
| `/docs/cua-sdk/reference/spaces/app-core/telemetry` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core/teleport-grid` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core/teleport-review` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core/teleport-run` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core/teleport-transfer` | `/docs/cua-sdk/reference/spaces` | 3 |
| `/docs/cua-sdk/reference/spaces/app-core/teleport` | `/docs/cua-sdk/reference/spaces` | 2 |
| `/docs/cua-sdk/reference/spaces/app-core/this-machine` | `/docs/cua-sdk/reference/spaces` | 3 |
| `/docs/cua-sdk/reference/spaces/app-core/window` | `/docs/cua-sdk/reference/spaces` | 5 |
| `/docs/cua-sdk/reference/rust/cua-fleet` | `/docs/cua-sdk/guides/cua-cloud-ended` | 3 |
| `/docs/cua-sdk/reference/rust/cua-sandbox-core` | `/docs/cua-sdk/reference/rust` | 2 |
| `/docs/cua-sdk/reference/rust/cua-spaces` | `/docs/cua-sdk/reference/rust` | 3 |
| `/docs/cua-sdk/reference/rust/cua-spaces-client` | `/docs/cua-sdk/reference/rust` | 0 |
| `/docs/cua-sdk/reference/rust/cua-spaces-agents-mcp` | `/docs/cua-sdk/reference/rust` | 3 |

Every old URL redirects (docs/content/docs/redirects.json). Existing redirects that pointed at a removed page were repointed at its new home.
