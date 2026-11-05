use super::*;

#[test]
fn the_default_is_the_public_cache_for_linux_only() {
    let c = RegistryConfig::default();
    c.validate().unwrap();
    assert_eq!(c.for_guest(GuestOs::Linux), [DEFAULT_MIRROR.to_string()]);
    assert!(c.for_guest(GuestOs::Macos).is_empty());
}

#[test]
fn good_and_bad_mirrors() {
    for ok in [
        "https://mirror.gcr.io",
        "https://mirror.gcr.io/",
        "http://10.0.0.5:5000",
        "https://cache.corp.example:8443",
        "http://[fd00::1]:5000",
    ] {
        check_mirror(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    for bad in [
        "mirror.gcr.io",
        "ftp://mirror.gcr.io",
        "https://user:pw@mirror.gcr.io",
        "https://token@mirror.gcr.io",
        "https://mirror.gcr.io/v2/",
        "https://mirror.gcr.io?x=1",
        "https://",
        "https://host:notaport",
        "https://host:0",
        "https://mirror.gcr.io;rm -rf /",
        "https://a b.example",
        "https://$(id).example",
    ] {
        assert!(check_mirror(bad).is_err(), "{bad} must be refused");
    }
}

#[test]
fn validation_names_the_field_and_caps_the_list() {
    let c = RegistryConfig {
        mirrors: vec!["https://u:p@x.example".into()],
    };
    let e = c.validate().unwrap_err().to_string();
    assert!(
        e.contains("perf.registry.mirrors") && e.contains("credentials"),
        "{e}"
    );
    let c = RegistryConfig {
        mirrors: (0..=MAX_MIRRORS)
            .map(|i| format!("https://m{i}.example"))
            .collect(),
    };
    assert!(c.validate().is_err());
    // Off is an empty list.
    RegistryConfig { mirrors: vec![] }.validate().unwrap();
}

#[test]
fn toml_defaults_and_unknown_fields() {
    let c: RegistryConfig = toml::from_str("").unwrap();
    assert_eq!(c, RegistryConfig::default());
    let c: RegistryConfig = toml::from_str("mirrors = []").unwrap();
    assert!(c.mirrors.is_empty());
    assert!(toml::from_str::<RegistryConfig>("password = \"x\"").is_err());
}

#[test]
fn the_env_line_is_empty_without_mirrors_and_quoted_with_them() {
    assert_eq!(env_line(&[]), "");
    let l = env_line(&["https://a.example".into(), "http://b.example:5000".into()]);
    assert_eq!(
        l,
        "CUA_CI_REGISTRY_MIRRORS='https://a.example http://b.example:5000'\n"
    );
}

#[test]
fn the_hook_snippet_is_valid_sh_never_fails_and_keeps_other_settings() {
    let dir = tempfile::tempdir().unwrap();
    // Run the snippet's python against a temp daemon.json by swapping the
    // path: the logic (merge, keep keys, idempotent) is what is under test.
    let py = hook_snippet()
        .split("<<'PY'")
        .nth(1)
        .unwrap()
        .split_once('\n')
        .unwrap()
        .1
        .split("\nPY\n")
        .next()
        .unwrap()
        .replace(
            "/etc/docker/daemon.json",
            dir.path().join("daemon.json").to_str().unwrap(),
        )
        .replace(
            "/run/cua-ci-docker-reload",
            dir.path().join("reload").to_str().unwrap(),
        );
    let run = |mirrors: &str| {
        std::process::Command::new("python3")
            .arg("-c")
            .arg(&py)
            .env("MIRRORS", mirrors)
            .output()
            .unwrap()
    };
    std::fs::write(
        dir.path().join("daemon.json"),
        r#"{"log-driver":"json-file"}"#,
    )
    .unwrap();
    assert!(run("https://a.example https://b.example").status.success());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("daemon.json")).unwrap())
            .unwrap();
    assert_eq!(v["log-driver"], "json-file");
    assert_eq!(v["registry-mirrors"][1], "https://b.example");
    assert!(
        dir.path().join("reload").exists(),
        "a change asks for a reload"
    );
    std::fs::remove_file(dir.path().join("reload")).unwrap();
    // The same mirrors again change nothing and ask for nothing.
    assert!(run("https://a.example https://b.example").status.success());
    assert!(!dir.path().join("reload").exists());
    // A broken file is replaced, not fatal.
    std::fs::write(dir.path().join("daemon.json"), "{not json").unwrap();
    assert!(run("https://a.example").status.success());
    // The whole snippet parses as sh.
    let sh = format!("ms() {{ echo 0; }}\nD=/tmp\n{}", hook_snippet());
    let o = std::process::Command::new("sh")
        .arg("-n")
        .arg("-c")
        .arg(&sh)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}
