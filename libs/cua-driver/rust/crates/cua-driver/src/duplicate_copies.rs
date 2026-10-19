//! Other copies of Cua Driver.app that macOS knows about.
//!
//! A Mac can hold several copies of `CuaDriver.app`: an old release in
//! `~/Downloads`, one in the Trash, a benchmark folder. macOS resolves a bundle
//! id, an `open -a CuaDriver` and a System Settings permission row to *one* of
//! the registered copies, not necessarily the installed one. That is how a
//! 0.34.0 copy from the Trash got launched and labelled the permission row
//! "CuaDriver-0.34.0".
//!
//! This module finds those copies (LaunchServices' `lsregister -dump` plus
//! Spotlight's `mdfind`), and `cua-driver doctor --fix` makes LaunchServices
//! forget them with `lsregister -u`. It never deletes a file. The installer does
//! the same in shell (`unregister_other_cua_driver_copies` in
//! `scripts/_install-common.sh`); keep the two algorithms in step.
//!
//! The pure parts take injected inputs so unit tests never touch the real
//! LaunchServices database.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use crate::doctor::Probe;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Versions/A/\
Frameworks/LaunchServices.framework/Versions/A/Support/lsregister";
const MDFIND: &str = "/usr/bin/mdfind";
const SQLITE3: &str = "/usr/bin/sqlite3";
const PLUTIL: &str = "/usr/bin/plutil";

/// How long the (slow) LaunchServices dump may take before we give up on it.
const DUMP_TIMEOUT: Duration = Duration::from_secs(180);
const QUICK_TIMEOUT: Duration = Duration::from_secs(20);

/// Run `program args…` and return its stdout when it exits 0 within `timeout`.
pub type Run<'a> = &'a dyn Fn(&str, &[&str], Duration) -> Option<String>;

/// One app bundle that is not the installed one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCopy {
    pub path: PathBuf,
    pub version: Option<String>,
    /// False when the registration outlived the files (a deleted copy).
    pub exists: bool,
    /// LaunchServices has this copy registered, so macOS can launch it. A copy
    /// that only Spotlight lists is on disk but is not launched by `open -a`.
    pub registered: bool,
}

impl AppCopy {
    pub fn describe(&self) -> String {
        let mut text = self.path.display().to_string();
        match (&self.version, self.exists) {
            (Some(version), true) => text.push_str(&format!(" (version {version})")),
            (None, true) => {}
            (Some(version), false) => text.push_str(&format!(" (version {version}, files gone)")),
            (None, false) => text.push_str(" (files gone)"),
        }
        text
    }
}

/// The result of asking macOS to forget one copy.
#[derive(Debug, Clone)]
pub struct FixOutcome {
    pub copy: AppCopy,
    pub result: Result<(), String>,
}

/// Paths of every bundle in an `lsregister -dump` whose `identifier:` is exactly
/// `bundle_id`. The dump is a series of blocks split by a line of dashes; each
/// block has a `path:` line (with a trailing `(0x…)` record id) and an
/// `identifier:` line.
pub fn parse_lsregister_dump(dump: &str, bundle_id: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut path: Option<String> = None;
    let mut matches = false;
    let mut flush = |path: &mut Option<String>, matches: &mut bool| {
        if let Some(path) = path.take() {
            if *matches {
                found.push(PathBuf::from(path));
            }
        }
        *matches = false;
    };
    for line in dump.lines() {
        if line.starts_with("----------") {
            flush(&mut path, &mut matches);
        } else if let Some(rest) = line.strip_prefix("path:") {
            if rest.starts_with(char::is_whitespace) {
                path = Some(strip_record_id(rest.trim()));
            }
        } else if let Some(rest) = line.strip_prefix("identifier:") {
            if rest.starts_with(char::is_whitespace) {
                matches = rest.trim() == bundle_id;
            }
        }
    }
    flush(&mut path, &mut matches);
    found
}

/// `/Applications/CuaDriver.app (0x10268)` -> `/Applications/CuaDriver.app`.
fn strip_record_id(text: &str) -> String {
    match text.rfind(" (0x") {
        Some(index) if text.ends_with(')') => text[..index].to_owned(),
        _ => text.to_owned(),
    }
}

