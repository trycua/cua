//! The KILL RULE as a lint: nothing in the chaos tooling or the compat chaos
//! scripts may use a pattern kill (`pkill`, `killall`, `pgrep`) or a broad
//! signal. Chaos signals only PIDs it recorded when it started the target.

use std::path::{Path, PathBuf};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else if p
            .extension()
            .is_some_and(|x| ["sh", "py", "json", "yml", "rs"].iter().any(|y| x == *y))
        {
            out.push(p);
        }
    }
}

#[test]
fn no_pattern_kills_in_chaos_scripts_or_tooling() {
    let me = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = me.join("../../../..");
    let mut all = Vec::new();
    files(&root.join("tests/compat/chaos"), &mut all);
    files(&me.join("src"), &mut all);
    files(&root.join("libs/cua/crates/cua-compat/tests"), &mut all);
    all.push(root.join(".github/workflows/chaos-cua-compat.yml"));
    assert!(all.len() > 5, "found {} files", all.len());
    let mut bad = Vec::new();
    for f in all {
        // The lint's own word list and tests mention the forbidden words.
        if f.ends_with("inject.rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        for (n, l) in text.lines().enumerate() {
            if ["pkill", "killall", "pgrep"].iter().any(|w| l.contains(w)) {
                bad.push(format!("{}:{}: {}", f.display(), n + 1, l.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "pattern kills are forbidden:\n{}",
        bad.join("\n")
    );
}
