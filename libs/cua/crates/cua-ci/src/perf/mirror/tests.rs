use super::*;

fn meta(private: bool, event: &str, head: Option<&str>) -> JobMeta {
    JobMeta {
        account: "acct".into(),
        repo: "acme/app".into(),
        private: Some(private),
        event: Some(event.into()),
        git_ref: Some("refs/heads/main".into()),
        head_repo: head.map(str::to_string),
        default_branch: Some("main".into()),
        fork: None,
        policy_cache: None,
    }
}

#[test]
fn public_repos_are_mirrored_without_a_credential() {
    let r = request_for(&meta(false, "push", None), &MirrorConfig::default(), None).unwrap();
    assert!(!r.private && r.credential_scope.is_none());
    // A fork PR on a public repo still gets the base repo's mirror.
    assert!(
        request_for(
            &meta(false, "pull_request", Some("evil/app")),
            &MirrorConfig::default(),
            None
        )
        .is_some()
    );
}

#[test]
fn private_repos_need_the_switch_a_credential_scope_and_trusted_code() {
    let on = MirrorConfig {
        enabled: true,
        private: true,
    };
    let off = MirrorConfig::default();
    assert!(
        request_for(&meta(true, "push", None), &off, Some("inst-1")).is_none(),
        "off by default"
    );
    assert!(
        request_for(&meta(true, "push", None), &on, None).is_none(),
        "no credential scope"
    );
    let r = request_for(&meta(true, "push", None), &on, Some("inst-1")).unwrap();
    assert!(r.private);
    assert_eq!(r.credential_scope.as_deref(), Some("inst-1"));
    assert!(
        request_for(
            &meta(true, "pull_request", Some("evil/app")),
            &on,
            Some("inst-1")
        )
        .is_none(),
        "fork PR"
    );
    assert!(
        request_for(
            &meta(true, "pull_request_target", None),
            &on,
            Some("inst-1")
        )
        .is_none()
    );
    let mut unknown = meta(true, "push", None);
    unknown.event = None;
    assert!(request_for(&unknown, &on, Some("inst-1")).is_none());
    // Unknown privacy counts as private.
    let mut m = meta(false, "push", None);
    m.private = None;
    assert!(request_for(&m, &off, None).is_none());
}

#[test]
fn nothing_when_disabled_or_the_repo_name_is_odd() {
    let mut cfg = MirrorConfig::default();
    cfg.enabled = false;
    assert!(request_for(&meta(false, "push", None), &cfg, None).is_none());
    for bad in [
        "", "noslash", "a/b/c", "../x", "a/..", "o/n;rm", "o/n n", "-/",
    ] {
        assert!(!valid_repo(bad), "{bad}");
    }
    for ok in ["acme/app", "a-b/c_d.e", "Org1/repo-2"] {
        assert!(valid_repo(ok), "{ok}");
    }
    let mut m = meta(false, "push", None);
    m.repo = "../etc".into();
    assert!(request_for(&m, &MirrorConfig::default(), None).is_none());
}

#[test]
fn folders_are_shared_for_public_and_scoped_for_private() {
    let pubreq = |acct: &str| MirrorRequest {
        repo: "Acme/App".into(),
        account: acct.into(),
        private: false,
        credential_scope: None,
    };
    assert_eq!(
        folder_key(&pubreq("a")),
        folder_key(&pubreq("b")),
        "public: shared"
    );
    let priv_ = |acct: &str, cred: &str| MirrorRequest {
        repo: "acme/app".into(),
        account: acct.into(),
        private: true,
        credential_scope: Some(cred.into()),
    };
    let k = folder_key(&priv_("a", "i1"));
    assert_ne!(k, folder_key(&priv_("b", "i1")), "another account");
    assert_ne!(k, folder_key(&priv_("a", "i2")), "another credential");
    assert_ne!(k, folder_key(&pubreq("a")), "never the public folder");
    assert!(k.starts_with("private-") && folder_key(&pubreq("a")).starts_with("public-"));
}

#[tokio::test]
async fn ensure_refuses_what_it_must_not_fetch() {
    let d = tempfile::tempdir().unwrap();
    let m = GitMirrors::new(d.path(), "github.com");
    let private = MirrorRequest {
        repo: "acme/app".into(),
        account: "a".into(),
        private: true,
        credential_scope: Some("i".into()),
    };
    assert!(
        m.ensure(&private, None).await.is_none(),
        "private without a credential"
    );
    let bad = MirrorRequest {
        repo: "../x".into(),
        account: "a".into(),
        private: false,
        credential_scope: None,
    };
    assert!(m.ensure(&bad, None).await.is_none());
    assert!(
        std::fs::read_dir(d.path()).unwrap().next().is_none(),
        "nothing was started"
    );
}

/// A fill against a local bare repository standing in for GitHub: the
/// mirror becomes ready in the background and a second ensure returns it.
#[cfg(unix)]
#[tokio::test]
async fn a_mirror_fills_in_the_background_and_then_serves() {
    use std::process::Command;
    let d = tempfile::tempdir().unwrap();
    let work = d.path().join("w");
    let run = |args: &[&str], cwd: &Path| {
        let st = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap();
        assert!(st.success(), "{args:?}");
    };
    std::fs::create_dir_all(&work).unwrap();
    run(&["init", "-q", "-b", "main"], &work);
    std::fs::write(work.join("f"), b"x").unwrap();
    run(&["add", "f"], &work);
    run(&["commit", "-qm", "c"], &work);
    let upstream = d.path().join("up.git");
    run(
        &[
            "clone",
            "-q",
            "--bare",
            work.to_str().unwrap(),
            upstream.to_str().unwrap(),
        ],
        d.path(),
    );

    let folder = d.path().join("mirror-folder");
    fill_or_refresh(&folder, upstream.to_str().unwrap(), None)
        .await
        .unwrap();
    assert!(GitMirrors::ready(&folder));
    // A second pass is a refresh and keeps it ready.
    fill_or_refresh(&folder, upstream.to_str().unwrap(), None)
        .await
        .unwrap();
    assert!(folder.join(REPO_DIR).join("objects").is_dir());
    assert!(!folder.join(".filling").exists());
    // The alternates trick: a new repo with the mirror's objects as an
    // alternate has the commit without fetching it.
    let head = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&work)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let fresh = d.path().join("fresh");
    std::fs::create_dir_all(&fresh).unwrap();
    run(&["init", "-q"], &fresh);
    let st = Command::new("git")
        .args(["cat-file", "-e", head.trim()])
        .current_dir(&fresh)
        .env(
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            folder.join(REPO_DIR).join("objects"),
        )
        .status()
        .unwrap();
    assert!(st.success(), "the object is found through the alternate");
}
