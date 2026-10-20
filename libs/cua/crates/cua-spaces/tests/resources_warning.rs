//! The low-disk signal: a watcher hears when free space crosses the warning
//! line and again when it recovers. The free-space source is process-wide
//! (`cua_vmm::disk::set_probe`), so this is the only test in its binary.

use cua_spaces::Spaces;
use cua_spaces::resources::{DiskLevel, ResourcesEvent};
use cua_vmm::disk::{Space, SpaceProbe, set_probe};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const GIB: u64 = 1 << 30;

/// A 1 TiB volume with as much free as the test says.
struct Volume(Arc<AtomicU64>);

impl SpaceProbe for Volume {
    fn space(&self, _: &Path) -> std::io::Result<Space> {
        Ok(Space {
            available: self.0.load(Ordering::SeqCst),
            total: 1024 * GIB,
        })
    }
}

async fn next(w: &mut cua_spaces::resources::ResourceWatch) -> ResourcesEvent {
    tokio::time::timeout(Duration::from_secs(10), w.next())
        .await
        .expect("an event arrives")
        .expect("the watch is open")
}

/// Redo the sample once the last one is old enough to be replaced.
async fn nudge(w: &mut cua_spaces::resources::ResourceWatch) {
    tokio::time::sleep(Duration::from_millis(700)).await;
    w.set_visible(false);
    w.set_visible(true);
}

#[tokio::test]
async fn crossing_the_line_and_recovering_are_events() {
    let free = Arc::new(AtomicU64::new(400 * GIB));
    set_probe(Some(Arc::new(Volume(free.clone()))));
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    let mut watch = spaces.watch_resources(true);

    // Plenty free: a snapshot, level ok.
    let ResourcesEvent::Snapshot(snap) = next(&mut watch).await else {
        panic!("a snapshot first");
    };
    assert_eq!(snap.warning.level, DiskLevel::Ok);
    assert_eq!(snap.this_mac.disk_free, 400 * GIB);
    assert_eq!(snap.this_mac.disk_total, 1024 * GIB);

    // 90 GiB free of 1 TiB: under the 100 GiB line (15% is capped there).
    free.store(90 * GIB, Ordering::SeqCst);
    nudge(&mut watch).await;
    let ResourcesEvent::Update(u) = next(&mut watch).await else {
        panic!("the free space moved: an update");
    };
    assert_eq!(u.this_mac.unwrap().disk_free, 90 * GIB);
    let ResourcesEvent::LowDisk(w) = next(&mut watch).await else {
        panic!("then the warning");
    };
    assert_eq!(w.level, DiskLevel::Low);
    assert_eq!(
        (w.free, w.total, w.threshold),
        (90 * GIB, 1024 * GIB, 100 * GIB)
    );

    // Under the floor where creates already fail.
    free.store(3 * GIB, Ordering::SeqCst);
    nudge(&mut watch).await;
    let _update = next(&mut watch).await;
    let ResourcesEvent::LowDisk(w) = next(&mut watch).await else {
        panic!("the warning got worse");
    };
    assert_eq!(w.level, DiskLevel::Critical);

    // Space is freed: back to ok, said once.
    free.store(400 * GIB, Ordering::SeqCst);
    nudge(&mut watch).await;
    let _update = next(&mut watch).await;
    let ResourcesEvent::LowDisk(w) = next(&mut watch).await else {
        panic!("the warning cleared");
    };
    assert_eq!((w.level, w.threshold), (DiskLevel::Ok, 0));

    // A poll sees the same signal for the menu bar.
    let r = spaces.resources(false).await.unwrap();
    assert_eq!(r.warning.level, DiskLevel::Ok);
    set_probe(None);
}
