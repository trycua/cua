//! Collectors: how an [`Inventory`] is taken. A [`Source`] is one kind of
//! thing to list (processes, listening ports, containers, files under a
//! directory, records of a JSON file, the output of a command, cloud
//! resources by tag). A [`Sources`] list is plain JSON, so a Spaces client
//! describes its own state files without touching this crate.

use crate::inventory::{Inventory, Item};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One kind of thing to list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Source {
    /// Processes whose command line contains one of the patterns.
    Processes {
        /// `(substring of the command line, kind)`.
        patterns: Vec<(String, String)>,
        /// Only processes that mention this home (command line, or
        /// `CUA_HOME` in their environment where readable).
        #[serde(default)]
        home: Option<String>,
    },
    /// TCP listeners owned by a matched process (needs `Processes`' view).
    Ports {
        patterns: Vec<String>,
        #[serde(default)]
        home: Option<String>,
    },
    /// Docker containers (all states) and volumes. With `home`, only those
    /// that cua home created (the `ai.cua.home` label: 12 hex of the
    /// SHA-256 of the path); each container belongs to the sandbox named
    /// by its `cua.sandbox.name` label.
    Docker {
        #[serde(default)]
        home: Option<String>,
    },
    /// The sandboxes recorded under `<home>/sandboxes/*.json` (kind
    /// `space`, id = the sandbox name): what Cua believes exists.
    Spaces { home: String },
    /// Linux kernel objects a Space can leave: mounts under `home`, network
    /// namespaces, tap/veth devices, loop devices backed under `home`.
    Kernel { home: String },
    /// Entries `depth` levels below `dir` (files and directories), as
    /// `file:<kind>` with the path relative to `dir`.
    Dir {
        kind: String,
        dir: String,
        #[serde(default = "one")]
        depth: u32,
        #[serde(default)]
        exclude: Vec<String>,
    },
    /// Records of a JSON file: the array (or object) at `pointer`, each
    /// element identified by `id_field` (an object's keys when absent).
    Json {
        kind: String,
        file: String,
        #[serde(default)]
        pointer: String,
        #[serde(default)]
        id_field: Option<String>,
        #[serde(default)]
        owner_field: Option<String>,
    },
    /// A command that prints JSON (an array of objects, or an object), run
    /// like `Json`: `cua sandbox list --json`, an admin API call.
    Command {
        kind: String,
        argv: Vec<String>,
        #[serde(default)]
        env: Vec<(String, String)>,
        #[serde(default)]
        pointer: String,
        #[serde(default)]
        id_field: Option<String>,
        #[serde(default)]
        owner_field: Option<String>,
    },
    /// EC2 resources carrying a tag (instances not terminated, volumes,
    /// addresses, security groups, snapshots, network interfaces).
    Aws {
        profile: String,
        region: String,
        tag_key: String,
        tag_value: String,
    },
    /// Hetzner Cloud resources with a label (token from the file).
    Hetzner {
        token_file: String,
        label_selector: String,
    },
}

fn one() -> u32 {
    1
}

/// A list of sources.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Sources {
    pub sources: Vec<Source>,
}

