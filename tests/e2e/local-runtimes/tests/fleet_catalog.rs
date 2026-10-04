//! Every Fleet catalog image, booted locally through the SDK
//! (`CUA_E2E_FLEET_CATALOG=1`).
//!
//! For each catalog entry the test inspects every reference in the
//! registry (anonymous for public ones; private ECR through the docker
//! config, e.g. after `aws ecr get-login-password | docker login`), makes one
//! run per distinct (kind, architecture) it offers plus the `docker-latest`
//! rootfs variant when the repository publishes one, and for each run:
//!
//! 1. creates a local sandbox through `cua_sandbox_core::Sandboxes` with the
//!    **bare** Fleet ref (the SDK picks the backend and architecture),
//! 2. waits for the catalog's declared service ports (TCP; QEMU checks the
//!    guest side of each forward),
//! 3. checks every declared service port answers: TCP that stays open, plus
//!    the `GET /path` from the catalog's readiness text when there is one
//!    (any HTTP status below 500). Daemon-agnostic: nothing speaks a
//!    service protocol,
//! 4. deletes the sandbox and checks it is gone.
//!
//! Runs are sequential (one VM at a time, 4 vCPU / 4 GiB by default). A JSON report is
//! written to `CUA_E2E_FLEET_CATALOG_REPORT` (default
//! `target/fleet-catalog-report.json`) and a table is printed.
//!
//! | env | default | meaning |
//! |---|---|---|
//! | `CUA_FLEET_CATALOG` | `fleet-catalog.public.json` next to this crate | catalog path (the full `cloud/scripts/fleet-images/catalog.json` works) |
//! | `CUA_E2E_FLEET_CATALOG_ONLY` | all | comma list of `id`, `id@arm64`, `id@amd64`, `id@rootfs` |
//! | `CUA_E2E_FLEET_CATALOG_MAX_GB` | 20 | skip images whose compressed layers exceed this |
//! | `CUA_E2E_FLEET_TCG_TIMEOUT_SECS` | 2400 | readiness budget under emulation (TCG) |
//! | `CUA_E2E_FLEET_READY_TIMEOUT_SECS` | 900 | readiness budget with hardware acceleration |
//! | `CUA_E2E_FLEET_WINDOWS` | `auto` | `full` (boot + services), `evidence` (hard-capped boot with screenshots), `skip`; `auto` = full with KVM, evidence otherwise |
//! | `CUA_E2E_FLEET_WINDOWS_CAP_SECS` | 900 | evidence-mode cap |
//! | `CUA_E2E_FLEET_CATALOG_PRUNE` | 0 | delete each pulled disk after its run |
//! | `CUA_E2E_FLEET_CPUS` / `CUA_E2E_FLEET_MEMORY_MB` | 4 / 4096 | guest size (keep ≤ 4096 on shared dev machines) |
//! | `CUA_HOME` | `~/.cua` | image cache and VM state root |

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_daemon::local::VmmLocal;
use cua_e2e_local_runtimes::{gated, init_tracing, run_id};
use cua_image::RegistryClient;
use cua_image::detect::{ImageKind, inspect};
use cua_sandbox_core::{CreateOptions, PortTarget, Probe, ProviderKind, Sandboxes};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Clone, Debug)]
struct Service {
    name: String,
    port: u16,
    http_path: Option<String>,
}

#[derive(Clone, Debug)]
struct Variant {
    id: String,
    label: String,
    reference: String,
    kind: ImageKind,
    arch: String,
    windows: bool,
    services: Vec<Service>,
}

fn env_or<T: std::str::FromStr>(k: &str, d: T) -> T {
    std::env::var(k)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(d)
}

/// `GET /status` → `/status` from the catalog's prose readiness.
fn http_path(readiness: &str) -> Option<String> {
    let i = readiness.find("GET /")?;
    let rest = &readiness[i + 4..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == ';' || c == ',')
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn catalog_path() -> PathBuf {
    std::env::var("CUA_FLEET_CATALOG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fleet-catalog.public.json")
        })
}