/// The copies that are not `active`, from every `(path, registered)` pair macOS
/// reported. A path reported both ways counts as registered.
pub fn other_copies(
    active: &Path,
    reported: impl IntoIterator<Item = (PathBuf, bool)>,
    version_of: impl Fn(&Path) -> Option<String>,
) -> Vec<AppCopy> {
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let active_real = canonical(active);
    let mut copies: Vec<AppCopy> = Vec::new();
    for (path, registered) in reported {
        if path == active || canonical(&path) == active_real {
            continue;
        }
        if let Some(seen) = copies.iter_mut().find(|copy| copy.path == path) {
            seen.registered |= registered;
            continue;
        }
        let exists = path.is_dir();
        let version = if exists { version_of(&path) } else { None };
        copies.push(AppCopy {
            path,
            version,
            exists,
            registered,
        });
    }
    copies.sort_by(|a, b| a.path.cmp(&b.path));
    copies
}

/// Every other bundle with `bundle_id` that LaunchServices (`lsregister -dump`)
/// or Spotlight (`mdfind`) knows. If the LaunchServices dump is unavailable the
/// Spotlight hits are treated as registered, so the doctor errs on the side of
/// warning.
pub fn discover(active: &Path, bundle_id: &str, run: Run) -> Vec<AppCopy> {
    let mut reported: Vec<(PathBuf, bool)> = Vec::new();
    let dump = run(LSREGISTER, &["-dump"], DUMP_TIMEOUT);
    let dump_available = dump.is_some();
    if let Some(dump) = dump {
        reported.extend(
            parse_lsregister_dump(&dump, bundle_id)
                .into_iter()
                .map(|path| (path, true)),
        );
    }
    let query = format!("kMDItemCFBundleIdentifier == '{bundle_id}'");
    if let Some(found) = run(MDFIND, &[&query], QUICK_TIMEOUT) {
        reported.extend(
            found
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|line| (PathBuf::from(line), !dump_available)),
        );
    }
    other_copies(active, reported, |bundle| bundle_version_of(bundle, run))
}

pub fn bundle_version_of(bundle: &Path, run: Run) -> Option<String> {
    let plist = bundle.join("Contents/Info.plist");
    let plist = plist.to_str()?;
    let out = run(
        PLUTIL,
        &[
            "-extract",
            "CFBundleShortVersionString",
            "raw",
            "-o",
            "-",
            plist,
        ],
        QUICK_TIMEOUT,
    )?;
    let version = out.trim();
    (!version.is_empty()).then(|| version.to_owned())
}

/// Make LaunchServices forget every copy. Files are never touched.
pub fn unregister(copies: &[AppCopy], run: Run) -> Vec<FixOutcome> {
    copies
        .iter()
        .map(|copy| {
            let path = copy.path.to_string_lossy().into_owned();
            let result = match run(LSREGISTER, &["-u", &path], QUICK_TIMEOUT) {
                Some(_) => Ok(()),
                None => Err(format!("`lsregister -u {path}` failed")),
            };
            FixOutcome {
                copy: copy.clone(),
                result,
            }
        })
        .collect()
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        "copy"
    } else {
        "copies"
    }
}

