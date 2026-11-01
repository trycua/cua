//! envd's filesystem semantics, over the guest.
//!
//! E2B's envd runs as root: it reads and writes anywhere, gives what it
//! creates to the request's user, and reports entries the way Go's `os`
//! package sees them (`lstat` for type and size, the target's permissions
//! for a symlink, `filepath.EvalSymlinks` for its target, `fs.FileMode`
//! strings with the setuid/setgid/sticky letters, directory listings in
//! lexical walk order). cua-spacesd runs as an ordinary user in the Linux
//! image and hides the special mode bits, so metadata and the operations
//! that need rights run as root through one short `sh` script each (a guest
//! with GNU `find` is assumed, as every Cua Linux image has one).
//!
//! The error messages are the ones envd's Go code produces, because the SDKs
//! print them.

use super::pb::timestamp;
use crate::connect::{Code, RpcError};
use cua_spacesd_client::SpacesdClient;
use serde_json::{Map, Value, json};

/// One entry as `find` printed it.
#[derive(Debug, Clone, PartialEq)]
pub struct Ent {
    pub kind: char,
    pub size: u64,
    /// Permission bits including setuid/setgid/sticky (what `stat %a` shows).
    pub mode: u32,
    pub owner: String,
    pub group: String,
    pub mtime_s: i64,
    pub mtime_ns: u32,
    /// `readlink` of a symlink, as stored.
    pub raw_target: String,
    pub path: String,
    /// For a symlink: its resolved target (`EvalSymlinks`; the path itself
    /// when it cannot be resolved) and the target's permission bits (0 when
    /// there is none).
    pub resolved: Option<(String, u32)>,
    /// For a symlink: what it points at (`f` file, `d` directory, `?` a
    /// broken link or anything else). envd reports this as the entry's
    /// `type` and marks the link with `isSymlink`.
    pub target_kind: char,
}

/// Go's `fs.FileMode.String()` for a `find %y` type and the mode bits
/// (`-rw-r--r--`, `dtrwxrwxrwx`, `Lrwxrwxrwx`, `Dcrw-rw-rw-`, `urwxr-xr-x`).
pub fn go_mode_string(kind: char, mode: u32) -> String {
    let mut s = String::with_capacity(12);
    match kind {
        'd' => s.push('d'),
        'l' => s.push('L'),
        'c' | 'b' => s.push('D'),
        'p' => s.push('p'),
        's' => s.push('S'),
        _ => {}
    }
    if mode & 0o4000 != 0 {
        s.push('u');
    }
    if mode & 0o2000 != 0 {
        s.push('g');
    }
    if kind == 'c' {
        s.push('c');
    }
    if mode & 0o1000 != 0 {
        s.push('t');
    }
    if s.is_empty() {
        s.push('-');
    }
    let perm = if kind == 'l' { 0o777 } else { mode };
    for (i, c) in "rwxrwxrwx".chars().enumerate() {
        s.push(if perm & (1 << (8 - i)) != 0 { c } else { '-' });
    }
    s
}

impl Ent {
    pub fn name(&self) -> &str {
        if self.path == "/" {
            return "/";
        }
        self.path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
    }

    /// envd's `EntryInfo` JSON.
    pub fn json(&self) -> Value {
        let mut m = Map::new();
        m.insert("name".into(), json!(self.name()));
        m.insert(
            "type".into(),
            json!(match self.kind {
                'f' => "FILE_TYPE_FILE",
                'd' => "FILE_TYPE_DIRECTORY",
                'l' => match self.target_kind {
                    'f' => "FILE_TYPE_FILE",
                    'd' => "FILE_TYPE_DIRECTORY",
                    _ => "FILE_TYPE_UNSPECIFIED",
                },
                _ => "FILE_TYPE_UNSPECIFIED",
            }),
        );
        if self.kind == 'l' {
            m.insert("isSymlink".into(), json!(true));
        }
        m.insert("path".into(), json!(self.path));
        m.insert("size".into(), json!(self.size.to_string()));
        let mode = match (&self.resolved, self.kind) {
            (Some((_, m)), 'l') => *m,
            _ => self.mode,
        };
        m.insert("mode".into(), json!(mode & 0o777));
        m.insert(
            "permissions".into(),
            json!(go_mode_string(self.kind, self.mode)),
        );
        m.insert("owner".into(), json!(self.owner));
        m.insert("group".into(), json!(self.group));
        let ts = pbjson_types::Timestamp {
            seconds: self.mtime_s,
            nanos: self.mtime_ns as i32,
        };
        m.insert("modifiedTime".into(), json!(timestamp(&ts)));
        if self.kind == 'l' {
            let target = self
                .resolved
                .as_ref()
                .map(|(t, _)| t.clone())
                .unwrap_or_else(|| self.path.clone());
            m.insert("symlinkTarget".into(), json!(target));
        }
        Value::Object(m)
    }
}

