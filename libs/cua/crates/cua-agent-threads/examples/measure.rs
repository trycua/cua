//! Times `recent` and `history` on a synthetic, realistic history:
//! `cargo run --release -p cua-agent-threads --example measure -- <dir>`
//! writes (once) 300 Claude Code chats (6 of them 150 MB of tool output) and
//! 60 Codex rollouts under `<dir>`, then times the lister and the reader.
use cua_agent_threads::*;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

fn claude_chat(path: &Path, id: &str, turns: usize, blob: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let pad = "x".repeat(blob);
    for n in 0..turns {
        let ts = format!("2026-11-04T10:{:02}:{:02}.000Z", (n / 60) % 60, n % 60);
        writeln!(f, r#"{{"type":"user","uuid":"u{n}","cwd":"/Users/sam/code/kettle","timestamp":"{ts}","message":{{"role":"user","content":"question {n} about the kettle"}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"user","uuid":"t{n}","timestamp":"{ts}","message":{{"role":"user","content":[{{"type":"tool_result","content":"{pad}"}}]}}}}"#).unwrap();
        writeln!(f, r#"{{"type":"assistant","uuid":"a{n}","timestamp":"{ts}","message":{{"role":"assistant","content":[{{"type":"text","text":"answer {n} for {id}"}}]}}}}"#).unwrap();
    }
}

fn make(dir: &Path) {
    for i in 0..300 {
        let id = format!("00000000-0000-4000-8000-{i:012}");
        let big = i % 50 == 0;
        // 6 chats of about 150 MB; the rest 20 KB to 2 MB.
        let (turns, blob) = if big {
            (1500, 100_000)
        } else {
            (20 + (i % 7) * 10, 1_000 + (i % 13) * 1_000)
        };
        claude_chat(
            &dir.join(format!(
                ".claude/projects/-Users-sam-p{}/{id}.jsonl",
                i % 20
            )),
            &id,
            turns,
            blob,
        );
    }
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("fixture dir"));
    if !dir.join(".claude").exists() {
        make(&dir);
    }
    let roots = Roots::under_home(&dir);
    for limit in [4usize, 50] {
        let t = Instant::now();
        let r = recent(
            &roots,
            &ListOptions {
                agents: Agent::ALL.to_vec(),
                limit,
            },
        );
        println!(
            "recent limit={limit}: {} chats in {:?}",
            r.len(),
            t.elapsed()
        );
    }
    let mut files: Vec<_> = walk(&dir.join(".claude/projects"));
    files.sort_by_key(|f| std::cmp::Reverse(std::fs::metadata(f).unwrap().len()));
    for f in files.iter().take(2).chain(files.iter().rev().take(1)) {
        let id = f.file_stem().unwrap().to_str().unwrap();
        let t = Instant::now();
        let h = history(&roots, Agent::ClaudeCode, id, None).unwrap();
        println!(
            "history {} MB chat: {} messages ({} older) in {:?}",
            std::fs::metadata(f).unwrap().len() / 1_000_000,
            h.messages.len(),
            h.earlier_omitted,
            t.elapsed()
        );
    }
}

fn walk(d: &Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    for e in std::fs::read_dir(d).unwrap().flatten() {
        if e.path().is_dir() {
            out.extend(walk(&e.path()));
        } else {
            out.push(e.path());
        }
    }
    out
}
