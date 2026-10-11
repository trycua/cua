//! Live, read-only Fleet template lookup for local runs
//! (`CUA_E2E_FLEET_LIVE=1` plus Fleet credentials, e.g.
//! `set -a; source ~/.env; set +a`).
//!
//! Only GETs: lists the account's namespaces and pools, resolves one pool's
//! template with `local_from_fleet_template`, and checks the plan matches
//! the template it came from. Set `CUA_E2E_FLEET_TEMPLATE` to pick the
//! pool/template by name. Nothing is created, claimed or booted.

use cua_fleet::{FleetClient, RuntimeKind};
use cua_sandbox_core::{Error, local_from_fleet_template};

#[tokio::test]
async fn live_pool_template_resolves_for_local_runs() {
    if std::env::var("CUA_E2E_FLEET_LIVE").as_deref() != Ok("1") {
        eprintln!("skipping: set CUA_E2E_FLEET_LIVE=1 (and Fleet credentials)");
        return;
    }
    let fleet = FleetClient::from_env().expect("Fleet credentials");
    let name = match std::env::var("CUA_E2E_FLEET_TEMPLATE") {
        Ok(n) if !n.is_empty() => n,
        _ => {
            // First namespace that holds a pool with a template.
            let namespaces = fleet
                .sdk()
                .list_namespaces()
                .await
                .expect("list namespaces");
            eprintln!("namespaces visible: {}", namespaces.len());
            let mut found = None;
            for ns in namespaces.iter().take(40) {
                if let Ok(pools) = fleet.sdk().list_pools(ns.name.clone()).await
                    && let Some(p) = pools.first()
                {
                    found = Some(p.metadata.name.clone());
                    break;
                }
            }
            match found {
                Some(n) => n,
                None => {
                    eprintln!("no pools visible to this account; nothing to resolve");
                    return;
                }
            }
        }
    };
    eprintln!("resolving Fleet pool/template {name:?}");
    match local_from_fleet_template(&fleet, &name).await {
        Ok(plan) => {
            eprintln!(
                "plan: template={} runtime={:?} image={} firmware={:?} cpus={:?} memory_mb={:?} services={:?} probes={:?}",
                plan.template,
                plan.runtime,
                plan.image,
                plan.firmware,
                plan.cpus,
                plan.memory_mb,
                plan.services,
                plan.probes
            );
            let (ns, tname) = plan.template.split_once('/').unwrap();
            let t = fleet
                .sdk()
                .get_template(ns.into(), tname.into())
                .await
                .unwrap();
            assert_eq!(
                plan.reference,
                t.spec.vm_template.container_disk_image.trim()
            );
            match t
                .spec
                .vm_template
                .runtime
                .clone()
                .unwrap_or(RuntimeKind::Kubevirt)
            {
                RuntimeKind::Kubevirt => assert!(plan.image.starts_with("vm:")),
                RuntimeKind::Gvisor => assert!(plan.image.starts_with("container:")),
                RuntimeKind::Macos => unreachable!("macOS templates are refused"),
            }
            let declared = t.spec.vm_template.services.clone().unwrap_or_default();
            assert_eq!(plan.services.len(), declared.len());
        }
        Err(Error::UnsupportedImage(m)) => {
            eprintln!("macOS template refused as designed: {m}");
        }
        Err(e) => panic!("resolving {name}: {e}"),
    }
    // A name that does not exist is NotFound (and still read-only).
    let e = local_from_fleet_template(&fleet, "cua-e2e-does-not-exist-7f3a")
        .await
        .unwrap_err();
    assert!(matches!(e, Error::NotFound(_)), "{e}");
}