fn kvm() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok()
}

fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    }
}

/// Compressed layer bytes for `reference` on `linux/<arch>`.
async fn pull_size(client: &RegistryClient, reference: &str, arch: &str) -> Option<u64> {
    let (_, m, _) = client.resolve_platform(reference, arch).await.ok()?;
    Some(m.layers.iter().map(|l| l.size).sum())
}

/// Connect, then require the peer to hold the connection open or send data
/// (forwarders accept and close at once when nothing listens).
async fn tcp_alive(addr: &str) -> bool {
    let Ok(Ok(mut s)) =
        tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(addr)).await
    else {
        return false;
    };
    let mut b = [0u8; 64];
    match tokio::time::timeout(Duration::from_millis(800), s.read(&mut b)).await {
        Err(_) => true,
        Ok(Ok(n)) => n > 0,
        Ok(Err(_)) => false,
    }
}

/// Minimal HTTP/1.1 GET; the status code, if any response arrived.
async fn http_status(addr: &str, path: &str) -> Option<u16> {
    let fut = async {
        let mut s = TcpStream::connect(addr).await.ok()?;
        let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes()).await.ok()?;
        let mut buf = vec![0u8; 512];
        let n = s.read(&mut buf).await.ok()?;
        let line = std::str::from_utf8(&buf[..n])
            .ok()?
            .lines()
            .next()?
            .to_string();
        let mut it = line.split_whitespace();
        it.next().filter(|h| h.starts_with("HTTP/"))?;
        it.next()?.parse().ok()
    };
    tokio::time::timeout(Duration::from_secs(15), fut)
        .await
        .ok()
        .flatten()
}

fn cua_home() -> PathBuf {
    cua_vmm::host::cua_home()
}

