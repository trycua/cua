# A durable run's journal from cua 0.8.0

Used by `../../upgrade-080.test.mjs`, which re-drives it with this runner, and
by `cua-spaces-ext/tests/upgrade_durable_080.rs`, which ships it to the Volume
and restores it.

cua 0.8.0 (tag `cua-sdk-v0.8.0`, `63b54fd55d`) has durable runs (`cua agent run
--durable`). Its journal stays in the Space's state directory, and nothing
ships it to the Volume. To write this journal, the 0.8.0 `runner/` directory
was taken with `git archive cua-sdk-v0.8.0 libs/cua/crates/cua-agents/runner`
and run with node 26 and `@agentclientprotocol/sdk@1.5.0` (the pinned
version). It ran against the 0.8.0 mock ACP agent (`test/mock-acp-agent.mjs`),
through the 0.8.0 `test/helpers.mjs` `Box`, in a throwaway directory. There was
no model and no network.

The run used the same script as the 0.8.0 test "a crash between tool start and
outcome": `call-1` completes ("wrote 3 files"), then the runner is killed
(SIGKILL) after `call-2` was approved and before its outcome.

```js
box.script({ prompts: [{ match: ".", steps: [
  tool("call-1", { output: "wrote 3 files" }),
  tool("call-2", { killRunnerBeforeOutcome: true }),
  { say: "done" } ] }] });
box.config();
await box.go({ epoch: 1, resume: false });   // exit: SIGKILL
```

- `state/`: the state directory as the killed runner left it (`epoch` 1,
  `events.jsonl`, `tools.jsonl` with `intent:call-1`, `outcome:call-1` and
  `intent:call-2`, and `state.json`).
- `mock/`: the mock agent's `sessions.json` (so the session can be loaded
  again) and `effects.log` (`call-1`, `call-2`: the tools that ran).

Nothing was edited. The files hold no host path, key or token.
