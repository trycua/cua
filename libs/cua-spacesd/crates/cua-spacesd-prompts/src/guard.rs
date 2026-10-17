// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The host guard and the password source.

use serde::Serialize;

/// A password that never prints.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// Wraps a password.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The password, for typing into a dialog.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Where answering is allowed, and with what.
#[derive(Debug, Clone)]
pub struct Guard {
    /// `hw.model` of this machine.
    pub hw_model: String,
    /// The account this process runs as.
    pub user: String,
    /// The guest password, when one is provisioned.
    pub password: Option<Secret>,
    /// Where the password came from (never its value).
    pub password_source: &'static str,
}

/// Why answering is refused, or what it would use.
#[derive(Debug, Clone, Serialize)]
pub struct GuardReport {
    /// This machine is an Apple Virtual Machine.
    pub virtual_machine: bool,
    /// The account.
    pub user: String,
    /// `env:<NAME>`, `default:lume`, or `none`.
    pub password_source: &'static str,
}

/// Env names, in priority order.
const PASSWORD_ENV: &[&str] = &[
    "CUA_SPACESD_PROMPT_PASSWORD",
    "CUA_GUEST_PASSWORD",
    "CUA_SUDO_PW",
];

#[cfg(target_os = "macos")]
fn hw_model() -> String {
    let name = b"hw.model\0";
    let mut len: libc::size_t = 0;
    unsafe {
        if libc::sysctlbyname(
            name.as_ptr().cast(),
            std::ptr::null_mut(),
            &mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
            || len == 0
        {
            return String::new();
        }
        let mut buf = vec![0u8; len];
        if libc::sysctlbyname(
            name.as_ptr().cast(),
            buf.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return String::new();
        }
        buf.truncate(len);
        String::from_utf8_lossy(&buf)
            .trim_end_matches('\0')
            .trim()
            .to_owned()
    }
}

#[cfg(not(target_os = "macos"))]
fn hw_model() -> String {
    String::new()
}

fn current_user() -> String {
    std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_default()
}

/// Pure decision: is `hw_model` a Virtual Machine, and which password.
pub(crate) fn decide(
    hw_model: String,
    user: String,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Guard, String> {
    if !(hw_model.contains("VirtualMac") || hw_model.contains("Apple Virtual Machine")) {
        return Err(format!(
            "refusing to answer security prompts: hw.model={hw_model:?} is not an Apple \
             Virtual Machine. This feature types the guest password into system dialogs and \
             is only safe inside a disposable Space VM."
        ));
    }
    let (password, password_source) = PASSWORD_ENV
        .iter()
        .find_map(|name| {
            env(name)
                .filter(|v| !v.is_empty())
                .map(|v| (Some(Secret::new(v)), source_name(name)))
        })
        .unwrap_or_else(|| {
            // The images' contract: account `lume`, password `lume`.
            if user == "lume" {
                (Some(Secret::new("lume")), "default:lume")
            } else {
                (None, "none")
            }
        });
    Ok(Guard {
        hw_model,
        user,
        password,
        password_source,
    })
}

fn source_name(name: &str) -> &'static str {
    match name {
        "CUA_SPACESD_PROMPT_PASSWORD" => "env:CUA_SPACESD_PROMPT_PASSWORD",
        "CUA_GUEST_PASSWORD" => "env:CUA_GUEST_PASSWORD",
        _ => "env:CUA_SUDO_PW",
    }
}

/// Checks this machine. `Err` carries the reason answering is refused.
pub fn guard() -> Result<Guard, String> {
    decide(hw_model(), current_user(), |n| std::env::var(n).ok())
}

impl Guard {
    /// The reportable part.
    pub fn report(&self) -> GuardReport {
        GuardReport {
            virtual_machine: true,
            user: self.user.clone(),
            password_source: self.password_source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn refuses_real_hardware() {
        let err = decide("MacBookPro18,3".into(), "lume".into(), no_env).unwrap_err();
        assert!(err.contains("not an Apple Virtual Machine"));
    }

    #[test]
    fn lume_account_defaults_to_the_image_password() {
        let g = decide("VirtualMac2,1".into(), "lume".into(), no_env).unwrap();
        assert_eq!(g.password.unwrap().expose(), "lume");
        assert_eq!(g.password_source, "default:lume");
    }

    #[test]
    fn other_accounts_need_an_explicit_password() {
        let g = decide("VirtualMac2,1".into(), "alice".into(), no_env).unwrap();
        assert!(g.password.is_none());
        let g = decide("VirtualMac2,1".into(), "alice".into(), |n| {
            (n == "CUA_SUDO_PW").then(|| "pw".to_owned())
        })
        .unwrap();
        assert_eq!(g.password.unwrap().expose(), "pw");
        assert_eq!(g.password_source, "env:CUA_SUDO_PW");
    }

    #[test]
    fn secret_never_prints() {
        assert_eq!(
            format!("{:?}", Secret::new("hunter2")),
            "Secret(<redacted>)"
        );
    }
}