/// The doctor probe for duplicate copies.
///
/// `fixed` is `None` for a plain `doctor` run and the outcomes of `--fix`
/// otherwise; `remaining` is what macOS still reports after the fix.
pub fn duplicate_probe(
    active: &Path,
    app_name: &str,
    copies: &[AppCopy],
    indexed_only: &[AppCopy],
    fixed: Option<(&[FixOutcome], &[AppCopy])>,
) -> Probe {
    let label = "duplicate copies";
    if copies.is_empty() {
        let probe = Probe::ok(
            label,
            format!("only {} is registered with macOS", active.display()),
        );
        if indexed_only.is_empty() {
            return probe;
        }
        // On disk and listed by Spotlight, but not registered: `open -a` does not
        // launch them. Say so without nagging.
        return probe.with_detail(format!(
            "Spotlight also lists {} {} on disk that macOS does not launch:\n{}\n\
             Delete {} when you no longer need {}.",
            indexed_only.len(),
            plural(indexed_only.len()),
            indexed_only
                .iter()
                .map(|copy| format!("- {}", copy.describe()))
                .collect::<Vec<_>>()
                .join("\n"),
            if indexed_only.len() == 1 {
                "it"
            } else {
                "them"
            },
            if indexed_only.len() == 1 {
                "it"
            } else {
                "them"
            },
        ));
    }
    let list = |copies: &mut dyn Iterator<Item = &AppCopy>| {
        copies
            .map(|copy| format!("- {}", copy.describe()))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let Some((outcomes, remaining)) = fixed else {
        return Probe::warn(
            label,
            format!(
                "{} other {} of {app_name} known to macOS; it can open or label permissions \
                 with one of them instead of {}",
                copies.len(),
                plural(copies.len()),
                active.display()
            ),
        )
        .with_detail(format!(
            "{}\nRun `cua-driver doctor --fix` to make macOS forget them. No files are deleted.",
            list(&mut copies.iter())
        ));
    };

    let failed: Vec<&FixOutcome> = outcomes.iter().filter(|o| o.result.is_err()).collect();
    let done = outcomes.len() - failed.len();
    let mut detail = outcomes
        .iter()
        .map(|outcome| match &outcome.result {
            Ok(()) => format!("unregistered {}", outcome.copy.describe()),
            Err(error) => format!("could not unregister {}: {error}", outcome.copy.describe()),
        })
        .collect::<Vec<_>>()
        .join("\n");
    detail.push_str(
        "\nNo files were deleted; remove the old copies yourself if you do not need them.",
    );
    if !remaining.is_empty() {
        detail.push_str(&format!(
            "\nmacOS still reports {} {} (it re-registers apps it can see in indexed folders). \
             Move or delete:\n{}",
            remaining.len(),
            plural(remaining.len()),
            list(&mut remaining.iter())
        ));
        return Probe::warn(
            label,
            format!(
                "unregistered {done} of {} other {}, but macOS still knows {}",
                outcomes.len(),
                plural(outcomes.len()),
                remaining.len()
            ),
        )
        .with_detail(detail);
    }
    if failed.is_empty() {
        Probe::ok(
            label,
            format!(
                "unregistered {done} other {}; macOS will now only use {}",
                plural(done),
                active.display()
            ),
        )
        .with_detail(detail)
    } else {
        Probe::warn(
            label,
            format!(
                "could not unregister {} of {} copies",
                failed.len(),
                outcomes.len()
            ),
        )
        .with_detail(detail)
    }
}

// ── Permission rows ───────────────────────────────────────────────────────

/// One row of the TCC `access` table that mentions Cua Driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TccRow {
    pub service: String,
    pub client: String,
    /// 0 = bundle identifier, 1 = absolute path of an executable.
    pub client_type: i64,
}

/// Cua Driver rows in the current user's permission database. `Err` carries the
/// plain reason, normally that macOS only lets apps with Full Disk Access read it.
pub fn read_tcc_rows(home: &Path, run: Run) -> Result<Vec<TccRow>, String> {
    let db = home.join("Library/Application Support/com.apple.TCC/TCC.db");
    let db_text = db.to_string_lossy().into_owned();
    let sql = "SELECT service, client, client_type FROM access \
               WHERE client = 'com.trycua.driver' OR client LIKE '%cua-driver%' \
               OR client LIKE '%CuaDriver%';";
    let out =
        run(SQLITE3, &["-separator", "|", &db_text, sql], QUICK_TIMEOUT).ok_or_else(|| {
            "macOS only lets apps with Full Disk Access read the permission database".to_owned()
        })?;
    Ok(parse_tcc_rows(&out))
}

