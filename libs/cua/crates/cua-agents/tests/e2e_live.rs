//! Live agent runs against a real sandbox. Opt-in: without
//! `CUA_AGENTS_E2E_URL` every test prints why and passes. Run through
//! `tests/e2e/run-agents-e2e.sh`, which starts the sandbox container and the
//! scripted mock provider (cua-mock-llm) next to it.
//!
//! Env: `CUA_AGENTS_E2E_URL`, `CUA_AGENTS_E2E_TOKEN` (the spacesd),
//! `CUA_AGENTS_E2E_ENDPOINT` (model base URL as the sandbox sees it),
//! `CUA_AGENTS_E2E_KEY` (the key the endpoint expects; never printed),
//! `CUA_AGENTS_E2E_HARNESSES` (comma list, default claude-code),
//! `CUA_AGENTS_E2E_EVIDENCE` (directory for event logs).
//!
//! Every model reply is scripted by the mock provider; the harness, its
//! install, its tool execution in the sandbox, credential delivery,
//! follow-ups, interrupts and reattachment are real.

use cua_agents::harness::{self, Endpoint};
use cua_agents::{Agents, RunOptions, RunStatus};
use std::collections::BTreeMap;
use std::time::Duration;

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

async fn connect() -> Agents {
    let url = env("CUA_AGENTS_E2E_URL").unwrap();
    let guest = cua_spacesd_client::SpacesdClient::connect_url(&url, env("CUA_AGENTS_E2E_TOKEN"))
        .await
        .expect("connect to the spacesd");
    Agents::new(guest).await.unwrap()
}

struct Log(std::fs::File);
impl Log {
    fn line(&mut self, s: &str) {
        use std::io::Write;
        println!("{s}");
        let _ = writeln!(self.0, "{s}");
    }
}

