//! Network settings of Spaces: kept with the Space ([`SpaceMeta::network`]),
//! carried to the runtime that starts it, and compiled there to rules the
//! host enforces outside the guest ([`cua_vmm::network`] has the model:
//! presets, layers, the support table and the fail-closed plan).
//!
//! - A new Space gets what its create asks for: [`SpaceCreate::network_settings`],
//!   else what the old options say (`network=none` is `none`, an egress
//!   policy is `custom` from `internet`, nothing is `internet`).
//! - A child Space (`parent`) gets the intersection of its own settings and
//!   its parent's: it can only tighten. `widen` skips that and records who
//!   widened it.
//! - A Space from before network settings gets them, migrated the same
//!   way, on its next start ([`migrate_meta`]), and a one-time notice.
//! - Forks keep their source's settings and tree
//!   ([`crate::registry::Registry::inherit_meta`]).
//!
//! [`SpaceMeta::network`]: crate::registry::SpaceMeta::network
//! [`SpaceCreate::network_settings`]: crate::SpaceCreate::network_settings

use crate::registry::SpaceMeta;
use crate::{Error, Result, Spaces};
use cua_proto::daemon::v1 as dpb;
use cua_proto::env::v1 as epb;

pub use cua_vmm::network::{
    Mode, NO_NETWORK_NOTE, NetworkContext, NetworkPlan, NetworkPreset, NetworkSettings,
    NetworkSupport, Placement, UNPROTECTED_NOTE, network_support,
};

/// What a Space's listing shows of its network.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SpaceNetwork {
    /// Its settings.
    pub settings: NetworkSettings,
    /// Why its network is not what was asked, in plain words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Its settings were changed by an update; say so once.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notice_pending: bool,
}

/// The one-time notice of a Space moved to the new defaults.
pub const MIGRATION_NOTICE: &str = "This Space now uses Internet only. It can't reach this \
     computer or your local network. Change it with `cua network set`.";

/// The settings of a Space from before network settings
/// ([`NetworkSettings::migrate`] of its egress policy), and whether they
/// open anything its old policy's lists named (kept, and labelled by
/// [`NetworkSettings::widenings`]). `restrict_network`: it was created with
/// `network=none`.
pub fn migrate_meta(meta: &SpaceMeta, restrict_network: bool) -> NetworkSettings {
    NetworkSettings::migrate(&meta.egress, restrict_network)
}

/// `settings` in the daemon's wire form.
pub fn to_pb(s: &NetworkSettings) -> dpb::NetworkSettings {
    dpb::NetworkSettings {
        preset: s.preset.as_str().into(),
        localhost_ports: s.localhost_ports.iter().map(|p| u32::from(*p)).collect(),
        all_localhost_ports: s.all_localhost_ports,
        allow: s.custom.allow.clone(),
        deny: s.custom.deny.clone(),
        custom_base: match s.custom_base {
            NetworkPreset::None => "none".into(),
            _ => String::new(),
        },
        widened_by: s.widened_by.clone().unwrap_or_default(),
    }
}

/// The daemon's wire form back, checked ([`NetworkSettings::validate`]).
pub fn from_pb(p: &dpb::NetworkSettings) -> Result<NetworkSettings> {
    settings_from_parts(
        &p.preset,
        &p.localhost_ports,
        p.all_localhost_ports,
        &p.allow,
        &p.deny,
        &p.custom_base,
        &p.widened_by,
    )
}

/// `settings` as a host's create carries them.
pub fn to_host_pb(s: &NetworkSettings) -> epb::HostNetworkSettings {
    let p = to_pb(s);
    epb::HostNetworkSettings {
        preset: p.preset,
        localhost_ports: p.localhost_ports,
        all_localhost_ports: p.all_localhost_ports,
        allow: p.allow,
        deny: p.deny,
        custom_base: p.custom_base,
        widened_by: p.widened_by,
    }
}

/// A host's create's settings back, checked.
pub fn from_host_pb(p: &epb::HostNetworkSettings) -> Result<NetworkSettings> {
    settings_from_parts(
        &p.preset,
        &p.localhost_ports,
        p.all_localhost_ports,
        &p.allow,
        &p.deny,
        &p.custom_base,
        &p.widened_by,
    )
}

