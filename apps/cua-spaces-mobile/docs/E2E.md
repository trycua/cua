# End to end: what ran for real and what is sample

Nothing here touched a real account, the production relay, the installed Cua app, `~/.cua`, the Keychain or any real service. Everything below ran on this Mac, on loopback, in an isolated home.

## The local harness

A local relay, a fake identity provider and one machine daemon (`cua-spacesd`), all started by `libs/cua-spacesd/crates/cua-relay/scripts/mobile-harness.sh start` under their own directory. The relay has push turned on with a recording provider (no Apple servers). The machine has no signed-in coding agent.

## The run: `__tests__/e2e/harness.e2e.test.ts`

Opt in: build the core helper (`cargo build --bin core-call` in `core/`), then

```sh
CUA_E2E_HARNESS=<harness dir>/env.sh CUA_E2E_CORE_CALL=<target>/debug/core-call npx jest __tests__/e2e
```

Without `CUA_E2E_HARNESS` the file does nothing, like every test that needs a host. It drives the app's real live backend (`createLiveBackend`, the relay client, the device manager, the Connect transport) with the real Rust core answering the app's core calls (`core/src/bin/core-call.rs`).

| Step                                                                                                    | Result                                                                                                                                                                                                                                                 |
| ------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Sign in with a token from the harness identity provider (the dev sign-in the app has for this)          | Real.                                                                                                                                                                                                                                                  |
| Enroll this phone: device key made and signed by the real core, `POST /v1/devices`, device session      | Real. Enrolled and a session was issued.                                                                                                                                                                                                               |
| List machines                                                                                           | Real. The harness machine, online.                                                                                                                                                                                                                     |
| `task_options` on the machine's daemon                                                                  | Real. The daemon listed its agents (all not ready: not signed in) and said it can create a Space.                                                                                                                                                      |
| `GET /v1/push`, register and unregister a phone for push (body built by the core's `push.registration`) | Real against the local relay. It said `configured: true` and accepted both. The APNs token is made up; no push reached any phone.                                                                                                                      |
| `task_start`                                                                                            | Real daemon answer, shown as a failed task: "the built-in Linux runtime cannot run from ... (its path is 132 bytes; Unix sockets allow 103)". That is the harness directory being too long, not the phone. The app shows a failed task with Try again. |

The first attempt at push registration used a hand written body without `platform`, and the relay refused it in words. The test now uses the core's `push.registration` body, which is also the only body the app sends.

## Not run for real

- A task that starts and runs: the harness has no signed-in coding agent, and its Linux runtime did not start. So the live thread, tool rows, approval scopes, Queue and Steer, and the diff screens were not driven against a real daemon with a real agent. They are tested against a fake daemon speaking the daemon's wire shapes (reconciled with the daemon's code), and shown on the Simulator through the sample backend, which answers in those same shapes through the same mappers.
- The iOS app against the harness (the dev sign-in code is in the app, `EXPO_PUBLIC_CUA_DEV_TOKEN_URL`, and was exercised here through the same session code path only as far as the token step).
- Real push delivery (needs the APNs key, see `SETUP.md`), real sign-in with cua.ai, a real device, TestFlight.
- Dictation: the recognizer ran for real on the Simulator for the idle and silent press paths. The listening state (words filling in) is covered by tests with a fake recognizer only. The refused-permission state cannot be set from the command line on the Simulator, so it is covered by tests and the core's golden, not a screenshot.

## Screenshots

Real renders from the Simulator (iPhone and iPad, light and dark) built with sample data (`EXPO_PUBLIC_CUA_SAMPLE=1`, the screens say "Sample data"). The example-job cards appear once the core's prompt library is on main; none are shown in these.
