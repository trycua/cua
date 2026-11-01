//! fork-batch (qemu lane): the reference disk booted through
//! `Sandboxes.create(local)`, checkpointed with its memory, then
//! `Sandboxes.fork_many` of that checkpoint: every fork is streamed as it
//! becomes ready, at most `max_parallel_per_host` start at once, and each
//! fork's cua-spacesd answers with its own token. Everything it makes is
//! deleted.
// The test futures nest deeply enough that computing their layout passes
// the default query depth of 128.
#![recursion_limit = "256"]

use std::collections::HashMap;
use std::time::Duration;

use cua_sdk_e2e::*;

const MIN: Duration = Duration::from_secs(60);

fn desktop_disk() -> std::path::PathBuf {
    std::env::var("CUA_E2E_DISK_LINUX")
        .map(Into::into)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap())
                .join(".cache/cua-images-e2e/linux")
                .join(host_arch())
                .join("disk.img")
        })
}

#[tokio::test]
async fn fork_many_from_a_memory_checkpoint() {
    e2e(
        "fork-batch",
        "qemu",
        "create -> memory checkpoint -> fork_many(3, parallel 2) streamed -> each fork's env -> cleanup",
        40 * MIN,
        None,
        || async {
            let disk = desktop_disk();
            if !disk.exists() {
                return skip(format!("missing {}", disk.display()));
            }
            let c = embedded_local();
            let sbx = c.sandboxes();
            let src = name("fb-src");
            let ck = format!("{src}-ck");
            let token = hex(16);
            let mut env = HashMap::new();
            env.insert("CUA_ENV_TOKEN".to_string(), token.clone());
            let opts = cua::SandboxCreateOptions {
                on: Some("local".into()),
                image: format!("vm:{}", disk.display()),
                name: Some(src.clone()),
                token: Some(token),
                env,
                cpus: Some(2),
                memory_mb: Some(1536),
                ports: vec![3211],
                wait_for: vec![cua::ReadinessProbe {
                    port: 3211,
                    http_path: Some("/viewer/".into()),
                    http_status: None,
                    service: None,
                }],
                ready_timeout_ms: Some(600_000),
                ..Default::default()
            };
            let mut forks: Vec<String> = vec![];
            let r = async {
                let sb = sbx.create(opts).await?;
                wait_env(&sb, 300).await?;
                sbx.checkpoint(src.clone(), ck.clone(), false).await?;
                let batch = sbx
                    .fork_many(
                        ck.clone(),
                        3,
                        Some(cua::ForkManyOptions {
                            on: Some("local".into()),
                            max_parallel_per_host: 2,
                            name_prefix: format!("{src}-f"),
                            ..Default::default()
                        }),
                    )
                    .await?;
                let mut ready = vec![];
                let mut summary = None;
                while let Some(ev) = batch.next().await? {
                    match ev {
                        cua::ForkBatchEvent::Item { item } => {
                            assert!(summary.is_none(), "items stream before the summary");
                            forks.push(item.name.clone());
                            assert!(item.error.is_empty(), "{item:?}");
                            ready.push(item);
                        }
                        cua::ForkBatchEvent::Done { summary: s } => summary = Some(s),
                        _ => {}
                    }
                }
                let s = summary.ok_or("no summary")?;
                assert_eq!((s.ok, s.failed), (3, 0), "{s:?}");
                assert!(s.peak_parallel.get("local").copied().unwrap_or(0) <= 2, "{s:?}");
                for item in &ready {
                    assert_eq!(item.space, format!("local:{}", item.name));
                    let fork = sbx.connect(item.name.clone()).await?;
                    let env = wait_env(&fork, 120).await?;
                    sh_ok(&env, "true").await?;
                }
                Res::Ok(())
            }
            .await;
            for f in &forks {
                let _ = sbx.delete(f.clone()).await;
            }
            let _ = sbx.delete_checkpoint(ck.clone()).await;
            let _ = sbx.delete(src.clone()).await;
            r
        },
    )
    .await;
}
