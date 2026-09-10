#[cfg(unix)]
mod unix {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::process::Command;

    use tempfile::tempdir;

    #[test]
    fn status_and_uninstall_use_the_selected_hermes_home() {
        let root = tempdir().expect("temporary test root");
        let driver_home = root.path().join("driver");
        let hermes_home = root.path().join("hermes-profile");
        let hermes_skills = hermes_home.join("skills");
        let local_skill = driver_home.join("skills").join("cua-driver");
        let hermes_link = hermes_skills.join("cua-driver");

        fs::create_dir_all(&hermes_skills).expect("create Hermes skills directory");
        fs::create_dir_all(&local_skill).expect("create local skill directory");
        fs::write(local_skill.join("SKILL.md"), "# test\n").expect("write test skill");
        symlink(&local_skill, &hermes_link).expect("link test skill into Hermes");

        let status = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
            .args(["skills", "status"])
            .env("CUA_DRIVER_RS_HOME", &driver_home)
            .env("HERMES_HOME", &hermes_home)
            .env("HOME", root.path().join("unused-home"))
            .output()
            .expect("run skills status");
        assert!(status.status.success(), "{status:?}");
        let stdout = String::from_utf8(status.stdout).expect("UTF-8 status output");
        assert!(
            stdout.contains(&hermes_link.display().to_string()),
            "{stdout}"
        );
        assert!(stdout.contains("Hermes"), "{stdout}");
        assert!(stdout.contains("linked"), "{stdout}");

        let uninstall = Command::new(env!("CARGO_BIN_EXE_cua-driver"))
            .args(["skills", "uninstall"])
            .env("CUA_DRIVER_RS_HOME", &driver_home)
            .env("HERMES_HOME", &hermes_home)
            .env("HOME", root.path().join("unused-home"))
            .output()
            .expect("run skills uninstall");
        assert!(uninstall.status.success(), "{uninstall:?}");
        let stdout = String::from_utf8(uninstall.stdout).expect("UTF-8 uninstall output");
        assert!(
            stdout.contains(&hermes_link.display().to_string()),
            "{stdout}"
        );
        assert!(!hermes_link.exists());
        assert!(hermes_link.symlink_metadata().is_err());
        assert!(
            local_skill.exists(),
            "uninstall without --all keeps local skill"
        );
    }
}