/// Reads events from `cursor` until `pred` matches or `secs` pass
/// (bounded: at most one read per 300 ms).
async fn follow(
    a: &Agents,
    run: &str,
    cursor: &mut u64,
    log: &mut Log,
    secs: u64,
    mut pred: impl FnMut(&cua_agents::AgentEvent) -> bool,
) -> bool {
    for _ in 0..(secs * 10 / 3) {
        let page = a.events(run, *cursor, 200).await.unwrap();
        *cursor = page.cursor;
        for e in &page.events {
            if let Some(r) = e.render() {
                log.line(&format!(
                    "  #{:<3} t{} {:<14} {}",
                    e.seq,
                    e.turn,
                    e.kind,
                    r.lines().next().unwrap_or("")
                ));
            } else {
                log.line(&format!("  #{:<3} t{} {}", e.seq, e.turn, e.kind));
            }
            if pred(e) {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    false
}

#[tokio::test]
async fn e2e_harness_runs() {
    let Some(_) = env("CUA_AGENTS_E2E_URL") else {
        eprintln!("skipped: set CUA_AGENTS_E2E_URL (run tests/e2e/run-agents-e2e.sh)");
        return;
    };
    let base = env("CUA_AGENTS_E2E_ENDPOINT").expect("CUA_AGENTS_E2E_ENDPOINT");
    let key = env("CUA_AGENTS_E2E_KEY").expect("CUA_AGENTS_E2E_KEY");
    let evidence = std::path::PathBuf::from(env("CUA_AGENTS_E2E_EVIDENCE").unwrap_or_else(|| {
        std::env::temp_dir()
            .join("cua-agents-e2e")
            .display()
            .to_string()
    }));
    std::fs::create_dir_all(&evidence).unwrap();
    let list = env("CUA_AGENTS_E2E_HARNESSES").unwrap_or_else(|| "claude-code".into());
    let mut failures = vec![];
    for id in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let h = harness::harness(id).unwrap_or_else(|| panic!("unknown harness {id}"));
        let mut log = Log(std::fs::File::create(evidence.join(format!("{id}.log"))).unwrap());
        log.line(&format!(
            "== {id} ({}) against the mock provider (scripted) at {base}",
            h.name
        ));
        let a = connect().await;
        let mut env_map = BTreeMap::new();
        env_map.insert(harness::endpoint_key_var(h, None).to_string(), key.clone());
        let started = a
            .start(
                id,
                &format!("Write the proof file. mock: think planning the step; plan; say writing the proof; shell echo {id}-proof > proof.txt && cat proof.txt"),
                RunOptions {
                    env: env_map,
                    endpoint: Some(Endpoint { base_url: base.clone(), wire: None, model: Some(mock_model(h)) }),
                    label: Some("e2e".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let run = started.run_id.clone();
        log.line(&format!(
            "started {run} tag={} cwd={}",
            started.process_tag, started.cwd
        ));
        let mut cursor = 0;
        // Watch until the agent is working, then drop this client.
        let mut first = "";
        follow(&a, &run, &mut cursor, &mut log, 1200, |e| {
            first = e.kind;
            matches!(e.kind, "turn_started" | "error" | "exited")
        })
        .await;
        let working = first == "turn_started";
        drop(a);
        log.line("-- client disconnected; reattaching with a fresh connection");
        let a = connect().await;
        let s = a.status(&run).await.unwrap();
        log.line(&format!(
            "-- after reattach: status={} phase={} alive={:?}",
            s.status.as_str(),
            s.phase,
            s.alive
        ));
        let ended = working
            && follow(&a, &run, &mut cursor, &mut log, 600, |e| {
                e.kind == "turn_ended" || e.kind == "exited"
            })
            .await;
        let r = a.result(&run).await.unwrap();
        log.line(&format!(
            "-- result turn={} stop={:?} tools={} text={:?}",
            r.turn, r.stop_reason, r.tool_calls, r.text
        ));
        let arts = a.artifacts(&run).await.unwrap();
        log.line(&format!(
            "-- artifacts: {:?}",
            arts.iter().map(|x| &x.path).collect::<Vec<_>>()
        ));
        let proof_ok = ended
            && r.text.contains(&format!("{id}-proof"))
            && arts.iter().any(|x| x.path.ends_with("/proof.txt"));
        // Follow-up in the same session.
        a.send(
            &run,
            "Now a follow-up. mock: tool list_windows; say followup-ok",
            vec![],
        )
        .await
        .unwrap();
        let fu = follow(&a, &run, &mut cursor, &mut log, 300, |e| {
            e.kind == "turn_ended"
        })
        .await;
        let r2 = a.result(&run).await.unwrap();
        let followup_ok = fu && r2.turn == 2 && r2.text.contains("followup-ok");
        // The sandbox's own MCP (cua-driver through the spacesd) was called.
        let evs = a.events(&run, 0, 100_000).await.unwrap().events;
        let mcp_ok = evs.iter().any(|e| {
            e.turn == 2
                && e.kind == "tool_update"
                && e.tool_status.as_deref() == Some("completed")
                && e.raw.to_string().contains("list_windows")
        }) || evs.iter().any(|e| {
            e.turn == 2 && e.kind == "tool_call" && e.raw.to_string().contains("list_windows")
        });
        log.line(&format!(
            "-- sandbox MCP tool call (list_windows): {mcp_ok}"
        ));
        // Interrupt a slow turn.
        a.send(&run, "A long answer. mock: slow 40; say this answer streams slowly one chunk per second so it can be interrupted", vec![])
            .await
            .unwrap();
        let streaming = follow(&a, &run, &mut cursor, &mut log, 300, |e| {
            e.turn == 3 && e.kind == "message"
        })
        .await;
        a.interrupt(&run).await.unwrap();
        log.line("-- interrupt sent");
        let mut stop_reason = None;
        let cancelled = streaming
            && follow(&a, &run, &mut cursor, &mut log, 120, |e| {
                if e.kind == "turn_ended" {
                    stop_reason = e.stop_reason.clone();
                    true
                } else {
                    false
                }
            })
            .await;
        let interrupt_ok = cancelled && stop_reason.as_deref() == Some("cancelled");
        // Credentials: never in argv, the run's records or its event log.
        let procs = cua_spacesd_client::SpacesdClient::connect_url(&env("CUA_AGENTS_E2E_URL").unwrap(), env("CUA_AGENTS_E2E_TOKEN"))
            .await
            .unwrap()
            .run(cua_spacesd_client::Command::shell(format!(
                // The pattern comes from a 0600 file, so the check itself
                // never puts the key in an argv.
                "cd {d} && umask 077 && f=$(mktemp) && printf %s \"$K\" > \"$f\" && \
                 (for p in /proc/[0-9]*/cmdline; do tr '\\0' ' ' < \"$p\"; echo; done 2>/dev/null; \
                 cat meta.json run.json events.jsonl state.json launch.sh install.log agent.log 2>/dev/null) \
                 | grep -c -F -f \"$f\"; rm -f \"$f\"; true",
                d = started.run_dir
            )).env("K", key.clone()))
            .await
            .unwrap();
        let leaks: u32 = String::from_utf8_lossy(&procs.stdout)
            .trim()
            .parse()
            .unwrap_or(99);
        let creds_ok = leaks == 0;
        let stopped = a.stop(&run).await.unwrap();
        log.line(&format!(
            "== {id}: tool+artifact={proof_ok} followup={followup_ok} mcp={mcp_ok} interrupt={interrupt_ok} (stop={stop_reason:?}) key-leaks={leaks} stopped_alive={:?}",
            stopped.alive
        ));
        let runs = a.list().await.unwrap();
        assert!(runs.iter().any(|x| x.run_id == run));
        // The sandbox's own MCP is part of the contract where the harness
        // takes it by default.
        let mcp_required = h.sandbox_mcp;
        if !(proof_ok
            && followup_ok
            && (mcp_ok || !mcp_required)
            && interrupt_ok
            && creds_ok
            && stopped.alive == Some(false))
        {
            failures.push(id.to_string());
        }
        assert_ne!(stopped.status, RunStatus::Running);
    }
    assert!(failures.is_empty(), "harnesses that failed: {failures:?}");
}

fn mock_model(h: &harness::Harness) -> String {
    match h.wires[0] {
        harness::Wire::Anthropic => "claude-mock-1",
        harness::Wire::Gemini => "gemini-2.5-pro",
        _ => "gpt-mock-1",
    }
    .into()
}