/// Lexical walk order, as Go's `filepath.WalkDir` yields entries.
pub fn walk_order(a: &str, b: &str) -> std::cmp::Ordering {
    a.split('/').cmp(b.split('/'))
}

/// The sandbox's filesystem, as envd sees it.
pub struct Fs<'a> {
    pub c: &'a SpacesdClient,
    /// The user cua-spacesd runs as.
    pub driver: &'a str,
}

const FIND_FORMAT: &str = "%y\\t%s\\t%m\\t%u\\t%g\\t%T@\\t%l\\t%p\\0";

/// A failed script: `None` when the guest cannot run it (use the native
/// calls), else the error to answer with.
type Fail = Option<RpcError>;

fn some(e: RpcError) -> Fail {
    Some(e)
}

impl Fs<'_> {
    /// Runs a script as root. `Err(None)`: the guest cannot run it (no `sh`
    /// or GNU `find`, as on a Windows or macOS guest or a bare container).
    async fn root(&self, script: &str, args: &[&str]) -> Result<cua_spacesd_client::Output, Fail> {
        let out = crate::guest::run_as(self.c, self.driver, "root", script, args)
            .await
            .map_err(|_| None)?;
        match out.status.code {
            None | Some(127) => Err(None),
            Some(1) if String::from_utf8_lossy(&out.stderr).contains("unknown predicate") => {
                Err(None)
            }
            _ => Ok(out),
        }
    }

    /// The entries of `find <path> -mindepth min -maxdepth max`, fixed up
    /// for symlinks, in walk order.
    async fn find(&self, path: &str, min: u32, max: u32, as_dir: bool) -> Result<Vec<Ent>, Fail> {
        let script = format!(
            "p=\"$1\"; [ -e \"$p\" ] || [ -L \"$p\" ] || exit 44; \
             if [ \"$4\" = dir ]; then [ -d \"$p\" ] || {{ readlink -e -- \"$p\" || printf '%s' \"$p\"; exit 21; }}; p=\"${{p%/}}/\"; fi; \
             find \"$p\" -mindepth \"$2\" -maxdepth \"$3\" -printf '{FIND_FORMAT}'"
        );
        let out = self
            .root(
                &script,
                &[
                    path,
                    &min.to_string(),
                    &max.to_string(),
                    if as_dir { "dir" } else { "any" },
                ],
            )
            .await?;
        match out.status.code {
            Some(44) => {
                return Err(some(RpcError::not_found(if as_dir {
                    format!("path not found: lstat {path}: no such file or directory")
                } else {
                    format!("file not found: lstat {path}: no such file or directory")
                })));
            }
            Some(21) => {
                let resolved = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
                return Err(some(RpcError::invalid(format!(
                    "path is not a directory: {resolved}"
                ))));
            }
            _ if !out.success() => {
                return Err(some(RpcError::internal(
                    String::from_utf8_lossy(&out.stderr).trim().to_string(),
                )));
            }
            _ => {}
        }
        let mut ents: Vec<Ent> = out
            .stdout
            .split(|b| *b == 0)
            .filter(|r| !r.is_empty())
            .filter_map(|r| parse_record(&String::from_utf8_lossy(r)))
            .collect();
        ents.sort_by(|a, b| walk_order(&a.path, &b.path));
        self.resolve_links(&mut ents).await?;
        Ok(ents)
    }

    /// Fills in `resolved` for the symlinks among `ents`.
    async fn resolve_links(&self, ents: &mut [Ent]) -> Result<(), Fail> {
        let links: Vec<usize> = ents
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == 'l')
            .map(|(i, _)| i)
            .collect();
        if links.is_empty() {
            return Ok(());
        }
        let paths: Vec<&str> = links.iter().map(|i| ents[*i].path.as_str()).collect();
        let out = self
            .root(
                "for p in \"$@\"; do t=$(readlink -e -- \"$p\" 2>/dev/null) || t=\"\"; \
                 m=$(stat -L -c %a -- \"$p\" 2>/dev/null) || m=0; \
                 if [ -d \"$p\" ]; then k=d; elif [ -f \"$p\" ]; then k=f; else k=?; fi; \
                 printf '%s\\t%s\\t%s\\0' \"$m\" \"$k\" \"$t\"; done",
                &paths,
            )
            .await?;
        let recs: Vec<String> = out
            .stdout
            .split(|b| *b == 0)
            .filter(|r| !r.is_empty())
            .map(|r| String::from_utf8_lossy(r).to_string())
            .collect();
        for (n, i) in links.iter().enumerate() {
            let (m, rest) = recs
                .get(n)
                .and_then(|r| r.split_once('\t'))
                .unwrap_or(("0", "?\t"));
            let (k, t) = rest.split_once('\t').unwrap_or(("?", ""));
            ents[*i].target_kind = k.chars().next().unwrap_or('?');
            let target = if t.is_empty() {
                ents[*i].path.clone()
            } else {
                t.to_string()
            };
            ents[*i].resolved = Some((target, u32::from_str_radix(m, 8).unwrap_or(0)));
        }
        Ok(())
    }

    /// `Stat`: the entry itself (`lstat`).
    pub async fn stat(&self, path: &str) -> Result<Ent, RpcError> {
        match self.find(path, 0, 0, false).await {
            Ok(mut v) => v.pop().ok_or_else(|| {
                RpcError::not_found(format!(
                    "file not found: lstat {path}: no such file or directory"
                ))
            }),
            Err(Some(e)) => Err(e),
            Err(None) => native::stat(self.c, path).await,
        }
    }

    /// `ListDir`: the entries below `path` down to `depth`, in walk order.
    pub async fn list(&self, path: &str, depth: u32) -> Result<Vec<Ent>, RpcError> {
        match self.find(path, 1, depth.max(1), true).await {
            // envd lists regular files, directories and symlinks that lead
            // somewhere; sockets, pipes, devices and broken links are left out.
            Ok(v) => Ok(v
                .into_iter()
                .filter(|e| match e.kind {
                    'f' | 'd' => true,
                    'l' => e.resolved.as_ref().is_some_and(|(t, _)| *t != e.path),
                    _ => false,
                })
                .collect()),
            Err(Some(e)) => Err(e),
            Err(None) => native::list(self.c, path, depth.max(1)).await,
        }
    }

    /// `MakeDir`: makes the directory and any missing parents as root and
    /// gives what it made to `user`.
    pub async fn make_dir(&self, path: &str, user: &str) -> Result<Ent, RpcError> {
        let out = match self
            .root(
                "p=\"$1\"; \
                 if [ -e \"$p\" ] || [ -L \"$p\" ]; then [ -d \"$p\" ] && exit 17; exit 20; fi; \
                 top=\"$p\"; q=$(dirname \"$p\"); while [ ! -e \"$q\" ] && [ ! -L \"$q\" ]; do top=\"$q\"; q=$(dirname \"$q\"); done; \
                 [ -d \"$q\" ] || exit 21; \
                 mkdir -p -- \"$p\" || exit 1; [ \"$2\" = root ] || chown -R \"$2\": \"$top\"",
                &[path, user],
            )
            .await
        {
            Ok(o) => o,
            Err(Some(e)) => return Err(e),
            Err(None) => return native::make_dir(self.c, path).await,
        };
        match out.status.code {
            Some(0) => {}
            Some(17) => {
                return Err(RpcError::new(
                    Code::AlreadyExists,
                    format!("directory '{path}' already exists"),
                ));
            }
            Some(20) => {
                return Err(RpcError::invalid(format!(
                    "path already exists but it is not a directory: {path}"
                )));
            }
            Some(21) => {
                return Err(RpcError::internal(format!(
                    "error getting file info: stat {path}: not a directory"
                )));
            }
            _ => {
                return Err(RpcError::internal(
                    String::from_utf8_lossy(&out.stderr).trim().to_string(),
                ));
            }
        }
        self.stat(path).await
    }

    /// `Remove`: recursive; a missing path is fine. cua-spacesd removes
    /// what it may; the rest goes through root.
    pub async fn remove(&self, path: &str) -> Result<(), RpcError> {
        match native::remove(self.c, path).await {
            Ok(()) => {}
            Err(e) if e.code == Code::PermissionDenied => {}
            Err(e) => return Err(e),
        }
        // Whatever it could not remove (other users' files).
        if self.stat(path).await.is_err() {
            return Ok(());
        }
        match self.root("rm -rf -- \"$1\"", &[path]).await {
            Ok(out) if out.success() => Ok(()),
            Ok(out) => Err(RpcError::internal(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            )),
            Err(Some(e)) => Err(e),
            Err(None) => Ok(()),
        }
    }

    /// `Move`: rename as root, making the destination's parents (for
    /// `user`); a directory in the way is an error, a file is replaced.
    pub async fn rename(&self, src: &str, dst: &str, user: &str) -> Result<Ent, RpcError> {
        let out = match self
            .root(
                "src=\"$1\"; dst=\"$2\"; \
                 [ -e \"$src\" ] || [ -L \"$src\" ] || exit 44; \
                 if [ -d \"$dst\" ] && [ ! -L \"$dst\" ]; then exit 17; fi; \
                 d=$(dirname \"$dst\"); top=\"\"; q=\"$d\"; while [ ! -e \"$q\" ]; do top=\"$q\"; q=$(dirname \"$q\"); done; \
                 mkdir -p -- \"$d\" || exit 1; [ -z \"$top\" ] || [ \"$3\" = root ] || chown -R \"$3\": \"$top\"; \
                 mv -f -T -- \"$src\" \"$dst\"",
                &[src, dst, user],
            )
            .await
        {
            Ok(o) => o,
            Err(Some(e)) => return Err(e),
            Err(None) => return native::rename(self.c, src, dst).await,
        };
        match out.status.code {
            Some(0) => {}
            Some(44) => {
                return Err(RpcError::not_found(format!(
                    "source file not found: rename {src} {dst}: no such file or directory"
                )));
            }
            Some(17) => {
                return Err(RpcError::internal(format!(
                    "error renaming: rename {src} {dst}: file exists"
                )));
            }
            _ => {
                return Err(RpcError::internal(format!(
                    "error renaming: rename {src} {dst}: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
        }
        self.stat(dst).await
    }
}

/// cua-spacesd's own filesystem calls: what a guest without GNU tools
/// (Windows, macOS, bare containers) gets. Same messages, less detail
/// (cua-spacesd hides the special mode bits and device kinds).
mod native {
    use super::*;
    use cua_spacesd_client::pb;

    fn ent(e: &pb::EntryInfo) -> Ent {
        let kind = match e.r#type {
            2 => 'd',
            3 => 'l',
            4 => 'p',
            _ => 'f',
        };
        let (secs, nanos) = e
            .modified_at
            .as_ref()
            .map(|t| (t.seconds, t.nanos.max(0) as u32))
            .unwrap_or((0, 0));
        Ent {
            kind,
            size: e.size,
            mode: e.mode,
            owner: e.owner.clone(),
            group: e.group.clone(),
            mtime_s: secs,
            mtime_ns: nanos,
            raw_target: e.symlink_target.clone(),
            path: e.path.clone(),
            resolved: None,
            target_kind: '?',
        }
    }

    fn missing(path: &str, what: &str) -> RpcError {
        RpcError::not_found(format!(
            "{what} not found: lstat {path}: no such file or directory"
        ))
    }

    async fn lstat(
        c: &SpacesdClient,
        path: &str,
        follow: bool,
    ) -> Result<pb::EntryInfo, tonic::Status> {
        Ok(c.filesystem()
            .stat(pb::StatRequest {
                path: path.to_string(),
                no_follow_symlinks: !follow,
                ..Default::default()
            })
            .await?
            .into_inner()
            .entry
            .unwrap_or_default())
    }

    pub async fn stat(c: &SpacesdClient, path: &str) -> Result<Ent, RpcError> {
        let raw = lstat(c, path, false).await.map_err(|s| {
            if s.code() == tonic::Code::NotFound {
                missing(path, "file")
            } else {
                s.into()
            }
        })?;
        let mut e = ent(&raw);
        if e.kind == 'l' {
            let followed = lstat(c, path, true).await;
            e.target_kind = match &followed {
                Ok(t) => match t.r#type {
                    2 => 'd',
                    1 => 'f',
                    _ => '?',
                },
                Err(_) => '?',
            };
            e.resolved = Some(match followed {
                Ok(t) => (
                    if raw.symlink_target.starts_with('/') {
                        raw.symlink_target.clone()
                    } else {
                        t.path
                    },
                    t.mode,
                ),
                Err(_) => (path.to_string(), 0),
            });
        }
        Ok(e)
    }

    pub async fn list(c: &SpacesdClient, path: &str, depth: u32) -> Result<Vec<Ent>, RpcError> {
        let mut out = Vec::new();
        let mut token = String::new();
        loop {
            let resp = c
                .filesystem()
                .list_dir(pb::ListDirRequest {
                    path: path.to_string(),
                    depth,
                    include_hidden: true,
                    page_size: 0,
                    page_token: token,
                    ..Default::default()
                })
                .await
                .map_err(|s| match s.code() {
                    tonic::Code::NotFound => missing(path, "path"),
                    tonic::Code::FailedPrecondition => {
                        RpcError::invalid(format!("path is not a directory: {path}"))
                    }
                    _ => s.into(),
                })?
                .into_inner();
            out.extend(resp.entries.iter().map(ent));
            if resp.next_page_token.is_empty() {
                break;
            }
            token = resp.next_page_token;
        }
        out.sort_by(|a, b| walk_order(&a.path, &b.path));
        Ok(out)
    }

    pub async fn make_dir(c: &SpacesdClient, path: &str) -> Result<Ent, RpcError> {
        let resp = c
            .filesystem()
            .make_dir(pb::MakeDirRequest {
                path: path.to_string(),
                parents: true,
                mode: 0,
                ..Default::default()
            })
            .await?
            .into_inner();
        if !resp.created {
            return Err(RpcError::new(
                Code::AlreadyExists,
                format!("directory '{path}' already exists"),
            ));
        }
        stat(c, path).await
    }

    pub async fn remove(c: &SpacesdClient, path: &str) -> Result<(), RpcError> {
        c.filesystem()
            .remove(pb::RemoveRequest {
                path: path.to_string(),
                recursive: true,
                missing_ok: true,
                ..Default::default()
            })
            .await?;
        Ok(())
    }

    pub async fn rename(c: &SpacesdClient, src: &str, dst: &str) -> Result<Ent, RpcError> {
        if let Ok(d) = lstat(c, dst, false).await
            && d.r#type == 2
        {
            return Err(RpcError::internal(format!(
                "error renaming: rename {src} {dst}: file exists"
            )));
        }
        c.filesystem()
            .r#move(pb::MoveRequest {
                source: src.to_string(),
                destination: dst.to_string(),
                overwrite: true,
                create_parents: true,
                ..Default::default()
            })
            .await
            .map_err(|s| {
                if s.code() == tonic::Code::NotFound {
                    RpcError::not_found(format!(
                        "source file not found: rename {src} {dst}: no such file or directory"
                    ))
                } else {
                    s.into()
                }
            })?;
        stat(c, dst).await
    }
}

