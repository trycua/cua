//! Runs one gang member in this machine's own Space the way a dedicated
//! host's daemon runs it (`kind = "machine"`, through the machine's
//! cua-spacesd), with the mesh peer router (CUA-1783), so its peers on
//! other machines are reached over the Cua mesh. For measuring rank
//! traffic between machines without a coordinator: the peers and their
//! machines are given by hand, as a coordinator would place them.
//!
//! ```text
//! gang-member --home <cua home> --run NAME --rank R \
//!   --peers ip:port,ip:port --machines id,id [--spacesd http://127.0.0.1:3211] \
//!   [--timeout 600] -- ARGV...
//! ```
//!
//! `<cua home>` is the machine's (its cua-spacesd, `cua-spacesd join` with
//! `--data-dir <cua home>/spacesd` and `--token-file <cua home>/host/env-token`,
//! serves the mesh control socket there and runs the member). The member's
//! output (with the `cua: peer rank ...` lines) is printed when it ends;
//! the exit code is the member's.

use std::sync::Arc;

use cua_cluster::gang_mesh::MeshPeerRouter;
use cua_cluster::machine::{HostSpacesd, MachineExecutor};
use cua_cluster::{ExecEnd, JobExecutor as _, JobSpec};
use tokio_util::sync::CancellationToken;

fn usage() -> ! {
    eprintln!(
        "usage: gang-member --home DIR --run NAME --rank R --peers A,B --machines M,N [--spacesd URL] [--timeout S] [--env K=V]... -- ARGV..."
    );
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    let mut home = None;
    let mut run = None;
    let mut rank = None::<u32>;
    let mut peers = None::<String>;
    let mut machines = None::<String>;
    let mut timeout = 600u32;
    let mut spacesd = "http://127.0.0.1:3211".to_string();
    let mut env = std::collections::BTreeMap::new();
    let mut argv = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut v = || it.next().unwrap_or_else(|| usage());
        match a.as_str() {
            "--home" => home = Some(v()),
            "--run" => run = Some(v()),
            "--rank" => rank = v().parse().ok(),
            "--peers" => peers = Some(v()),
            "--machines" => machines = Some(v()),
            "--timeout" => timeout = v().parse().unwrap_or_else(|_| usage()),
            "--spacesd" => spacesd = v(),
            "--env" => {
                let kv = v();
                let (k, val) = kv.split_once('=').unwrap_or_else(|| usage());
                env.insert(k.to_string(), val.to_string());
            }
            "--" => {
                argv = it.by_ref().collect();
                break;
            }
            _ => usage(),
        }
    }
    let (Some(home), Some(run), Some(rank), Some(peers), Some(machines)) =
        (home, run, rank, peers, machines)
    else {
        usage()
    };
    if argv.is_empty() {
        usage()
    }
    let world = peers.split(',').count();
    for (k, v) in [
        ("CUA_RUN_ID", run.clone()),
        ("CUA_ATTEMPT", "1".to_string()),
        ("CUA_RANK", rank.to_string()),
        ("CUA_WORLD_SIZE", world.to_string()),
        ("CUA_PEERS", peers),
        ("CUA_PEER_MACHINES", machines),
    ] {
        env.insert(k.to_string(), v);
    }
    let job = JobSpec {
        job_id: format!("local/{run}-{rank}"),
        attempt: 1,
        kind: "machine".into(),
        argv,
        env,
        timeout_secs: timeout,
        max_output_bytes: 4 << 20,
        run_id: format!("local/{run}"),
        ..Default::default()
    };
    let token_file = std::path::Path::new(&home).join("host").join("env-token");
    let exec = MachineExecutor::new(&home, move || {
        Ok(HostSpacesd {
            url: spacesd.clone(),
            token: std::fs::read_to_string(&token_file)
                .map_err(|e| format!("{}: {e}", token_file.display()))?
                .trim()
                .to_string(),
        })
    })
    .with_peer_router(Arc::new(MeshPeerRouter::new(&home)));
    let space = cua_cluster::model::space_name(&job.job_id, 1);
    let out = exec.run(&job, &space, CancellationToken::new()).await;
    print!("{}", String::from_utf8_lossy(&out.stdout));
    let code = match out.end {
        ExecEnd::Exited(c) => c,
        other => {
            eprintln!("gang-member: {other:?}");
            1
        }
    };
    std::process::exit(code);
}
