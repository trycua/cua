//! Property tests of the translation layer: whatever the client sends, the
//! parsers do not panic and the invariants the wire relies on hold.

use crate::modal::dockerfile::{self, Exec, Instr};
use crate::modal::exec::{self, Config, Exit, OutBuf, STDOUT};
use crate::modal::fstools;
use crate::modal::sandbox::take_utf8;
use crate::modal::volumes::rel_path;
use crate::modal::{ids, jwt};
use proptest::prelude::*;

fn shellish() -> impl Strategy<Value = String> {
    // Words, quotes, escapes, dollars, newlines: the characters that matter.
    proptest::collection::vec(
        prop_oneof![
            "[a-zA-Z0-9_./=-]{1,8}",
            Just(" ".to_string()),
            Just("\"".to_string()),
            Just("'".to_string()),
            Just("\\".to_string()),
            Just("$".to_string()),
            Just("${".to_string()),
            Just("}".to_string()),
            Just("\n".to_string()),
            Just("#".to_string()),
            Just("<<".to_string()),
        ],
        0..24,
    )
    .prop_map(|v| v.concat())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn dockerfile_parser_never_panics(text in "\\PC{0,200}") {
        let _ = dockerfile::parse(&text);
        let _ = dockerfile::logical_lines(&text);
    }

    #[test]
    fn dockerfile_parser_never_panics_on_dockerfile_like_text(
        kw in prop_oneof![Just("RUN"), Just("ENV"), Just("COPY"), Just("ADD"), Just("FROM"), Just("ARG"), Just("CMD"), Just("WORKDIR"), Just("USER"), Just("SHELL"), Just("LABEL")],
        rest in shellish(),
    ) {
        let _ = dockerfile::parse(&format!("{kw} {rest}"));
    }

    #[test]
    fn expansion_never_panics_and_keeps_plain_text(s in shellish(), v in "[a-z]{0,4}") {
        let env = std::collections::BTreeMap::from([("A".to_string(), v.clone())]);
        let _ = dockerfile::expand(&s, &env);
        let plain = s.replace(['$', '\\'], "");
        prop_assert_eq!(dockerfile::expand(&plain, &env), plain);
    }

    #[test]
    fn run_lines_roundtrip(cmds in proptest::collection::vec("[a-zA-Z0-9_./=:][a-zA-Z0-9 _./=:-]{0,29}", 1..6)) {
        let text = std::iter::once("FROM base".to_string())
            .chain(cmds.iter().map(|c| format!("RUN {c}")))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed = dockerfile::parse(&text).unwrap();
        prop_assert_eq!(parsed.len(), cmds.len() + 1);
        for (p, c) in parsed[1..].iter().zip(&cmds) {
            prop_assert_eq!(p, &Instr::Run(Exec::Shell(c.trim().to_string())));
        }
    }

    #[test]
    fn env_pairs_roundtrip(pairs in proptest::collection::vec(("[A-Z_][A-Z0-9_]{0,6}", "[a-zA-Z0-9_./:-]{0,12}"), 1..5)) {
        let line = format!(
            "ENV {}",
            pairs.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect::<Vec<_>>().join(" ")
        );
        match &dockerfile::parse(&line).unwrap()[0] {
            Instr::Env(got) => prop_assert_eq!(got, &pairs),
            other => prop_assert!(false, "{other:?}"),
        }
    }

    #[test]
    fn utf8_chunking_equals_lossy_decoding(bytes in proptest::collection::vec(any::<u8>(), 0..300), cuts in proptest::collection::vec(0usize..300, 0..8)) {
        let mut points: Vec<usize> = cuts.into_iter().map(|c| c.min(bytes.len())).collect();
        points.push(0);
        points.push(bytes.len());
        points.sort_unstable();
        let mut out = String::new();
        let mut carry = Vec::new();
        for w in points.windows(2) {
            carry.extend_from_slice(&bytes[w[0]..w[1]]);
            out.push_str(&take_utf8(&mut carry));
        }
        out.push_str(&String::from_utf8_lossy(&carry));
        prop_assert_eq!(out, String::from_utf8_lossy(&bytes).into_owned());
    }

    #[test]
    fn outbuf_serves_every_retained_byte(pieces in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..50), 0..20), probe in 0usize..2000, max in 1usize..64) {
        let mut b = OutBuf::default();
        let mut all = Vec::new();
        for p in &pieces {
            b.push(p);
            all.extend_from_slice(p);
        }
        prop_assert_eq!(b.total(), all.len() as u64);
        let off = probe.min(all.len());
        let got = b.slice(off as u64, max).unwrap();
        let end = (off + max).min(all.len());
        prop_assert_eq!(got, &all[off..end]);
    }

    #[test]
    fn volume_paths_cannot_escape(p in "[a-zA-Z0-9_./ -]{0,40}") {
        if let Ok(rel) = rel_path(&p) {
            prop_assert!(!rel.starts_with('/'));
            prop_assert!(rel.split('/').all(|c| c != ".." && c != "." ));
            prop_assert_eq!(rel_path(&rel).unwrap(), rel.clone());
        }
    }

    #[test]
    fn tokens_verify_only_untouched(sub in "[a-zA-Z0-9-]{1,30}", scope in prop_oneof![Just("tcr"), Just("api"), Just("vol")], flip in 0usize..300) {
        let s = jwt::Signer::new("secret");
        let t = s.mint(scope, &sub, 60);
        let claims = s.verify(&t, scope).unwrap();
        prop_assert_eq!(claims["sub"].as_str().unwrap(), sub.as_str());
        let mut bytes = t.clone().into_bytes();
        let i = flip % bytes.len();
        bytes[i] = if bytes[i] == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(bytes).unwrap();
        if tampered != t {
            prop_assert!(s.verify(&tampered, scope).is_err());
        }
    }

    #[test]
    fn permissions_strings_are_ten_chars(mode in 0u32..0o10000, kind in prop_oneof![Just("file"), Just("directory"), Just("symlink")]) {
        let p = fstools::permissions_string(kind, mode);
        prop_assert_eq!(p.chars().count(), 10);
        prop_assert!(p.chars().skip(1).all(|c| "rwxsStT-".contains(c)));
    }

    #[test]
    fn ids_always_have_the_clients_shape(_n in 0u8..50) {
        prop_assert!(ids::is_id(&ids::new_id("sb"), "sb"));
        prop_assert!(!ids::is_id(&ids::new_id_v2("sb"), "sb"));
        prop_assert!(!ids::new_task_id().ends_with('V'));
        prop_assert!(ids::new_task_id_v2().ends_with('V'));
    }
}