fn settings_from_parts(
    preset: &str,
    ports: &[u32],
    all_ports: bool,
    allow: &[String],
    deny: &[String],
    base: &str,
    widened_by: &str,
) -> Result<NetworkSettings> {
    let word = |w: &str| -> Result<NetworkPreset> {
        if w.trim().is_empty() {
            return Ok(NetworkPreset::Internet);
        }
        NetworkPreset::parse(w).map_err(|e| Error::invalid(e.0))
    };
    let localhost_ports = ports
        .iter()
        .map(|p| {
            u16::try_from(*p)
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| Error::invalid(format!("localhost port {p} is not a port")))
        })
        .collect::<Result<Vec<u16>>>()?;
    let s = NetworkSettings {
        preset: word(preset)?,
        localhost_ports,
        all_localhost_ports: all_ports,
        custom: crate::egress::EgressPolicy {
            allow: allow.to_vec(),
            deny: deny.to_vec(),
        },
        custom_base: word(base)?,
        widened_by: (!widened_by.is_empty()).then(|| widened_by.to_string()),
    };
    s.validate().map_err(|e| Error::invalid(e.0))?;
    Ok(s)
}

/// Who widened a child and when: `user@2026-10-26T12:00:00Z`.
pub fn widened_stamp() -> String {
    let who = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| "user".into());
    format!(
        "{who}@{}",
        humantime::format_rfc3339_seconds(std::time::SystemTime::now())
    )
}

/// The network a create resolved: the settings the Space gets and its
/// place in a tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CreateNetwork {
    /// What the Space gets.
    pub settings: NetworkSettings,
    /// Its parent's canonical id.
    pub parent: Option<String>,
    /// Its tree's root's canonical id.
    pub root: Option<String>,
}

/// The settings a parent Space has: its own, or what it would migrate to.
pub(crate) fn settings_of(meta: Option<&SpaceMeta>) -> NetworkSettings {
    match meta {
        Some(m) => m.network.clone().unwrap_or_else(|| migrate_meta(m, false)),
        None => NetworkSettings::internet(),
    }
}

/// [`CreateNetwork`] from what a create asks (`asked`), the parent's id
/// and record (if any) and `widen`.
pub(crate) fn resolve_create(
    asked: NetworkSettings,
    parent: Option<(String, Option<SpaceMeta>)>,
    widen: bool,
) -> Result<CreateNetwork> {
    asked.validate().map_err(|e| Error::invalid(e.0))?;
    let Some((parent_id, parent_meta)) = parent else {
        if widen {
            return Err(Error::invalid(
                "widen goes with parent: only a child Space can be allowed more than its parent",
            ));
        }
        return Ok(CreateNetwork {
            settings: asked,
            parent: None,
            root: None,
        });
    };
    let root = parent_meta
        .as_ref()
        .and_then(|m| m.root.clone())
        .unwrap_or_else(|| parent_id.clone());
    let theirs = settings_of(parent_meta.as_ref());
    let settings = if widen {
        NetworkSettings {
            widened_by: Some(widened_stamp()),
            ..asked
        }
    } else {
        asked.intersect(&theirs)
    };
    Ok(CreateNetwork {
        settings,
        parent: Some(parent_id),
        root: Some(root),
    })
}