fn parse_record(r: &str) -> Option<Ent> {
    let f: Vec<&str> = r.splitn(8, '\t').collect();
    if f.len() < 8 {
        return None;
    }
    let mtime = f[5];
    let (secs, frac) = mtime.split_once('.').unwrap_or((mtime, "0"));
    let nanos = format!("{frac:0<9}")[..9].parse::<u32>().unwrap_or(0);
    Some(Ent {
        kind: f[0].chars().next().unwrap_or('f'),
        size: f[1].parse().unwrap_or(0),
        mode: u32::from_str_radix(f[2], 8).unwrap_or(0),
        owner: f[3].to_string(),
        group: f[4].to_string(),
        mtime_s: secs.parse().unwrap_or(0),
        mtime_ns: nanos,
        raw_target: f[6].to_string(),
        path: f[7].to_string(),
        resolved: None,
        target_kind: '?',
    })
}

/// The JSON of a Stat answer.
pub fn stat_json(e: &Ent) -> Value {
    json!({"entry": e.json()})
}

/// The JSON of a ListDir answer.
pub fn list_json(es: &[Ent]) -> Value {
    json!({"entries": es.iter().map(Ent::json).collect::<Vec<_>>()})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_strings() {
        assert_eq!(go_mode_string('f', 0o644), "-rw-r--r--");
        assert_eq!(go_mode_string('d', 0o1777), "dtrwxrwxrwx");
        assert_eq!(go_mode_string('f', 0o4755), "urwxr-xr-x");
        assert_eq!(go_mode_string('f', 0o6755), "ugrwxr-xr-x");
        assert_eq!(go_mode_string('l', 0o777), "Lrwxrwxrwx");
        assert_eq!(go_mode_string('c', 0o666), "Dcrw-rw-rw-");
        assert_eq!(go_mode_string('b', 0o600), "Drw-------");
        assert_eq!(go_mode_string('p', 0o644), "prw-r--r--");
        assert_eq!(go_mode_string('s', 0o755), "Srwxr-xr-x");
    }

    #[test]
    fn walk_order_is_lexical_depth_first() {
        let mut v = vec!["/a/f2", "/a", "/empty", "/a/b", "/f1", "/.hidden"];
        v.sort_by(|a, b| walk_order(a, b));
        assert_eq!(v, ["/.hidden", "/a", "/a/b", "/a/f2", "/empty", "/f1"]);
    }

    #[test]
    fn parses_find_records() {
        let e = parse_record("l\t5\t777\tuser\tuser\t1614834367.1234500000\tf.txt\t/tmp/x/link")
            .unwrap();
        assert_eq!(e.kind, 'l');
        assert_eq!(e.mtime_ns, 123_450_000);
        assert_eq!(e.name(), "link");
        assert_eq!(e.raw_target, "f.txt");
    }

    #[test]
    fn symlink_entry_json_follows_envd() {
        let mut e = parse_record("l\t5\t777\tuser\tuser\t1.0\tf.txt\t/tmp/x/link").unwrap();
        e.resolved = Some(("/tmp/x/f.txt".into(), 0o4755));
        e.target_kind = 'f';
        let j = e.json();
        // The type is the target's; the link itself is marked.
        assert_eq!(j["type"], "FILE_TYPE_FILE");
        assert_eq!(j["isSymlink"], true);
        assert_eq!(j["permissions"], "Lrwxrwxrwx");
        assert_eq!(j["mode"], 0o755);
        assert_eq!(j["symlinkTarget"], "/tmp/x/f.txt");
        assert_eq!(j["size"], "5");
        // Broken: the target is the path itself and the mode is 0.
        e.resolved = Some(("/tmp/x/link".into(), 0));
        e.target_kind = '?';
        let j = e.json();
        assert_eq!(j["type"], "FILE_TYPE_UNSPECIFIED");
        assert_eq!(j["isSymlink"], true);
        assert_eq!(j["mode"], 0);
        assert_eq!(j["symlinkTarget"], "/tmp/x/link");
    }
}
