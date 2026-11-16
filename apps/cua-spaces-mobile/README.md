# Cua Spaces for iPhone, iPad and Android

React Native (Expo) client for Cua Spaces: your Spaces grouped by computer, each Space as a chat with its agents, approvals with Deny and Allow, and the live desktop one tap away. Design and the reasons for the choices: [`docs/DESIGN.md`](docs/DESIGN.md). Accounts, keys and release setup: [`docs/SETUP.md`](docs/SETUP.md). Licensed FSL-1.1-MIT like the other Cua Spaces apps.

## What is real and what is not available yet

| Area                                                                                                                                            | State                                                                                                                                                                                                                                                                                  |
| ----------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Home (a bar of live thumbnails above the agent list), thread, composer with @mentions, approval cards, status pill, the stream screen, Settings | Built. Seen on the iOS Simulator with labelled sample data.                                                                                                                                                                                                                            |
| Start a task from the phone (a box, an agent, where it runs, "Add a repo"; the job becomes the first message `@agent job` as on the web Home)   | Built against the daemon's `task_options`, `task_start` and `task_status` (a fake relay and daemon in the tests; see `docs/E2E.md` for what ran against the local harness). Example jobs come from the core's prompt library (`firstPrompts.choose`) once it is in the core.           |
| Tool-call rows                                                                                                                                  | Built: the daemon's `thread_tool_rows` feed with a long poll, the group phrase from the app core. The sample build answers in the same wire shape through the same mapper.                                                                                                             |
| Offline: an outbox that survives a restart, cached threads and Spaces list                                                                      | Built. Sealed on disk (AES-256-GCM, key in the Keychain), exactly once on reconnect (the core's outbox state machine, the same message semantics as onboarding's queue). A sample build keeps nothing between launches unless `EXPO_PUBLIC_CUA_SAMPLE_PERSIST=1`.                      |
| Approvals: allow once, allow for this Space, decline, "1 of 3"; Queue and Steer while an agent runs                                             | Built from the daemon's structured data (`approval.kind`, `scope_choices`, `agent_message force`). Buttons and words are the core's, never model text.                                                                                                                                 |
| Voice: hold the microphone button to dictate                                                                                                    | Built on the phone's own speech recognizer (on the phone where the language supports it). No Cua server transcribes. Permission states and errors in the core's words. The recognizer was run for real on the Simulator for the permission and silence paths only (see `docs/E2E.md`). |
| Diff review: the changed files, a unified and a word diff, image and text preview                                                               | Built on the daemon's `space_changes`, `space_file_diff`, `space_file_preview`; reachable from the thread header and from the push approval card.                                                                                                                                      |
| Push notifications (client side)                                                                                                                | Built: token registration with the relay, tap routing to the thread or approval, Settings. Tested against a mock push provider. Needs an APNs key and the relay's sender to reach a closed app (`docs/SETUP.md`).                                                                      |
| Sign-in with cua.ai (PKCE in the system sheet), tokens in the Keychain                                                                          | Built, tested with unit tests. Needs a public OAuth client for the phone to be registered. Not run against the real provider.                                                                                                                                                          |
| Relay computers, device enrollment, thread read and post, stream ticket, touch input                                                            | Built against the relay's and the daemon's documented protocol, tested against a fake relay and a fake host daemon, and run against a local relay and daemon in an isolated home (`docs/E2E.md`). The signing is the Rust core's, checked against `cua_host::device`.                  |
| The live desktop                                                                                                                                | The daemon's HTML5 viewer in a web view. Whether the production relay lets the viewer's own requests through without the account header is unverified; a native media client is the later step.                                                                                        |
| Sleep from a swipe                                                                                                                              | Sample data only. Hidden on a live account (the daemon has no phone-reachable call for it yet).                                                                                                                                                                                        |
| Android                                                                                                                                         | Same JS. The Rust core library is not built for Android yet (the screens say so), so there is no outbox, no dictation and no push there. No Android build was made.                                                                                                                    |
| Attachments, hosting a Space on the phone                                                                                                       | Not available yet (hosting is out of scope).                                                                                                                                                                                                                                           |

## Run it

```sh
pnpm install
rustup target add aarch64-apple-ios aarch64-apple-ios-sim --toolchain 1.97.1
pnpm core:ios                  # builds the Rust app core into modules/cua-core/ios
npx expo prebuild --platform ios
# open ios/CuaSpaces.xcworkspace, or: npx expo run:ios
```

Sample data, no account: build with `EXPO_PUBLIC_CUA_SAMPLE=1` (a labelled sample-data build: no sign-in, made up Spaces; it is how the screenshots and the end-to-end flow run). A normal build cannot reach it.

## Checks

```sh
pnpm check                      # tsc, the proto drift check, jest
(cd core && cargo test)         # the core binding: parity flows and the phone's flows through its entry point, the fixtures, the sealed store
```

`core/fixtures/results.json` is what the React Native tests read for the core's words; `cargo test --test fixtures` keeps it equal to the real core (`UPDATE_FIXTURES=1` rewrites it). After a test starts asking the core something new, `scripts/record-core-fixtures.sh` records the calls and has the real core answer them.