/// Stdin offsets make writes idempotent: however a client's writes are
/// retried or overlapped, the process sees each byte once, in order.
#[derive(Debug, Clone)]
struct W {
    from: usize,
    to: usize,
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn stdin_writes_are_exactly_once(
        data in proptest::collection::vec(any::<u8>(), 1..200),
        raw in proptest::collection::vec((0usize..200, 0usize..200), 1..20),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let (sink, mut rx) = fstools::channel();
            let s = exec::internal(
                Some(sink),
                &Config { discard_stdout: false, discard_stderr: false, merge_stderr: false },
            );
            let collector = tokio::spawn(async move {
                let mut got = Vec::new();
                while let Some(Some(chunk)) = rx.recv().await {
                    got.extend_from_slice(&chunk);
                }
                got
            });
            // Writes that start at or before the bytes written so far, in
            // any overlap, then the rest.
            let mut written = 0usize;
            let ws: Vec<W> = raw
                .into_iter()
                .map(|(a, b)| {
                    let from = a.min(data.len());
                    let to = b.max(from).min(data.len());
                    W { from, to }
                })
                .collect();
            for w in ws {
                if w.from > written {
                    // A gap fails and writes nothing.
                    let r = s.write_stdin(w.from as u64, &data[w.from..w.to], false).await;
                    assert!(matches!(r, Err(exec::StdinErr::Gap { .. })));
                    continue;
                }
                s.write_stdin(w.from as u64, &data[w.from..w.to], false).await.unwrap();
                written = written.max(w.to);
            }
            s.write_stdin(written as u64, &data[written..], true).await.unwrap();
            // A retry of the final write after EOF is accepted and writes nothing.
            s.write_stdin(written as u64, &data[written..], true).await.unwrap();
            drop(s);
            let got = collector.await.unwrap();
            assert_eq!(got, data);
        });
    }
}

#[test]
fn exit_status_shapes() {
    assert_eq!(Exit::code(3).status(), (Some(3), None));
    assert_eq!(
        Exit {
            signal: Some(9),
            ..Default::default()
        }
        .status(),
        (None, Some(9))
    );
    assert_eq!(Exit::default().status(), (Some(127), None));
    let _ = STDOUT;
}
