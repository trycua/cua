//! App icon lookup benchmark against a real Space: a window list's icons
//! fetched one app at a time (what every UI did before the shared cache),
//! then as one batch cold, warm from memory, and warm from disk (a fresh
//! process: memory empty, `$CUA_HOME/cache/icons` kept).
//!
//!     CUA_HOME=<temp> cargo run -p cua-sdk --example icon_bench -- <space id> [windows]
//!
//! Run it with a throwaway `CUA_HOME` (and `HOME`): it clears the icon
//! cache directory under it between passes.

use cua_sdk::{Cua, CuaConfig, SpaceAppIconRequest};
use std::time::{Duration, Instant};

fn ms(d: Duration) -> String {
    format!("{:.3} ms", d.as_secs_f64() * 1000.0)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let id = args
        .next()
        .ok_or("usage: icon_bench <space id> [windows]")?;
    let want: usize = args.next().map(|n| n.parse()).transpose()?.unwrap_or(10);
    let dir = cua_icon_cache::default_dir();
    if cua_home::is_under_real_home(&dir) {
        return Err("set CUA_HOME to a throwaway directory first".into());
    }
    let cua = Cua::embedded(CuaConfig::default())?;
    let space = cua.spaces().space(id.clone()).await?;
    let windows = space.windows(None).await?;
    if windows.is_empty() {
        return Err("the Space lists no windows".into());
    }
    // `want` rows, repeating the Space's windows if it has fewer.
    let requests: Vec<SpaceAppIconRequest> = windows
        .iter()
        .cycle()
        .take(want)
        .map(|w| SpaceAppIconRequest {
            app_name: w.app_name.clone(),
            app_id: w.app_id.clone(),
            pid: w.pid,
        })
        .collect();
    let apps: std::collections::BTreeSet<String> = requests
        .iter()
        .map(|r| cua_spaces::app_icon::icon_identity(&r.app_name, &r.app_id))
        .collect();
    let cache = cua_icon_cache::IconCache::shared();
    let cold = || {
        cache.clear_memory();
        let _ = std::fs::remove_dir_all(&dir);
    };

    cold();
    let t = Instant::now();
    let mut one_by_one = 0;
    let mut seen = std::collections::BTreeSet::new();
    for r in &requests {
        // The old per-UI path: one lookup per app, each its own round trip.
        if seen.insert(cua_spaces::app_icon::icon_identity(&r.app_name, &r.app_id)) {
            cache.clear_memory();
            let _ = std::fs::remove_dir_all(&dir);
        }
        if space.app_icons(vec![r.clone()]).await?[0].is_some() {
            one_by_one += 1;
        }
    }
    let per_app = t.elapsed();

    cold();
    let t = Instant::now();
    let first = space.app_icons(requests.clone()).await?;
    let batch_cold = t.elapsed();

    let t = Instant::now();
    let _ = space.app_icons(requests.clone()).await?;
    let memory = t.elapsed();

    cache.clear_memory();
    let t = Instant::now();
    let _ = space.app_icons(requests.clone()).await?;
    let disk = t.elapsed();

    let found = first.iter().filter(|i| i.is_some()).count();
    println!(
        "space {id}: {} windows, {} apps, {found} with an icon ({one_by_one} one by one)",
        requests.len(),
        apps.len()
    );
    println!("one app at a time, uncached : {}", ms(per_app));
    println!("one batch, cold             : {}", ms(batch_cold));
    println!("cached, memory              : {}", ms(memory));
    println!("cached, disk (new process)  : {}", ms(disk));
    println!("stats: {:?}", cache.stats());
    Ok(())
}