impl Sources {
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// The default host view of an isolated `CUA_HOME` (a daemon, its
    /// Spaces and the gateway): VM, container, helper processes, listening
    /// ports, kernel objects, per-Space directories, the sandboxes Cua
    /// recorded and the gateway's records.
    pub fn for_cua_home(home: &Path, gateway_dir: Option<&Path>) -> Self {
        let h = home.to_string_lossy().to_string();
        let pats: Vec<(String, String)> = [
            ("qemu-system", "process:qemu"),
            ("cua-spacesd", "process:spacesd"),
            ("runsc", "process:runsc"),
            ("firecracker", "process:firecracker"),
            ("cloud-hypervisor", "process:chv"),
            ("containerd-shim", "process:shim"),
            ("Virtualization.VirtualMachine", "process:vz"),
            ("socat", "process:socat"),
            ("cua daemon", "process:daemon"),
            ("cua compat serve", "process:gateway"),
            ("cua-spaces-cli", "process:cli"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let mut s = vec![
            Source::Processes {
                patterns: pats.clone(),
                home: Some(h.clone()),
            },
            Source::Ports {
                patterns: pats.iter().map(|p| p.0.clone()).collect(),
                home: Some(h.clone()),
            },
            Source::Docker {
                home: Some(h.clone()),
            },
            Source::Spaces { home: h.clone() },
            Source::Kernel { home: h.clone() },
        ];
        // Per-Space data under the home. Static things (images, caches,
        // logs, the binaries) are not Space state.
        for d in [
            "sandboxes",
            "vmm",
            "checkpoints",
            "runtimes",
            "tmp",
            "spaces",
            "overlays",
            "disks",
        ] {
            s.push(Source::Dir {
                kind: d.into(),
                dir: format!("{h}/{d}"),
                depth: 2,
                exclude: vec![
                    "*.log".into(),
                    "cache".into(),
                    "bases".into(),
                    "golden".into(),
                ],
            });
        }
        if let Some(g) = gateway_dir {
            let g = g.to_string_lossy();
            s.push(Source::Json {
                kind: "record:gateway".into(),
                file: format!("{g}/state.json"),
                pointer: "/records".into(),
                id_field: Some("id".into()),
                owner_field: Some("cua_name".into()),
            });
        }
        Sources { sources: s }
    }
}

fn run(argv: &[String], env: &[(String, String)]) -> Result<String, String> {
    let (first, rest) = argv.split_first().ok_or("empty command")?;
    let mut c = Command::new(first);
    c.args(rest);
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().map_err(|e| format!("{first}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{} failed: {}",
            argv.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The `ai.cua.home` label value of a cua home: 12 hex of the SHA-256 of
/// its path (cua-vmm `home_tag_for`).
fn home_tag(home: &str) -> Result<String, String> {
    let out = Command::new("sh")
        .arg("-c")
        .arg("printf %s \"$1\" | (sha256sum 2>/dev/null || shasum -a 256)")
        .arg("sh")
        .arg(home)
        .output()
        .map_err(|e| e.to_string())?;
    let t = String::from_utf8_lossy(&out.stdout);
    t.get(..12)
        .map(str::to_string)
        .ok_or_else(|| "sha256 of the home failed".into())
}

fn sh(cmd: &str) -> Result<String, String> {
    run(&["sh".into(), "-c".into(), cmd.into()], &[])
}

fn records(
    kind: &str,
    text: &str,
    pointer: &str,
    id_field: &Option<String>,
    owner_field: &Option<String>,
) -> Result<Vec<Item>, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("{kind}: not JSON: {e}"))?;
    let at = if pointer.is_empty() {
        Some(&v)
    } else {
        v.pointer(pointer)
    };
    let Some(at) = at else {
        return Ok(vec![]);
    };
    let text_of = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        o => Some(o.to_string()),
    };
    let mut out = Vec::new();
    match at {
        Value::Array(xs) => {
            for x in xs {
                let id = match (id_field, x) {
                    (Some(f), Value::Object(_)) => x.get(f).and_then(text_of),
                    (None, Value::String(s)) => Some(s.clone()),
                    _ => None,
                };
                let Some(id) = id else { continue };
                let mut it = Item::new(kind, &id);
                if let Some(of) = owner_field {
                    it.owner = x.get(of).and_then(text_of);
                }
                out.push(it);
            }
        }
        Value::Object(m) => {
            for (k, x) in m {
                let mut it = Item::new(
                    kind,
                    id_field
                        .as_ref()
                        .and_then(|f| x.get(f))
                        .and_then(text_of)
                        .as_deref()
                        .unwrap_or(k),
                );
                if let Some(of) = owner_field {
                    it.owner = x.get(of).and_then(text_of);
                }
                out.push(it);
            }
        }
        _ => {}
    }
    Ok(out)
}

fn glob_match(pat: &str, name: &str) -> bool {
    match pat.split_once('*') {
        None => pat == name,
        Some((a, b)) => name.starts_with(a) && name.ends_with(b) && name.len() >= a.len() + b.len(),
    }
}

fn walk(dir: &Path, depth: u32, exclude: &[String], base: &Path, out: &mut Vec<(PathBuf, u64)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if exclude.iter().any(|p| glob_match(p, &name)) {
            continue;
        }
        let p = e.path();
        let md = e.metadata().ok();
        if depth <= 1 {
            out.push((
                p.strip_prefix(base).unwrap_or(&p).to_path_buf(),
                md.map_or(0, |m| m.len()),
            ));
        } else if md.as_ref().is_some_and(|m| m.is_dir()) {
            walk(&p, depth - 1, exclude, base, out);
        } else {
            out.push((
                p.strip_prefix(base).unwrap_or(&p).to_path_buf(),
                md.map_or(0, |m| m.len()),
            ));
        }
    }
}

/// `(pid, ppid, command line)` of every process.
pub fn process_table() -> Result<Vec<(u32, u32, String)>, String> {
    let text = run(
        &[
            "ps".into(),
            "-axww".into(),
            "-o".into(),
            "pid=,ppid=,args=".into(),
        ],
        &[],
    )?;
    let mut out = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(a), Some(b)) = (it.next(), it.next()) else {
            continue;
        };
        let (Ok(pid), Ok(ppid)) = (a.parse(), b.parse()) else {
            continue;
        };
        let args = it.collect::<Vec<_>>().join(" ");
        out.push((pid, ppid, args));
    }
    Ok(out)
}

