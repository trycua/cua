// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The image's `User` (`name`, `uid`, `name:group`, `uid:gid`) resolved
//! against the image's own `/etc/passwd` and `/etc/group`, like the
//! container engines do.

/// A resolved user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// User id.
    pub uid: u32,
    /// Primary group id.
    pub gid: u32,
    /// The login name, when `/etc/passwd` has one (its supplementary groups
    /// come from `initgroups`).
    pub name: Option<String>,
    /// Home directory from `/etc/passwd`, when it has the user.
    pub home: Option<String>,
}

impl Identity {
    /// Whether this is root (no privileges to drop).
    pub fn is_root(&self) -> bool {
        self.uid == 0 && self.gid == 0
    }
}

fn passwd_entry<'a>(passwd: &'a str, who: &str) -> Option<Vec<&'a str>> {
    passwd.lines().find_map(|line| {
        let f: Vec<&str> = line.split(':').collect();
        (f.len() >= 6 && !line.starts_with('#') && (f[0] == who || f[2] == who)).then_some(f)
    })
}

fn group_id(group: &str, who: &str) -> Option<u32> {
    if let Ok(n) = who.parse::<u32>() {
        return Some(n);
    }
    group.lines().find_map(|line| {
        let f: Vec<&str> = line.split(':').collect();
        (f.len() >= 3 && f[0] == who)
            .then(|| f[2].parse().ok())
            .flatten()
    })
}

/// Resolves `spec` the way `docker run --user` does: a name is looked up in
/// `passwd`; a number is a uid whose passwd entry (if any) supplies the
/// primary group and home, else the gid is the uid's; a `:group` part (name
/// or number) overrides the primary group.
pub fn resolve(spec: &str, passwd: &str, group: &str) -> Result<Identity, String> {
    let spec = spec.trim();
    let (user, grp) = match spec.split_once(':') {
        Some((u, g)) => (u, Some(g)),
        None => (spec, None),
    };
    if user.is_empty() {
        return Err(format!("empty user in {spec:?}"));
    }
    let entry = passwd_entry(passwd, user);
    let (uid, mut gid, name, home) = match (&entry, user.parse::<u32>()) {
        (Some(f), _) => {
            let uid: u32 = f[2]
                .parse()
                .map_err(|_| format!("bad uid {:?} in /etc/passwd for {user}", f[2]))?;
            let gid: u32 = f[3]
                .parse()
                .map_err(|_| format!("bad gid {:?} in /etc/passwd for {user}", f[3]))?;
            (
                uid,
                gid,
                Some(f[0].to_string()),
                Some(f[5].to_string()).filter(|h| !h.is_empty()),
            )
        }
        (None, Ok(uid)) => (uid, uid, None, None),
        (None, Err(_)) => return Err(format!("no user {user:?} in the image's /etc/passwd")),
    };
    if let Some(g) = grp {
        gid = group_id(group, g)
            .ok_or_else(|| format!("no group {g:?} in the image's /etc/group"))?;
    }
    Ok(Identity {
        uid,
        gid,
        name,
        home,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0:root:/root:/bin/sh\n\
        node:x:1000:1000::/home/node:/bin/bash\n\
        nobody:x:65534:65534:nobody:/nonexistent:/sbin/nologin\n";
    const GROUP: &str = "root:x:0:\nnode:x:1000:\nstaff:x:50:node\n";

    #[test]
    fn names_resolve_through_passwd() {
        let i = resolve("node", PASSWD, GROUP).unwrap();
        assert_eq!(
            i,
            Identity {
                uid: 1000,
                gid: 1000,
                name: Some("node".into()),
                home: Some("/home/node".into())
            }
        );
        assert!(!i.is_root());
        assert!(resolve("root", PASSWD, GROUP).unwrap().is_root());
    }

    #[test]
    fn a_uid_takes_its_passwd_entry_or_stands_alone() {
        let i = resolve("1000", PASSWD, GROUP).unwrap();
        assert_eq!(i.name.as_deref(), Some("node"));
        assert_eq!(i.gid, 1000);
        let lone = resolve("4242", PASSWD, GROUP).unwrap();
        assert_eq!(
            (lone.uid, lone.gid, lone.name, lone.home),
            (4242, 4242, None, None)
        );
    }

    #[test]
    fn a_group_part_overrides_the_primary_group() {
        assert_eq!(resolve("node:staff", PASSWD, GROUP).unwrap().gid, 50);
        assert_eq!(resolve("node:7", PASSWD, GROUP).unwrap().gid, 7);
        assert_eq!(resolve("4242:staff", PASSWD, GROUP).unwrap().gid, 50);
    }

    #[test]
    fn unknown_names_are_errors() {
        assert!(resolve("ghost", PASSWD, GROUP)
            .unwrap_err()
            .contains("ghost"));
        assert!(resolve("node:ghosts", PASSWD, GROUP)
            .unwrap_err()
            .contains("ghosts"));
        assert!(resolve("", PASSWD, GROUP).is_err());
    }
}
