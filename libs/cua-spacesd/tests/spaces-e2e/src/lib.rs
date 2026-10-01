// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A real cua-spacesd server core, in-process on loopback, for the Spaces
//! suites. It lives outside libs/cua so the SDK never depends on the driver
//! workspace.
//!
//! Host safety: the server runs child processes as this user, so every test
//! Space gets `SystemService.Init(env = {HOME: <temp>, PATH: <temp bin>:/usr/bin:/bin:…})`
//! ([`Driver::confine`]) before anything else. Guest `$HOME`, Downloads, the
//! teleport import home and the data dir are all temp directories; the
//! teleport receiver uses a `FakeHost`; tool calls go to [`FakeTools`].
//! Nothing here reads or writes the real home directory, launches an app or
//! touches the keychain. Used by this crate's tests (the cua-spaces,
//! cua-daemon and SDK Spaces suites) and the `cua-test-fixtures` binary
//! behind the language smoke tests.

use async_trait::async_trait;
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The in-process driver's env token.
pub const TOKEN: &str = "cua-spaces-test-token";

/// The Spaces tool names, in order, from the contract
/// (`cua_spaces::contract::tools()`, checked in as
/// `libs/cua/spaces-contract/manifest.json`). The suites compare the tool
/// lists every surface serves to this, so adding a tool to the contract
/// needs no edit here.
pub fn contract_tool_names() -> Vec<String> {
    cua_spaces::contract::tools()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect()
}

/// A fake cua-driver registry (no platform effects).
pub struct FakeTools;

#[async_trait]
impl cua_driver_core::server::ToolProvider for FakeTools {
    fn tools_list(&self) -> Value {
        json!({
            "tools": [{
                "name": "get_screen_size",
                "description": "Fake screen size.\nSecond line.",
                "inputSchema": {"type": "object", "properties": {}},
                "annotations": {"readOnlyHint": true, "destructiveHint": false},
            }],
            "capability_version": "1",
        })
    }

    async fn invoke_tool(&self, name: &str, _arguments: Value) -> Result<Value, String> {
        match name {
            "get_screen_size" => Ok(json!({
                "content": [{"type": "text", "text": "1280x800"}],
                "structuredContent": {"width": 1280, "height": 800, "scale_factor": 1.0},
            })),
            other => Err(format!("unknown tool {other}")),
        }
    }
}

/// One in-process driver plus the temp directories behind it. Dropping it
/// removes the directories (the server task ends with the runtime).
pub struct Driver {
    /// `http://127.0.0.1:<port>`.
    pub url: String,
    /// Guest `$HOME` (set through Init by [`Driver::confine`]).
    pub home: PathBuf,
    /// The driver's Downloads (send_file lands here).
    pub downloads: PathBuf,
    /// Where teleport imports land.
    pub teleport_home: PathBuf,
    /// Prepended to the guest `PATH` (fake agent CLIs live here).
    pub bin: PathBuf,
    _dirs: Vec<tempfile::TempDir>,
}

/// Starts a driver on loopback with [`TOKEN`].
pub async fn driver() -> Driver {
    driver_with(|_| {}).await
}

/// [`driver`], with `configure` applied to its server config last.
pub async fn driver_with(configure: impl FnOnce(&mut ServerConfig)) -> Driver {
    let dirs: Vec<tempfile::TempDir> = (0..5).map(|_| tempfile::tempdir().unwrap()).collect();
    let canon = |d: &tempfile::TempDir| d.path().canonicalize().unwrap();
    let (data, downloads, teleport_home, home, bin) = (
        canon(&dirs[0]),
        canon(&dirs[1]),
        canon(&dirs[2]),
        canon(&dirs[3]),
        canon(&dirs[4]),
    );
    let mut config = ServerConfig {
        data_dir: data,
        downloads_dir: Some(downloads.clone()),
        teleport_home: Some(teleport_home.clone()),
        shutdown_grace: Duration::from_secs(1),
        ..ServerConfig::default()
    };
    configure(&mut config);
    let ctx = ServerContext::new(config, Some(TOKEN.into()));
    // The *receiver* is the spacesd under test (import side); its FakeHost
    // comes with the server crate.
    let receiver_host =
        Arc::new(cua_spacesd_teleport::FakeHost::new().with_home(teleport_home.clone()));
    let teleport = Arc::new(cua_spacesd_teleport::Receiver::with_host(
        teleport_home.clone(),
        receiver_host,
    ));
    let server = ServerBuilder::new(ctx)
        .tools(Arc::new(FakeTools))
        .teleport_receiver(teleport)
        .build();
    let addr = cua_spacesd_server::spawn_local(server).await.unwrap();
    Driver {
        url: format!("http://{addr}"),
        home,
        downloads,
        teleport_home,
        bin,
        _dirs: dirs,
    }
}