fn qemu_state(name: &str) -> Option<Value> {
    let p = cua_home().join("vmm/qemu").join(name).join("state.json");
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

fn tail(path: &std::path::Path, lines: usize) -> String {
    let s = std::fs::read_to_string(path).unwrap_or_default();
    let v: Vec<&str> = s.lines().collect();
    v[v.len().saturating_sub(lines)..].join("\n")
}

async fn build_variants(client: &RegistryClient, catalog: &Value) -> (Vec<Variant>, Vec<Value>) {
    let only: Vec<String> = std::env::var("CUA_E2E_FLEET_CATALOG_ONLY")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for e in catalog["images"].as_array().into_iter().flatten() {
        let id = e["id"].as_str().unwrap_or("?").to_string();
        let windows = e["guest_os"]
            .as_str()
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains("windows");
        let services: Vec<Service> = e["services"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| {
                Some(Service {
                    name: s["name"].as_str()?.to_string(),
                    port: u16::try_from(s["port"].as_u64()?).ok()?,
                    http_path: s["readiness"].as_str().and_then(http_path),
                })
            })
            .collect();
        let mut refs: Vec<String> = e["references"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r.as_str().map(str::to_string))
            .collect();
        // Fleet's gVisor rootfs sibling (`docker-*` tags), when published.
        if let Some(repo) = e["repository"].as_str() {
            refs.push(format!("{repo}:docker-latest"));
        }
        let mut seen: Vec<(ImageKind, String)> = Vec::new();
        for r in refs {
            let rootfs_probe = r.ends_with(":docker-latest");
            let i = match inspect(client, &r, host_arch()).await {
                Ok(i) => i,
                Err(err) => {
                    if !rootfs_probe {
                        skipped.push(json!({"id": id, "reference": r, "skipped": format!("not pullable with this account: {err}")}));
                    }
                    continue;
                }
            };
            let arch = i
                .run_arch(host_arch())
                .unwrap_or_else(|| host_arch().to_string());
            if seen.contains(&(i.kind, arch.clone())) {
                continue;
            }
            seen.push((i.kind, arch.clone()));
            let label = match i.kind {
                ImageKind::Rootfs => format!("{id}@rootfs"),
                _ => format!("{id}@{arch}"),
            };
            if !only.is_empty() && !only.iter().any(|o| *o == id || *o == label) {
                continue;
            }
            out.push(Variant {
                id: id.clone(),
                label,
                reference: r,
                kind: i.kind,
                arch,
                windows,
                services: services.clone(),
            });
        }
    }
    (out, skipped)
}

async fn run_variant(v: &Variant, local: Arc<VmmLocal>, state: &std::path::Path) -> Value {
    let client = RegistryClient::default();
    let max_gb: f64 = env_or("CUA_E2E_FLEET_CATALOG_MAX_GB", 20.0);
    let size = pull_size(&client, &v.reference, &v.arch).await;
    let mut row = json!({
        "id": v.id, "variant": v.label, "reference": v.reference, "kind": format!("{:?}", v.kind),
        "arch": v.arch, "pull_gb": size.map(|s| (s as f64 / 1e9 * 100.0).round() / 100.0),
    });
    if size.is_some_and(|s| s as f64 / 1e9 > max_gb) {
        row["result"] = json!("skipped");
        row["note"] = json!(format!("compressed image > {max_gb} GB"));
        return row;
    }
    let tcg = v.kind == ImageKind::ContainerDisk && v.arch != host_arch()
        || (v.kind == ImageKind::ContainerDisk && !kvm() && !cfg!(target_os = "macos"));
    let windows_mode = match std::env::var("CUA_E2E_FLEET_WINDOWS").as_deref() {
        Ok("full") => "full",
        Ok("evidence") => "evidence",
        Ok("skip") => "skip",
        _ if tcg => "evidence",
        _ => "full",
    };
    if v.windows && windows_mode == "skip" {
        row["result"] = json!("skipped");
        row["note"] = json!("CUA_E2E_FLEET_WINDOWS=skip");
        return row;
    }
    let evidence = v.windows && windows_mode == "evidence";
    let budget = if evidence {
        Duration::from_secs(env_or("CUA_E2E_FLEET_WINDOWS_CAP_SECS", 900))
    } else if tcg {
        Duration::from_secs(env_or("CUA_E2E_FLEET_TCG_TIMEOUT_SECS", 2400))
    } else {
        Duration::from_secs(env_or("CUA_E2E_FLEET_READY_TIMEOUT_SECS", 900))
    };
    let short: String =
        v.id.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(10)
            .collect();
    let name = format!("cua-e2e-fc-{short}-{}", run_id());
    row["sandbox"] = json!(name);
    let sbx = Sandboxes::builder()
        .local(local.clone())
        .state_dir(state)
        .build();
    let mut o = CreateOptions::new(ProviderKind::Local, &v.reference).name(&name);
    o.cpus = env_or("CUA_E2E_FLEET_CPUS", 4);
    o.memory_mb = env_or("CUA_E2E_FLEET_MEMORY_MB", 4096);
    for s in &v.services {
        o = o.service(&s.name, s.port).wait_for(Probe::Tcp(s.port));
    }
    o.ready_timeout = budget;

    // Evidence: periodic screenshots of VM guests while they boot.
    let shots_dir = cua_home().join("e2e-fleet-catalog");
    let _ = std::fs::create_dir_all(&shots_dir);
    let shooter = (v.kind == ImageKind::ContainerDisk).then(|| {
        let (local, name, dir) = (local.clone(), name.clone(), shots_dir.clone());
        tokio::spawn(async move {
            let t0 = Instant::now();
            let mut shots = Vec::new();
            // Bounded: at most 60 shots.
            for i in 0..60u32 {
                tokio::time::sleep(Duration::from_secs(if i == 0 { 20 } else { 90 })).await;
                let png = dir.join(format!("{name}-{:04}s.png", t0.elapsed().as_secs()));
                if let Ok(mut q) = local.qemu().qmp(&name).await
                    && q.screendump(&png).await.is_ok()
                {
                    shots.push(png.display().to_string());
                }
            }
            shots
        })
    });

    let t0 = Instant::now();
    let created = sbx.create(o).await;
    let boot = t0.elapsed().as_secs_f64();
    row["boot_secs"] = json!((boot * 10.0).round() / 10.0);
    if let Some(st) = qemu_state(&name) {
        row["backend"] = json!("qemu");
        row["accel"] = st["accel"].clone();
        row["firmware"] = st["firmware"]["type"].clone();
        row["guest_os"] = st["os"].clone();
    }
    // Nothing declared to wait for (e.g. a registry-discovered Windows image
    // with no services): keep the VM up for the evidence cap so the
    // screenshots show how far it boots.
    if evidence && v.services.is_empty() && created.is_ok() {
        tokio::time::sleep(budget).await;
        row["note"] = json!(format!(
            "no declared services; observed {}s of boot",
            budget.as_secs()
        ));
    }
    match &created {
        Ok(sb) => {
            row["backend"] = json!(sb.runtime_type());
            let mut checks = Vec::new();
            let mut all_ok = true;
            for s in &v.services {
                let addr = match sb.port(s.port) {
                    Ok(PortTarget::Addr { host, port }) => format!("{host}:{port}"),
                    other => {
                        all_ok = false;
                        checks.push(json!({"service": s.name, "port": s.port, "ok": false, "error": format!("{other:?}")}));
                        continue;
                    }
                };
                let tcp = tcp_alive(&addr).await;
                let http = match &s.http_path {
                    Some(p) => http_status(&addr, p).await,
                    None => None,
                };
                let ok = tcp
                    && s.http_path
                        .as_ref()
                        .is_none_or(|_| http.is_some_and(|c| c < 500));
                all_ok &= ok;
                checks.push(json!({"service": s.name, "port": s.port, "tcp": tcp, "http_path": s.http_path, "http_status": http, "ok": ok}));
            }
            row["services"] = json!(checks);
            row["result"] = json!(if v.services.is_empty() && evidence {
                "evidence"
            } else if all_ok {
                "pass"
            } else {
                "fail"
            });
        }
        Err(e) => {
            row["error"] = json!(e.to_string());
            row["result"] = json!(if evidence { "evidence" } else { "fail" });
        }
    }
    if v.kind == ImageKind::ContainerDisk {
        // Final screenshot + serial tail for every VM run.
        let png = shots_dir.join(format!("{name}-final.png"));
        if let Ok(mut q) = local.qemu().qmp(&name).await {
            if q.screendump(&png).await.is_ok() {
                row["screenshot"] = json!(png.display().to_string());
            }
            row["qemu_status"] = json!(q.status().await.unwrap_or_default());
            if row["result"] != "pass" {
                // Is the guest on the network at all? slirp lists every
                // guest-initiated flow (DHCP/DNS/TCP) and every forward.
                row["usernet"] = json!(q.hmp("info usernet").await.unwrap_or_default());
            }
        }
        row["serial_tail"] = json!(tail(
            &cua_home().join("vmm/qemu").join(&name).join("serial.log"),
            12
        ));
    }
    if let Some(h) = shooter {
        h.abort();
    }
    // Teardown, always; then prove it is gone.
    use cua_sandbox_core::LocalRuntime;
    let del = match created {
        Ok(sb) => sb.delete().await.map_err(|e| e.to_string()),
        // Nothing may have been created (e.g. a pull failed): NotFound is fine.
        Err(_) => match local.delete(&name).await {
            Err(cua_sandbox_core::RuntimeError::NotFound(_)) => Ok(()),
            other => other.map_err(|e| e.to_string()),
        },
    };
    let gone = local.status(&name).await.is_err();
    row["deleted"] = json!(del.is_ok() && gone);
    if let Err(e) = del {
        row["delete_error"] = json!(e);
    }
    if std::env::var("CUA_E2E_FLEET_CATALOG_PRUNE").as_deref() == Ok("1")
        && v.kind == ImageKind::ContainerDisk
    {
        if let Ok((_, _, digest)) = client.resolve_platform(&v.reference, &v.arch).await {
            let dir = cua_home()
                .join("images/disks")
                .join(digest.trim_start_matches("sha256:"));
            let _ = std::fs::remove_dir_all(dir);
        }
    }
    row
}

fn table(rows: &[Value]) -> String {
    let mut s = String::from(
        "| variant | backend | accel | firmware | arch | pull GB | boot s | services | result |\n|---|---|---|---|---|---|---|---|---|\n",
    );
    for r in rows {
        let svc = r["services"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| {
                        format!(
                            "{}:{} {}{}",
                            c["service"].as_str().unwrap_or(""),
                            c["port"],
                            if c["ok"] == true { "ok" } else { "FAIL" },
                            c["http_status"]
                                .as_u64()
                                .map(|h| format!(" ({h})"))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let f = |k: &str| match &r[k] {
            Value::Null => "-".to_string(),
            Value::String(s) => s.clone(),
            v => v.to_string(),
        };
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {}{} |\n",
            f("variant"),
            f("backend"),
            f("accel"),
            f("firmware"),
            f("arch"),
            f("pull_gb"),
            f("boot_secs"),
            svc,
            f("result"),
            r["note"]
                .as_str()
                .map(|n| format!(" ({n})"))
                .unwrap_or_default()
        ));
    }
    s
}

#[tokio::test]
async fn fleet_catalog_images_run_locally() {
    if !gated("CUA_E2E_FLEET_CATALOG") {
        return;
    }
    init_tracing();
    let path = catalog_path();
    let catalog: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("catalog")).expect("catalog JSON");
    eprintln!(
        "catalog: {} ({} entries)",
        path.display(),
        catalog["images"].as_array().map_or(0, Vec::len)
    );
    let client = RegistryClient::default();
    let (variants, mut rows) = build_variants(&client, &catalog).await;
    eprintln!(
        "variants: {:?}",
        variants
            .iter()
            .map(|v| (&v.label, &v.reference))
            .collect::<Vec<_>>()
    );
    let local = Arc::new(VmmLocal::default());
    let state = tempfile_dir();
    for v in &variants {
        eprintln!("=== {} ({}) ===", v.label, v.reference);
        let row = run_variant(v, local.clone(), &state).await;
        eprintln!("{}", serde_json::to_string_pretty(&row).unwrap());
        rows.push(row);
    }
    let _ = std::fs::remove_dir_all(&state);
    let report = std::env::var("CUA_E2E_FLEET_CATALOG_REPORT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fleet-catalog-report.json")
        });
    if let Some(p) = report.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    std::fs::write(&report, serde_json::to_vec_pretty(&json!({"host": {"os": std::env::consts::OS, "arch": host_arch(), "kvm": kvm()}, "runs": rows})).unwrap()).unwrap();
    let t = table(&rows);
    eprintln!("\n{t}\nreport: {}", report.display());
    if let Ok(summary) = std::env::var("GITHUB_STEP_SUMMARY") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(summary) {
            let _ = writeln!(f, "## Fleet catalog images, local\n\n{t}");
        }
    }
    let failed: Vec<&str> = rows
        .iter()
        .filter(|r| r["result"] == "fail" || r["deleted"] == false)
        .filter_map(|r| r["variant"].as_str())
        .collect();
    assert!(!variants.is_empty(), "no catalog image was pullable");
    assert!(failed.is_empty(), "failed: {failed:?}");
}

fn tempfile_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("cua-e2e-fleet-catalog-{}", run_id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn readiness_text_yields_http_paths() {
    assert_eq!(
        http_path("GET /status; default SDK also probes TCP 8000").as_deref(),
        Some("/status")
    );
    assert_eq!(
        http_path("GET /healthz then MCP initialize").as_deref(),
        Some("/healthz")
    );
    assert_eq!(
        http_path("MCP initialize and an interactive guest action"),
        None
    );
    let _ = BTreeMap::<u8, u8>::new();
}