fn mentions_home(pid: u32, args: &str, home: &str) -> bool {
    if args.contains(home) {
        return true;
    }
    std::fs::read(format!("/proc/{pid}/environ"))
        .map(|b| {
            b.split(|c| *c == 0)
                .any(|kv| kv == format!("CUA_HOME={home}").as_bytes())
        })
        .unwrap_or(false)
}

fn matched(
    patterns: &[(String, String)],
    home: &Option<String>,
) -> Result<Vec<(u32, String, String)>, String> {
    let me = std::process::id();
    let mut out = Vec::new();
    for (pid, _ppid, args) in process_table()? {
        if pid == me || args.contains("cua-chaos") {
            continue;
        }
        let Some((_, kind)) = patterns.iter().find(|(p, _)| args.contains(p.as_str())) else {
            continue;
        };
        if let Some(h) = home
            && !mentions_home(pid, &args, h)
        {
            continue;
        }
        out.push((pid, kind.clone(), args));
    }
    Ok(out)
}

fn collect_one(src: &Source, inv: &mut Inventory) -> Result<(), String> {
    match src {
        Source::Processes { patterns, home } => {
            for (pid, kind, args) in matched(patterns, home)? {
                // The identity is the command line; the pid is detail, so a
                // restarted daemon is the same item.
                let mut it = Item::new(&kind, &args).detail(format!("pid {pid}"));
                // A QEMU VM is named after its sandbox (`-name <sandbox>`).
                if kind == "process:qemu"
                    && !args.contains(" -name cua-pool-")
                    && let Some(n) = args
                        .split(" -name ")
                        .nth(1)
                        .and_then(|r| r.split([' ', ',']).next())
                {
                    it.owner = Some(n.to_string());
                }
                inv.add(it);
            }
        }
        Source::Ports { patterns, home } => {
            let pats: Vec<(String, String)> =
                patterns.iter().map(|p| (p.clone(), "x".into())).collect();
            // pid -> the pattern (process kind) that matched it.
            let by_pid: std::collections::BTreeMap<u32, String> = matched(&pats, home)?
                .into_iter()
                .map(|(pid, _, args)| {
                    let kind = patterns
                        .iter()
                        .find(|p| args.contains(p.as_str()))
                        .cloned()
                        .unwrap_or_default();
                    (pid, kind)
                })
                .collect();
            let text = run(&["ss".into(), "-Htlnp".into()], &[])?;
            // A restarted process listens on a new port: identify a port by
            // its process kind and ordinal, not its number, so a restart is
            // the same item and only an extra listener is new.
            let mut per_kind: std::collections::BTreeMap<String, Vec<String>> = Default::default();
            for line in text.lines() {
                let cols: Vec<&str> = line.split_whitespace().collect();
                let Some(local) = cols.get(3) else { continue };
                let pid = line
                    .split("pid=")
                    .nth(1)
                    .and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next())
                    .and_then(|p| p.parse::<u32>().ok());
                if let Some(kind) = pid.and_then(|p| by_pid.get(&p)) {
                    per_kind
                        .entry(kind.clone())
                        .or_default()
                        .push(format!("{local} pid {}", pid.unwrap_or(0)));
                }
            }
            for (kind, mut ports) in per_kind {
                ports.sort();
                for (i, d) in ports.iter().enumerate() {
                    inv.add(Item::new("port", &format!("{kind}#{i}")).detail(d.clone()));
                }
            }
        }
        Source::Docker { home } => {
            let mut ps = vec![
                "docker".to_string(),
                "ps".into(),
                "-a".into(),
                "--format".into(),
                "{{.Names}}\t{{.Label \"cua.sandbox.name\"}}\t{{.Image}}\t{{.Status}}".into(),
            ];
            let mut vl = vec![
                "docker".to_string(),
                "volume".into(),
                "ls".into(),
                "-q".into(),
            ];
            if let Some(h) = home {
                let tag = home_tag(h)?;
                for v in [&mut ps, &mut vl] {
                    v.push("--filter".into());
                    v.push(format!("label=ai.cua.home={tag}"));
                }
            }
            for l in run(&ps, &[])?.lines() {
                let c: Vec<&str> = l.split('\t').collect();
                let Some(n) = c.first().filter(|n| !n.is_empty()) else {
                    continue;
                };
                let mut it = Item::new("container", n).detail(l.replace('\t', " "));
                if let Some(o) = c.get(1).filter(|o| !o.is_empty()) {
                    it.owner = Some((*o).to_string());
                }
                inv.add(it);
            }
            for v in run(&vl, &[])?.lines().filter(|v| !v.is_empty()) {
                inv.add(Item::new("docker:volume", v));
            }
        }
        Source::Spaces { home } => {
            if let Ok(rd) = std::fs::read_dir(Path::new(home).join("sandboxes")) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().to_string();
                    if let Some(stem) = n.strip_suffix(".json") {
                        inv.add(Item::new("space", stem));
                    }
                }
            }
        }
        Source::Kernel { home } => {
            if let Ok(m) = std::fs::read_to_string("/proc/mounts") {
                for l in m.lines().filter(|l| l.contains(home.as_str())) {
                    let mp = l.split_whitespace().nth(1).unwrap_or(l);
                    inv.add(Item::new("mount", mp));
                }
            }
            if let Ok(t) = sh("ip netns list 2>/dev/null; true") {
                for l in t.lines().filter(|l| !l.is_empty()) {
                    let n = l.split_whitespace().next().unwrap_or(l);
                    inv.add(Item::new("netns", n));
                }
            }
            if let Ok(t) = sh("ip -o link show 2>/dev/null; true") {
                for l in t.lines() {
                    let name = l
                        .split(": ")
                        .nth(1)
                        .map(|n| n.split('@').next().unwrap_or(n).to_string())
                        .unwrap_or_default();
                    if ["tap", "vnet", "cua"].iter().any(|p| name.starts_with(p)) {
                        inv.add(Item::new("netdev", &name));
                    }
                }
            }
            if let Ok(t) = sh("losetup -a 2>/dev/null; true") {
                for l in t.lines().filter(|l| l.contains(home.as_str())) {
                    inv.add(Item::new("loop", l.split(':').next().unwrap_or(l)));
                }
            }
        }
        Source::Dir {
            kind,
            dir,
            depth,
            exclude,
        } => {
            let base = PathBuf::from(dir);
            let mut found = Vec::new();
            walk(&base, *depth, exclude, &base, &mut found);
            for (p, size) in found {
                inv.add(
                    Item::new(&format!("file:{kind}"), &p.to_string_lossy())
                        .detail(format!("{size} bytes")),
                );
            }
        }
        Source::Json {
            kind,
            file,
            pointer,
            id_field,
            owner_field,
        } => match std::fs::read_to_string(file) {
            Ok(t) if !t.trim().is_empty() => {
                for i in records(kind, &t, pointer, id_field, owner_field)? {
                    inv.add(i);
                }
            }
            // A missing state file is an empty record set.
            _ => {}
        },
        Source::Command {
            kind,
            argv,
            env,
            pointer,
            id_field,
            owner_field,
        } => {
            let t = run(argv, env)?;
            if !t.trim().is_empty() {
                for i in records(kind, &t, pointer, id_field, owner_field)? {
                    inv.add(i);
                }
            }
        }
        Source::Aws {
            profile,
            region,
            tag_key,
            tag_value,
        } => {
            let q = |svc: &str, filter_name: &str, jq: &[&str]| -> Result<Value, String> {
                let mut argv: Vec<String> = [
                    "aws",
                    "ec2",
                    svc,
                    "--profile",
                    profile,
                    "--region",
                    region,
                    "--output",
                    "json",
                    "--filters",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect();
                argv.push(format!("Name={filter_name},Values={tag_value}"));
                let _ = jq;
                serde_json::from_str(&run(&argv, &[])?).map_err(|e| e.to_string())
            };
            let tagf = format!("tag:{tag_key}");
            let name_of = |v: &Value| {
                v.get("Tags")
                    .and_then(Value::as_array)
                    .and_then(|ts| {
                        ts.iter()
                            .find(|t| t["Key"] == "Name")
                            .and_then(|t| t["Value"].as_str())
                    })
                    .unwrap_or("")
                    .to_string()
            };
            let v = q("describe-instances", &tagf, &[])?;
            for r in v["Reservations"].as_array().into_iter().flatten() {
                for i in r["Instances"].as_array().into_iter().flatten() {
                    let state = i["State"]["Name"].as_str().unwrap_or("");
                    if state == "terminated" {
                        continue;
                    }
                    inv.add(
                        Item::new("aws:instance", i["InstanceId"].as_str().unwrap_or(""))
                            .detail(format!("{state} {}", name_of(i))),
                    );
                }
            }
            for (svc, key, field, kind) in [
                ("describe-volumes", "Volumes", "VolumeId", "aws:volume"),
                (
                    "describe-addresses",
                    "Addresses",
                    "AllocationId",
                    "aws:address",
                ),
                (
                    "describe-security-groups",
                    "SecurityGroups",
                    "GroupId",
                    "aws:security-group",
                ),
                (
                    "describe-network-interfaces",
                    "NetworkInterfaces",
                    "NetworkInterfaceId",
                    "aws:eni",
                ),
            ] {
                let v = q(svc, &tagf, &[])?;
                for x in v[key].as_array().into_iter().flatten() {
                    inv.add(Item::new(kind, x[field].as_str().unwrap_or("")).detail(name_of(x)));
                }
            }
            let argv: Vec<String> = [
                "aws",
                "ec2",
                "describe-snapshots",
                "--profile",
                profile,
                "--region",
                region,
                "--owner-ids",
                "self",
                "--output",
                "json",
                "--filters",
            ]
            .iter()
            .map(|s| s.to_string())
            .chain([format!("Name={tagf},Values={tag_value}")])
            .collect();
            let v: Value = serde_json::from_str(&run(&argv, &[])?).map_err(|e| e.to_string())?;
            for x in v["Snapshots"].as_array().into_iter().flatten() {
                inv.add(Item::new(
                    "aws:snapshot",
                    x["SnapshotId"].as_str().unwrap_or(""),
                ));
            }
        }
        Source::Hetzner {
            token_file,
            label_selector,
        } => {
            let token = std::fs::read_to_string(token_file)
                .map_err(|e| format!("{token_file}: {e}"))?
                .trim()
                .to_string();
            for (path, key, kind) in [
                ("servers", "servers", "hetzner:server"),
                ("volumes", "volumes", "hetzner:volume"),
                ("floating_ips", "floating_ips", "hetzner:floating-ip"),
                ("firewalls", "firewalls", "hetzner:firewall"),
                ("ssh_keys", "ssh_keys", "hetzner:ssh-key"),
                ("networks", "networks", "hetzner:network"),
                ("primary_ips", "primary_ips", "hetzner:primary-ip"),
            ] {
                let url = format!(
                    "https://api.hetzner.cloud/v1/{path}?label_selector={}&per_page=50",
                    label_selector.replace('=', "%3D")
                );
                // The token goes through curl's stdin config, never argv.
                let mut child = Command::new("curl")
                    .args(["-sS", "--max-time", "30", "-K", "-", &url])
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .spawn()
                    .map_err(|e| format!("curl: {e}"))?;
                {
                    use std::io::Write;
                    let mut si = child.stdin.take().ok_or("curl stdin")?;
                    let _ = writeln!(si, "header = \"Authorization: Bearer {token}\"");
                }
                let out = child.wait_with_output().map_err(|e| e.to_string())?;
                let v: Value = serde_json::from_slice(&out.stdout)
                    .map_err(|e| format!("hetzner {path}: {e}"))?;
                if let Some(err) = v.get("error") {
                    return Err(format!("hetzner {path}: {err}"));
                }
                for x in v[key].as_array().into_iter().flatten() {
                    inv.add(
                        Item::new(kind, &x["id"].to_string())
                            .detail(x["name"].as_str().unwrap_or("").to_string()),
                    );
                }
            }
        }
    }
    Ok(())
}