impl Driver {
    /// Points the guest's `$HOME` and `PATH` at temp directories through
    /// any connection to this driver.
    pub async fn confine_env(&self, env: &cua_spacesd_client::SpacesdClient) {
        // Only the fake-CLI dir and the base system: never the host's own
        // PATH, so a real agent CLI, npm or brew can never be found (or
        // installed into) by a test.
        let home = self.home.display().to_string();
        #[cfg(not(windows))]
        let vars = [
            ("HOME".to_string(), home),
            (
                "PATH".to_string(),
                format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.bin.display()),
            ),
        ];
        // Windows: cmd.exe reads the home from USERPROFILE and needs System32.
        #[cfg(windows)]
        let vars = {
            let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
            [
                ("HOME".to_string(), home.clone()),
                ("USERPROFILE".to_string(), home),
                (
                    "PATH".to_string(),
                    format!("{};{root}\\System32;{root}", self.bin.display()),
                ),
            ]
        };
        env.init(cua_spacesd_client::pb::InitRequest {
            token: TOKEN.into(),
            env: vars.into_iter().collect(),
            ..Default::default()
        })
        .await
        .unwrap();
    }

    /// [`Driver::confine_env`] through a Space, then checks the guest HOME.
    pub async fn confine(&self, space: &cua_spaces::Space) {
        self.confine_env(space.spacesd().unwrap()).await;
        let home = space.home().await.unwrap();
        assert_eq!(
            Path::new(&home),
            self.home,
            "guest HOME must be the temp dir"
        );
    }

    /// Connects and confines without a Space (fixtures).
    pub async fn confine_direct(&self) {
        let mut o = cua_spacesd_client::ConnectOptions::parse(&self.url).unwrap();
        o.token = Some(TOKEN.into());
        let env = cua_spacesd_client::SpacesdClient::connect(o).await.unwrap();
        self.confine_env(&env).await;
    }

    /// Writes an executable into the fake-CLI bin directory.
    pub fn install_fake(&self, name: &str, script: &str) {
        let p = self.bin.join(name);
        std::fs::write(&p, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

/// Writes a synthetic Firefox profile under `home` in the macOS, Linux and
/// Windows default layouts (`profiles.ini` + one profile holding `prefs.js`
/// with `cua.test.marker`, a fake `cookies.sqlite` and `places.sqlite`), so
/// the built-in Firefox provider over a `FakeHost` rooted at `home` finds it
/// on any host. Returns the last profile directory written.
pub fn write_firefox_profile(home: &Path) -> PathBuf {
    let roots = [
        home.join("Library/Application Support/Firefox"),
        home.join(".mozilla/firefox"),
        home.join("AppData/Roaming/Mozilla/Firefox"),
    ];
    let mut profile = PathBuf::new();
    for root in roots {
        profile = root.join("Profiles/cuatest.default-release");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(
            root.join("profiles.ini"),
            "[General]\nStartWithLastProfile=1\n\n[Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/cuatest.default-release\nDefault=1\n",
        )
        .unwrap();
        std::fs::write(
            profile.join("prefs.js"),
            "user_pref(\"cua.test.marker\", \"teleported\");\n",
        )
        .unwrap();
        std::fs::write(profile.join("cookies.sqlite"), b"not-a-real-db").unwrap();
        std::fs::write(profile.join("places.sqlite"), b"history").unwrap();
    }
    profile
}

/// Finds `name` under `dir` (bounded walk).
pub fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    let mut visited = 0;
    while let Some(d) = stack.pop() {
        visited += 1;
        assert!(visited < 10_000, "bounded walk");
        for e in std::fs::read_dir(&d).ok()?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == name) {
                return Some(p);
            }
        }
    }
    None
}
