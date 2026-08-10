//! Testable authorization policy for the Windows UIAccess worker.
//!
//! The production binary carries a `uiAccess=true` manifest and therefore
//! cannot be launched by an unelevated test runner. Keeping the pure policy in
//! this library lets CI execute the security checks without weakening the
//! production manifest.

#[cfg(target_os = "windows")]
pub fn authorized_parent_pid_from<I>(args: I) -> anyhow::Result<u32>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--authorized-parent-pid" {
            let value = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("--authorized-parent-pid requires a value"))?;
            let pid = value.parse::<u32>().map_err(|_| {
                anyhow::anyhow!("--authorized-parent-pid must be a positive process id")
            })?;
            if pid == 0 {
                anyhow::bail!("--authorized-parent-pid must be a positive process id");
            }
            return Ok(pid);
        }
    }
    anyhow::bail!(
        "missing --authorized-parent-pid; the UIAccess worker may only be launched by cua-driver serve"
    )
}

#[cfg(target_os = "windows")]
pub fn client_identity_is_authorized(
    client_identity: Option<(u32, &str)>,
    authorized_parent_pid: u32,
    owner_sid: &str,
) -> bool {
    client_identity == Some((authorized_parent_pid, owner_sid))
}

#[cfg(all(test, target_os = "windows"))]
mod authorization_tests {
    use super::*;

    #[test]
    fn launch_requires_explicit_parent_pid() {
        assert!(authorized_parent_pid_from(Vec::<String>::new()).is_err());
        assert!(
            authorized_parent_pid_from(vec!["--authorized-parent-pid".into(), "0".into()]).is_err()
        );
        assert_eq!(
            authorized_parent_pid_from(vec!["--authorized-parent-pid".into(), "4242".into()])
                .unwrap(),
            4242
        );
    }

    #[test]
    fn client_must_match_both_exact_parent_and_owner_sid() {
        assert!(client_identity_is_authorized(
            Some((4242, "S-1-5-21-123")),
            4242,
            "S-1-5-21-123"
        ));
        assert!(!client_identity_is_authorized(
            Some((4243, "S-1-5-21-123")),
            4242,
            "S-1-5-21-123"
        ));
        assert!(!client_identity_is_authorized(
            Some((4242, "S-1-5-21-999")),
            4242,
            "S-1-5-21-123"
        ));
        assert!(!client_identity_is_authorized(None, 4242, "S-1-5-21-123"));
    }
}
