//! Live Fleet e2e, gated: needs `CUA_E2E_FLEET=1`, Fleet credentials
//! (`CUA_CLIENT_ID`/`CUA_CLIENT_SECRET` or `CUA_TOKEN`), and
//! `CUA_E2E_FLEET_IMAGE`: a **publicly pullable** image that ships
//! cua-spacesd (the plain `cua-ubuntu-24.04` image does not, so it cannot
//! be a Space). `CUA_E2E_FLEET_RUNTIME` picks `kubevirt` (containerDisk tag)
//! or `gvisor` (`docker-` tag); the pairing is validated before claiming.
//!
//! Resources are named `cua-e2e-spaces-*` and released even when an
//! assertion fails; the claim is checked gone afterwards.
//!
//! ```sh
//! set -a; source ~/.env; set +a
//! CUA_E2E_FLEET=1 CUA_E2E_FLEET_IMAGE=ghcr.io/trycua/linux:24.04 \
//!   CUA_E2E_FLEET_RUNTIME=gvisor \
//!   cargo test -p cua-spaces --test e2e_fleet -- --nocapture
//! ```

use cua_sandbox_core::placement::{On, Runtime};
use cua_spaces::contract::inputs::FleetRuntime;
use cua_spaces::{SpaceCreate, Spaces};
use std::time::Duration;

#[tokio::test]
async fn e2e_fleet_claim_bash_send_file_release() {
    if std::env::var("CUA_E2E_FLEET").as_deref() != Ok("1") {
        eprintln!(
            "skipped: set CUA_E2E_FLEET=1 and CUA_E2E_FLEET_IMAGE (an image with cua-spacesd)"
        );
        return;
    }
    let Ok(image) = std::env::var("CUA_E2E_FLEET_IMAGE") else {
        eprintln!("skipped: CUA_E2E_FLEET_IMAGE is unset (no public spacesd image to claim)");
        return;
    };
    // The spacesd token reaches the guest through the claim's Secret
    // (`claimSecrets`), which the published `ghcr.io/trycua/linux:24.04`
    // (pre-rename cua-guestd) and newer images both read.
    let runtime = match std::env::var("CUA_E2E_FLEET_RUNTIME").as_deref() {
        Ok("gvisor") => FleetRuntime::Gvisor,
        _ => FleetRuntime::Kubevirt,
    };
    let fleet = cua_fleet::FleetClient::from_env().expect("Fleet credentials");
    let reg = tempfile::tempdir().unwrap();
    // A run-private `cua-e2e-*` namespace so the shared per-image pool Spaces
    // creates is ours to delete (Spaces leaves pools warm on release).
    let namespace = format!("cua-e2e-spaces-{:08x}", rand::random::<u32>());
    let pool = {
        use sha2::Digest;
        let rt = match runtime {
            FleetRuntime::Gvisor => "gvisor",
            _ => "kubevirt",
        };
        let digest = hex::encode(sha2::Sha256::digest(image.as_bytes()));
        format!("{namespace}-{rt}-{}", &digest[..8])
    };
    let spaces = Spaces::builder()
        .home(reg.path())
        .fleet(fleet.clone())
        .fleet_namespace(namespace.clone())
        .build();
    let name = format!("cua-e2e-spaces-{:08x}", rand::random::<u32>());
    let claimed = spaces
        .create(SpaceCreate {
            on: Some(On::Cloud),
            image: Some(image),
            runtime: match runtime {
                FleetRuntime::Gvisor => Runtime::Gvisor,
                _ => Runtime::Kubevirt,
            },
            name: Some(name.clone()),
            ..Default::default()
        })
        .await;
    let delete_pool = || async {
        if let Ok(handle) = fleet.get_pool(&pool).await {
            let _ = fleet.delete_pool(handle).await;
            eprintln!("deleted pool {pool}");
        }
    };
    let info = match claimed.map(|c| c.ready()) {
        Ok(Ok(info)) => info,
        Ok(Err(p)) => {
            delete_pool().await;
            panic!("still pending: {}", p.id)
        }
        Err(e) => {
            delete_pool().await;
            panic!("claim failed: {e}")
        }
    };
    let result: cua_spaces::Result<()> = async {
        let space = spaces.space(&info.id).await?;
        let out = space.bash("uname -s", Duration::from_secs(60)).await?;
        assert!(out.stdout.starts_with("Linux"), "{}", out.render());
        let shot = space
            .spacesd()?
            .screenshot(Default::default())
            .await
            .map_err(cua_spaces::Error::from)?;
        assert!(shot.width > 0 && shot.height > 0 && !shot.image.is_empty());
        eprintln!(
            "screenshot {}x{} ({} bytes)",
            shot.width,
            shot.height,
            shot.image.len()
        );
        if let Ok(path) = std::env::var("CUA_E2E_FLEET_SCREENSHOT") {
            std::fs::write(&path, &shot.image).unwrap();
            eprintln!("wrote {path}");
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "from the e2e").unwrap();
        let r = space
            .send_file(&dir.path().join("hello.txt"), Default::default())
            .await?;
        assert!(r.verified);
        Ok(())
    }
    .await;
    let released = spaces.delete(&info.id).await;
    let claims_left = fleet
        .sdk()
        .list_claims(pool.clone())
        .await
        .map(|c| c.iter().any(|c| c.metadata.name == name));
    delete_pool().await;
    result.unwrap();
    released.unwrap();
    assert_eq!(claims_left.ok(), Some(false), "the claim was released");
    eprintln!("released claim {name}");
}