/// Takes an inventory of `scope` from `sources`. A source that fails is
/// recorded as a warning (and fails the check in strict callers), never
/// silently skipped.
pub fn collect(scope: &str, label: &str, sources: &Sources) -> Inventory {
    let mut inv = Inventory::new(scope, label);
    for s in &sources.sources {
        if let Err(e) = collect_one(s, &mut inv) {
            inv.warnings.push(e);
        }
    }
    inv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_records_by_pointer_and_owner() {
        let t = r#"{"records":[{"id":"a","cua_name":"e2b-a"},{"id":"b","cua_name":null}]}"#;
        let r = records(
            "record:gateway",
            t,
            "/records",
            &Some("id".into()),
            &Some("cua_name".into()),
        )
        .unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].owner.as_deref(), Some("e2b-a"));
        assert_eq!(r[1].owner, None);
        let m = records("k", r#"{"x":{},"y":{}}"#, "", &None, &None).unwrap();
        assert_eq!(
            m.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["x", "y"]
        );
    }

    #[test]
    fn dir_source_lists_entries_and_excludes() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("vm1")).unwrap();
        std::fs::write(d.path().join("vm1/disk.img"), b"abc").unwrap();
        std::fs::write(d.path().join("run.log"), b"x").unwrap();
        let s = Sources {
            sources: vec![Source::Dir {
                kind: "vmm".into(),
                dir: d.path().to_string_lossy().into(),
                depth: 2,
                exclude: vec!["*.log".into()],
            }],
        };
        let inv = collect("t", "x", &s);
        assert_eq!(inv.ids("file:vmm"), ["vm1/disk.img".to_string()].into());
    }

    #[test]
    fn command_source_and_failures_become_warnings() {
        let s = Sources {
            sources: vec![
                Source::Command {
                    kind: "space".into(),
                    argv: vec!["echo".into(), r#"[{"name":"a"},{"name":"b"}]"#.into()],
                    env: vec![],
                    pointer: String::new(),
                    id_field: Some("name".into()),
                    owner_field: None,
                },
                Source::Command {
                    kind: "space".into(),
                    argv: vec!["/nonexistent/cmd".into()],
                    env: vec![],
                    pointer: String::new(),
                    id_field: None,
                    owner_field: None,
                },
            ],
        };
        let inv = collect("t", "x", &s);
        assert_eq!(inv.ids("space").len(), 2);
        assert_eq!(inv.warnings.len(), 1);
    }

    #[test]
    fn processes_are_found_by_pattern_and_home() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().to_string_lossy().to_string();
        let mut child = Command::new("sleep")
            .arg("30")
            .env("CUA_HOME", &home)
            .spawn()
            .unwrap();
        let s = Sources {
            sources: vec![Source::Processes {
                patterns: vec![("sleep 30".into(), "process:sleep".into())],
                home: Some(home.clone()),
            }],
        };
        let inv = collect("t", "x", &s);
        let other = Sources {
            sources: vec![Source::Processes {
                patterns: vec![("sleep 30".into(), "process:sleep".into())],
                home: Some("/definitely/not/this/home".into()),
            }],
        };
        let none = collect("t", "x", &other);
        let _ = child.kill();
        let _ = child.wait();
        // /proc/<pid>/environ is Linux only; elsewhere the home filter hides it.
        if cfg!(target_os = "linux") {
            assert_eq!(inv.of_kind("process:sleep").count(), 1);
        }
        assert_eq!(none.of_kind("process:sleep").count(), 0);
    }
}