impl Spaces {
    /// The network a create asks for, intersected with its parent's.
    pub(crate) fn create_network(&self, opts: &crate::SpaceCreate) -> Result<CreateNetwork> {
        let asked = opts.network_settings.clone().unwrap_or_else(|| {
            NetworkSettings::from_legacy(
                &opts.egress,
                opts.network == cua_sandbox_core::NetworkMode::None,
            )
        });
        let parent = match opts.parent.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(p) => {
                let id = self.resolve(p)?.to_string();
                if self.inner.registry.get(&id)?.is_none()
                    && self.inner.registry.meta(&id).is_none()
                {
                    return Err(Error::NotFound(format!("parent {p}")));
                }
                let meta = self.inner.registry.meta(&id);
                Some((id, meta))
            }
        };
        resolve_create(asked, parent, opts.widen)
    }

    /// Records what a create resolved on the new Space.
    pub(crate) fn record_network(&self, id: &str, net: &CreateNetwork) -> Result<()> {
        self.inner.registry.update_meta(id, |m| {
            m.network = Some(net.settings.clone());
            m.network_generation = m.network_generation.saturating_add(1).max(1);
            m.parent = net.parent.clone();
            m.root = net.root.clone();
        })
    }

    /// A Space's network: its settings, a note on why it differs from
    /// them, and whether an update changed them. `None` for a Space with
    /// no record here.
    pub fn network(&self, space: &str) -> Result<Option<SpaceNetwork>> {
        let id = self.resolve(space)?.to_string();
        Ok(self.inner.registry.meta(&id).map(|m| SpaceNetwork {
            settings: settings_of(Some(&m)),
            note: m.network_note.clone(),
            notice_pending: m.network_notice_pending || m.network.is_none(),
        }))
    }

    /// Moves a Space from before network settings to them (at its next
    /// start): its old egress policy becomes `custom`, nothing becomes
    /// `internet`, and a one-time notice is left. A Space that has them is
    /// left as it is. Returns its settings.
    pub(crate) fn migrate_network(&self, id: &str) -> Result<NetworkSettings> {
        let Some(meta) = self.inner.registry.meta(id) else {
            // Nothing recorded: nothing it was created with but the
            // default.
            let s = NetworkSettings::internet();
            self.inner.registry.update_meta(id, |m| {
                m.network = Some(s.clone());
                m.network_generation = 1;
                m.network_notice_pending = true;
            })?;
            return Ok(s);
        };
        if let Some(s) = meta.network {
            return Ok(s);
        }
        let s = migrate_meta(&meta, false);
        self.inner.registry.update_meta(id, |m| {
            m.network = Some(s.clone());
            m.network_generation = m.network_generation.saturating_add(1);
            m.network_notice_pending = true;
        })?;
        tracing::info!(space = %id, preset = %s.preset, "moved a Space to network settings");
        Ok(s)
    }

    /// Clears a Space's one-time notice (the CLI or app showed it).
    pub fn ack_network_notice(&self, space: &str) -> Result<()> {
        let id = self.resolve(space)?.to_string();
        self.inner
            .registry
            .update_meta(&id, |m| m.network_notice_pending = false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(allow: &[&str], deny: &[&str]) -> SpaceMeta {
        SpaceMeta {
            egress: crate::egress::EgressPolicy {
                allow: allow.iter().map(|s| s.to_string()).collect(),
                deny: deny.iter().map(|s| s.to_string()).collect(),
            },
            ..Default::default()
        }
    }

    /// Old `space-meta.json` entries become the expected settings.
    #[test]
    fn migration_fixtures() {
        let old: std::collections::BTreeMap<String, SpaceMeta> = serde_json::from_str(
            r#"{
                "local:plain": {"labels": {"team": "a"}, "created_at_unix": 5},
                "local:policy": {"egress": {"allow": ["192.168.1.0/24"], "deny": ["all"]}},
                "local:allow-only": {"egress": {"allow": ["10.0.0.5"]}}
            }"#,
        )
        .unwrap();
        for m in old.values() {
            assert!(m.network.is_none(), "from before network settings");
        }
        assert_eq!(
            migrate_meta(&old["local:plain"], false),
            NetworkSettings::internet()
        );
        assert_eq!(
            migrate_meta(&old["local:plain"], true),
            NetworkSettings::none()
        );
        let p = migrate_meta(&old["local:policy"], false);
        assert_eq!(p.preset, NetworkPreset::Custom);
        assert_eq!(p.custom.allow, ["192.168.1.0/24"]);
        assert_eq!(p.custom.deny, ["all"]);
        assert_eq!(
            p.widenings(),
            ["Allows your local network (192.168.1.0/24)"]
        );
        let a = migrate_meta(&old["local:allow-only"], false);
        assert_eq!(a.widenings(), ["Allows your local network (10.0.0.5/32)"]);
        // A new file round-trips with its settings.
        let mut m = old["local:plain"].clone();
        m.network = Some(NetworkSettings::none());
        m.network_generation = 1;
        let back: SpaceMeta = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn wire_forms_round_trip_and_are_checked() {
        let s = NetworkSettings {
            preset: NetworkPreset::Localhost,
            localhost_ports: vec![5432, 3000],
            custom: crate::egress::EgressPolicy {
                allow: vec!["1.1.1.1".into()],
                deny: vec!["8.8.8.8".into()],
            },
            widened_by: Some("me@now".into()),
            ..Default::default()
        };
        assert_eq!(from_pb(&to_pb(&s)).unwrap(), s);
        assert_eq!(from_host_pb(&to_host_pb(&s)).unwrap(), s);
        let none_base = NetworkSettings {
            preset: NetworkPreset::Custom,
            custom_base: NetworkPreset::None,
            ..Default::default()
        };
        assert_eq!(to_pb(&none_base).custom_base, "none");
        assert_eq!(from_pb(&to_pb(&none_base)).unwrap(), none_base);
        // Empty is internet.
        assert_eq!(
            from_pb(&dpb::NetworkSettings::default()).unwrap(),
            NetworkSettings::internet()
        );
        let bad = |p: dpb::NetworkSettings| from_pb(&p).unwrap_err().tag();
        assert_eq!(
            bad(dpb::NetworkSettings {
                preset: "open".into(),
                ..Default::default()
            }),
            "invalid_argument"
        );
        assert_eq!(
            bad(dpb::NetworkSettings {
                preset: "localhost".into(),
                localhost_ports: vec![70000],
                ..Default::default()
            }),
            "invalid_argument"
        );
        assert_eq!(
            bad(dpb::NetworkSettings {
                preset: "localhost".into(),
                ..Default::default()
            }),
            "invalid_argument"
        );
        assert_eq!(
            bad(dpb::NetworkSettings {
                deny: vec!["example.com".into()],
                ..Default::default()
            }),
            "invalid_argument"
        );
    }

    #[test]
    fn a_child_gets_the_intersection_unless_widened() {
        let parent = SpaceMeta {
            network: Some(NetworkSettings::none()),
            ..Default::default()
        };
        let c = resolve_create(
            NetworkSettings::preset(NetworkPreset::Lan),
            Some(("local:p".into(), Some(parent.clone()))),
            false,
        )
        .unwrap();
        assert_eq!(c.settings, NetworkSettings::none());
        assert_eq!(c.parent.as_deref(), Some("local:p"));
        assert_eq!(c.root.as_deref(), Some("local:p"));
        // A grandchild keeps the root.
        let child = SpaceMeta {
            network: Some(NetworkSettings::internet()),
            parent: Some("local:p".into()),
            root: Some("local:p".into()),
            ..Default::default()
        };
        let g = resolve_create(
            NetworkSettings::internet(),
            Some(("local:c".into(), Some(child))),
            false,
        )
        .unwrap();
        assert_eq!(g.root.as_deref(), Some("local:p"));
        // Widened: what it asked, stamped.
        let w = resolve_create(
            NetworkSettings::preset(NetworkPreset::Lan),
            Some(("local:p".into(), Some(parent))),
            true,
        )
        .unwrap();
        assert_eq!(w.settings.preset, NetworkPreset::Lan);
        assert!(
            w.settings
                .widened_by
                .as_deref()
                .is_some_and(|s| s.contains('@'))
        );
        // A parent from before network settings counts as its migration.
        let old = meta(&[], &["all"]);
        let c = resolve_create(
            NetworkSettings::internet(),
            Some(("local:old".into(), Some(old))),
            false,
        )
        .unwrap();
        assert!(c.settings.custom.deny.contains(&"all".to_string()));
        // No parent: as asked; widen alone is refused.
        assert_eq!(
            resolve_create(NetworkSettings::none(), None, false)
                .unwrap()
                .settings,
            NetworkSettings::none()
        );
        assert_eq!(
            resolve_create(NetworkSettings::none(), None, true)
                .unwrap_err()
                .tag(),
            "invalid_argument"
        );
    }
}