pub fn parse_tcc_rows(output: &str) -> Vec<TccRow> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '|');
            let service = parts.next()?.to_owned();
            let client = parts.next()?.to_owned();
            let client_type = parts.next()?.trim().parse().ok()?;
            Some(TccRow {
                service,
                client,
                client_type,
            })
        })
        .collect()
}

fn service_name(service: &str) -> &str {
    match service {
        "kTCCServiceAccessibility" => "Accessibility",
        "kTCCServiceScreenCapture" => "Screen Recording",
        other => other,
    }
}

const REMOVE_ROWS_HELP: &str = "To remove one stale row: System Settings > Privacy & Security > \
Accessibility (and Screen & System Audio Recording), select the extra Cua Driver entry and click \
the minus button.\nTo clear every Cua Driver row and start over: `tccutil reset Accessibility \
com.trycua.driver`, `tccutil reset ScreenCapture com.trycua.driver`, then \
`cua-driver permissions grant`.";

/// The doctor probe for permission rows tied to a different path or version.
///
/// Rows keyed by bundle id follow whichever copy macOS resolves; rows keyed by
/// an absolute path belong to that exact executable. A path row that is not the
/// installed executable is stale.
pub fn permission_row_probe(
    active_executable: &Path,
    rows: Result<Vec<TccRow>, String>,
    version_of: impl Fn(&Path) -> Option<String>,
) -> Probe {
    let label = "permission rows";
    let rows = match rows {
        Ok(rows) => rows,
        Err(reason) => return Probe::ok(label, format!("not checked: {reason}")),
    };
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let active_real = canonical(active_executable);
    let stale: Vec<&TccRow> = rows
        .iter()
        .filter(|row| row.client_type == 1)
        .filter(|row| {
            let path = Path::new(&row.client);
            path != active_executable && canonical(path) != active_real
        })
        .collect();
    if stale.is_empty() {
        return Probe::ok(label, "none tied to another path or version");
    }
    let lines: Vec<String> = stale
        .iter()
        .map(|row| {
            let path = Path::new(&row.client);
            let bundle = row
                .client
                .split_once("/Contents/MacOS/")
                .map(|(bundle, _)| PathBuf::from(bundle));
            let version = bundle.as_deref().and_then(&version_of);
            let state = match (version, path.exists()) {
                (Some(version), true) => format!("version {version}"),
                (None, true) => "other copy".to_owned(),
                (Some(version), false) => format!("version {version}, files gone"),
                (None, false) => "files gone".to_owned(),
            };
            format!("- {}: {} ({state})", service_name(&row.service), row.client)
        })
        .collect();
    Probe::warn(
        label,
        format!(
            "{} permission row{} belong{} to a different Cua Driver path or version than {}",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" },
            if stale.len() == 1 { "s" } else { "" },
            active_executable.display()
        ),
    )
    .with_detail(format!("{}\n{REMOVE_ROWS_HELP}", lines.join("\n")))
}

// ── Process runner ────────────────────────────────────────────────────────

/// Run a program, returning stdout when it exits 0 within `timeout`.
pub fn run_program(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let bytes = reader.join().ok()?;
    status
        .filter(|status| status.success())
        .map(|_| String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::Status;

    const DUMP: &str = "\
--------------------------------------------------------------------------------
bundle id:                  Cua Driver (0xf0ac)
path:                       /Applications/CuaDriver.app (0x10268)
name:                       Cua Driver
identifier:                 com.trycua.driver
version:                    0.36 ({length = 32, bytes = 0x00})
infoDictionary:             14 values (144140 (0x2330c))
                            {
                                CFBundleIdentifier = \"com.trycua.driver\";
                            }
--------------------------------------------------------------------------------
bundle id:                  CuaDriver-0.34.0 (0x1)
path:                       /Users/me/.Trash/Old Apps/CuaDriver.app (0x2)
identifier:                 com.trycua.driver
--------------------------------------------------------------------------------
bundle id:                  Cua Driver Local (0x3)
path:                       /Applications/CuaDriverLocal.app (0x4)
identifier:                 com.trycua.driver.local
--------------------------------------------------------------------------------
bundle id:                  Other (0x5)
path:                       /tmp/bench/CuaDriver.app (0x6)
identifier:                 com.trycua.driver
--------------------------------------------------------------------------------
";

    #[test]
    fn dump_parser_returns_every_bundle_with_the_exact_identifier() {
        assert_eq!(
            parse_lsregister_dump(DUMP, "com.trycua.driver"),
            [
                PathBuf::from("/Applications/CuaDriver.app"),
                PathBuf::from("/Users/me/.Trash/Old Apps/CuaDriver.app"),
                PathBuf::from("/tmp/bench/CuaDriver.app"),
            ],
            "the local product (com.trycua.driver.local) must not match, \
             and a path with spaces and its (0x..) record id parse cleanly"
        );
        assert_eq!(
            parse_lsregister_dump(DUMP, "com.trycua.driver.local"),
            [PathBuf::from("/Applications/CuaDriverLocal.app")]
        );
    }

    #[test]
    fn a_second_bundle_at_another_path_is_found_and_the_installed_one_is_not() {
        let root = tempfile::tempdir().unwrap();
        let active = root.path().join("Applications/CuaDriver.app");
        let old = root.path().join("Downloads/CuaDriver.app");
        let gone = root.path().join(".Trash/CuaDriver.app");
        std::fs::create_dir_all(&active).unwrap();
        std::fs::create_dir_all(&old).unwrap();
        let alias = root.path().join("alias.app");
        std::os::unix::fs::symlink(&active, &alias).unwrap();

        let copies = other_copies(
            &active,
            [
                (active.clone(), true),
                (old.clone(), false),
                (old.clone(), true),
                (gone.clone(), true),
                (alias, true),
            ],
            |path| (path == old).then(|| "0.34.0".to_owned()),
        );

        assert_eq!(
            copies,
            [
                AppCopy {
                    path: gone,
                    version: None,
                    exists: false,
                    registered: true
                },
                AppCopy {
                    path: old,
                    version: Some("0.34.0".to_owned()),
                    exists: true,
                    registered: true
                },
            ]
        );
    }

    #[test]
    fn discover_merges_launchservices_and_spotlight_and_reads_versions() {
        let root = tempfile::tempdir().unwrap();
        let active = root.path().join("Applications/CuaDriver.app");
        let old = root.path().join("tmp/CuaDriver.app");
        let indexed = root.path().join("images/CuaDriver.app");
        for dir in [&active, &old, &indexed] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let dump = format!(
            "{sep}\npath:   {} (0x1)\nidentifier:   com.trycua.driver\n\
             {sep}\npath:   {} (0x2)\nidentifier:   com.trycua.driver\n",
            active.display(),
            old.display(),
            sep = "-".repeat(40)
        );
        let spotlight = format!("{}\n{}\n\n", active.display(), indexed.display());
        let run = move |program: &str, args: &[&str], _: Duration| -> Option<String> {
            match (program, args.first().copied()) {
                (LSREGISTER, Some("-dump")) => Some(dump.clone()),
                (MDFIND, _) => Some(spotlight.clone()),
                (PLUTIL, _) => Some("0.36.0\n".to_owned()),
                _ => None,
            }
        };

        let copies = discover(&active, "com.trycua.driver", &run);

        let paths: Vec<_> = copies.iter().map(|c| c.path.clone()).collect();
        assert_eq!(paths, [indexed.clone(), old.clone()]);
        assert!(copies
            .iter()
            .all(|c| c.version.as_deref() == Some("0.36.0")));
        // Only LaunchServices' own dump makes a copy launchable; Spotlight alone
        // just means the files exist.
        let registered: Vec<_> = copies.iter().map(|c| c.registered).collect();
        assert_eq!(registered, [false, true]);
    }

    #[test]
    fn without_a_launchservices_dump_spotlight_hits_count_as_registered() {
        let root = tempfile::tempdir().unwrap();
        let active = root.path().join("Applications/CuaDriver.app");
        let found = root.path().join("tmp/CuaDriver.app");
        std::fs::create_dir_all(&found).unwrap();
        let spotlight = format!("{}\n", found.display());
        let run = move |program: &str, _: &[&str], _: Duration| -> Option<String> {
            (program == MDFIND).then(|| spotlight.clone())
        };

        let copies = discover(&active, "com.trycua.driver", &run);

        assert_eq!(copies.len(), 1);
        assert!(copies[0].registered, "cannot prove it is unregistered");
    }

    #[test]
    fn copies_only_spotlight_lists_do_not_raise_a_warning() {
        let active = Path::new("/Applications/CuaDriver.app");
        let indexed = [copy("/Users/me/Downloads/CuaDriver.app", Some("0.34.0"))];

        let probe = duplicate_probe(active, "CuaDriver", &[], &indexed, None);

        assert_eq!(probe.status, Status::Ok);
        assert!(probe.message.contains("only /Applications/CuaDriver.app"));
        let detail = probe.detail.unwrap();
        assert!(detail.contains("Spotlight also lists 1 copy on disk that macOS does not launch"));
        assert!(detail.contains("Downloads/CuaDriver.app (version 0.34.0)"));
    }

    #[test]
    fn unregister_uses_lsregister_u_per_copy_and_reports_failures() {
        let calls = std::cell::RefCell::new(Vec::<Vec<String>>::new());
        let run = |program: &str, args: &[&str], _: Duration| -> Option<String> {
            let mut call = vec![program.to_owned()];
            call.extend(args.iter().map(|a| (*a).to_owned()));
            calls.borrow_mut().push(call);
            (args.last() != Some(&"/bad/CuaDriver.app")).then(String::new)
        };
        let copies = [
            AppCopy {
                path: "/good/CuaDriver.app".into(),
                version: None,
                exists: true,
                registered: true,
            },
            AppCopy {
                path: "/bad/CuaDriver.app".into(),
                version: None,
                exists: true,
                registered: true,
            },
        ];

        let outcomes = unregister(&copies, &run);

        assert!(outcomes[0].result.is_ok());
        assert!(outcomes[1].result.is_err());
        assert_eq!(
            calls.borrow()[0],
            [LSREGISTER, "-u", "/good/CuaDriver.app"],
            "only registration is removed; nothing deletes a file"
        );
    }

    fn copy(path: &str, version: Option<&str>) -> AppCopy {
        AppCopy {
            path: path.into(),
            version: version.map(str::to_owned),
            exists: true,
            registered: true,
        }
    }

    #[test]
    fn doctor_warns_about_duplicate_copies_and_points_at_fix() {
        let active = Path::new("/Applications/CuaDriver.app");
        let copies = [
            copy("/Users/me/.Trash/CuaDriver.app", Some("0.34.0")),
            copy("/tmp/CuaDriver.app", Some("0.36.0")),
        ];

        let probe = duplicate_probe(active, "CuaDriver", &copies, &[], None);

        assert_eq!(probe.status, Status::Warn);
        assert!(probe.message.contains("2 other copies of CuaDriver"));
        let detail = probe.detail.unwrap();
        assert!(detail.contains("/Users/me/.Trash/CuaDriver.app (version 0.34.0)"));
        assert!(detail.contains("cua-driver doctor --fix"));
        assert!(detail.contains("No files are deleted"));
    }

    #[test]
    fn doctor_is_quiet_when_only_the_installed_copy_is_registered() {
        let probe = duplicate_probe(
            Path::new("/Applications/CuaDriver.app"),
            "CuaDriver",
            &[],
            &[],
            None,
        );

        assert_eq!(probe.status, Status::Ok);
        assert!(probe.message.contains("only /Applications/CuaDriver.app"));
    }

    #[test]
    fn fix_reports_what_it_unregistered_and_what_macos_still_knows() {
        let active = Path::new("/Applications/CuaDriver.app");
        let a = copy("/tmp/a/CuaDriver.app", Some("0.34.0"));
        let b = copy("/tmp/b/CuaDriver.app", None);
        let outcomes = [
            FixOutcome {
                copy: a.clone(),
                result: Ok(()),
            },
            FixOutcome {
                copy: b.clone(),
                result: Ok(()),
            },
        ];

        let clean = duplicate_probe(
            active,
            "CuaDriver",
            &[a.clone(), b.clone()],
            &[],
            Some((&outcomes, &[])),
        );
        assert_eq!(clean.status, Status::Ok);
        assert!(clean.message.contains(
            "unregistered 2 other copies; macOS will now only use /Applications/CuaDriver.app"
        ));
        assert!(clean.detail.unwrap().contains("No files were deleted"));

        let reregistered = duplicate_probe(
            active,
            "CuaDriver",
            &[a.clone(), b.clone()],
            &[],
            Some((&outcomes, std::slice::from_ref(&b))),
        );
        assert_eq!(reregistered.status, Status::Warn);
        assert!(reregistered.message.contains("macOS still knows 1"));
        assert!(reregistered.detail.unwrap().contains("Move or delete"));

        let failed = [FixOutcome {
            copy: a.clone(),
            result: Err("`lsregister -u` failed".to_owned()),
        }];
        let probe = duplicate_probe(active, "CuaDriver", &[a], &[], Some((&failed, &[])));
        assert_eq!(probe.status, Status::Warn);
        assert!(probe.message.contains("could not unregister 1 of 1"));
    }

    #[test]
    fn permission_rows_tied_to_another_path_are_flagged_with_removal_steps() {
        let active = Path::new("/Applications/CuaDriver.app/Contents/MacOS/cua-driver");
        let rows = Ok(parse_tcc_rows(
            "kTCCServiceAccessibility|com.trycua.driver|0\n\
             kTCCServiceAccessibility|/Applications/CuaDriver.app/Contents/MacOS/cua-driver|1\n\
             kTCCServiceScreenCapture|/Users/me/Downloads/CuaDriver.app/Contents/MacOS/cua-driver|1\n",
        ));

        let probe = permission_row_probe(active, rows, |bundle| {
            bundle
                .ends_with("Downloads/CuaDriver.app")
                .then(|| "0.34.0".to_owned())
        });

        assert_eq!(probe.status, Status::Warn);
        assert!(probe
            .message
            .starts_with("1 permission row belongs to a different"));
        let detail = probe.detail.unwrap();
        assert!(detail.contains(
            "- Screen Recording: /Users/me/Downloads/CuaDriver.app/Contents/MacOS/cua-driver \
             (version 0.34.0, files gone)"
        ));
        assert!(detail.contains("click the minus button"));
        assert!(detail.contains("tccutil reset Accessibility com.trycua.driver"));
    }

    #[test]
    fn permission_rows_for_the_installed_app_and_its_bundle_id_are_fine() {
        let active = Path::new("/Applications/CuaDriver.app/Contents/MacOS/cua-driver");
        let rows = Ok(parse_tcc_rows(
            "kTCCServiceAccessibility|com.trycua.driver|0\n\
             kTCCServiceAccessibility|/Applications/CuaDriver.app/Contents/MacOS/cua-driver|1\n",
        ));

        let probe = permission_row_probe(active, rows, |_| None);

        assert_eq!(probe.status, Status::Ok);
    }

    #[test]
    fn an_unreadable_permission_database_is_reported_as_not_checked() {
        let probe = permission_row_probe(
            Path::new("/Applications/CuaDriver.app/Contents/MacOS/cua-driver"),
            Err("macOS only lets apps with Full Disk Access read the permission database".into()),
            |_| None,
        );

        assert_eq!(probe.status, Status::Ok);
        assert!(probe
            .message
            .starts_with("not checked: macOS only lets apps"));
    }

    #[test]
    fn run_program_returns_stdout_only_for_a_zero_exit_within_the_timeout() {
        let ok = run_program("/bin/echo", &["hi"], Duration::from_secs(5));
        assert_eq!(ok.as_deref(), Some("hi\n"));
        assert_eq!(
            run_program("/usr/bin/false", &[], Duration::from_secs(5)),
            None
        );
        assert_eq!(
            run_program("/bin/sleep", &["5"], Duration::from_millis(100)),
            None
        );
    }
}
