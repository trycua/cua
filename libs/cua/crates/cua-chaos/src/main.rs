//! `cua-chaos`: plan, snapshot, diff and run seeded chaos scenarios.
//!
//! ```text
//! cua-chaos plan     --profile faults --seed 7
//! cua-chaos snapshot --home $CUA_HOME --cua ./cua -o before.json
//! cua-chaos diff     --before before.json --after after.json [--wanted w.json]
//! cua-chaos run      --profile faults --seeds 1..20 --home $CUA_HOME \
//!                    --driver python3 driver.py --faults faults.json
//! ```
//!
//! Exit status 1 means a leak (the output names the seed and the command
//! that replays it); 2 means the tool itself could not run.

use clap::{Args, Parser, Subcommand};
use cua_chaos::runner::{RunConfig, run};
use cua_chaos::{FaultTable, Inventory, Plan, Profile, Sources, Wanted, collect, cross_check};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "cua-chaos", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Args, Clone)]
struct Scope {
    /// An isolated CUA_HOME to inventory.
    #[arg(long)]
    home: Option<PathBuf>,
    /// A compat gateway directory (its state.json records).
    #[arg(long)]
    gateway_dir: Option<PathBuf>,
    /// Extra sources (JSON, `{"sources": [...]}`), added to the defaults.
    #[arg(long)]
    sources: Option<PathBuf>,
    /// EC2 resources with a tag: `profile,region,key,value`.
    #[arg(long)]
    aws: Option<String>,
    /// Hetzner resources with a label: `token_file,selector`.
    #[arg(long)]
    hetzner: Option<String>,
    /// Name of the scope in reports.
    #[arg(long, default_value = "host")]
    scope: String,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the plan for a profile and seed.
    Plan {
        #[arg(long, default_value = "faults")]
        profile: String,
        /// A custom profile (JSON) instead of a preset.
        #[arg(long)]
        profile_file: Option<PathBuf>,
        #[arg(long)]
        seed: u64,
    },
    /// Take an inventory.
    Snapshot {
        #[command(flatten)]
        scope: Scope,
        #[arg(long, default_value = "snapshot")]
        label: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Diff two inventories: exit 1 on any leak.
    Diff {
        #[arg(long)]
        before: PathBuf,
        #[arg(long)]
        after: PathBuf,
        /// A `Wanted` JSON file (Spaces kept on purpose).
        #[arg(long)]
        wanted: Option<PathBuf>,
        /// Treat collector warnings as failures.
        #[arg(long)]
        strict: bool,
    },
    /// Two-way check of a Store kind against a cloud kind in one inventory.
    Crosscheck {
        #[arg(long)]
        inventory: PathBuf,
        #[arg(long)]
        store_kind: String,
        #[arg(long)]
        cloud_kind: String,
        /// Cloud ids that legitimately have no Store record.
        #[arg(long)]
        ignore: Vec<String>,
    },
    /// Play seeded plans against a driver and check for leaks after each.
    Run {
        #[command(flatten)]
        scope: Scope,
        #[arg(long, default_value = "faults")]
        profile: String,
        #[arg(long)]
        profile_file: Option<PathBuf>,
        /// One seed, or a range `a..b`.
        #[arg(long)]
        seeds: String,
        /// Where the run keeps its files (pidfiles, slots, reports).
        #[arg(long)]
        dir: PathBuf,
        /// The driver command (one operation per execution).
        #[arg(long, num_args = 1.., required = true, allow_hyphen_values = true)]
        driver: Vec<String>,
        /// Fault commands (JSON `{"faults": {...}}`); default: the host ones.
        #[arg(long)]
        faults: Option<PathBuf>,
        #[arg(long)]
        wanted_cmd: Option<String>,
        #[arg(long)]
        cleanup_cmd: Option<String>,
        #[arg(long, default_value_t = 90)]
        settle_s: u64,
        #[arg(long, default_value_t = 1.0)]
        speedup: f64,
        /// Stop at the first leaking seed.
        #[arg(long)]
        fail_fast: bool,
        /// Fail when a collector could not run.
        #[arg(long)]
        strict: bool,
    },
}

fn profile(name: &str, file: &Option<PathBuf>) -> Result<Profile, String> {
    if let Some(f) = file {
        let t = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        return serde_json::from_str(&t).map_err(|e| format!("{}: {e}", f.display()));
    }
    Profile::preset(name).ok_or_else(|| {
        format!("no profile '{name}' (sandbox, faults, host, daemon, hostloss, modal, burst)")
    })
}

fn sources(s: &Scope) -> Result<Sources, String> {
    let mut out = match &s.home {
        Some(h) => Sources::for_cua_home(h, s.gateway_dir.as_deref()),
        None => Sources::default(),
    };
    if let Some(f) = &s.sources {
        let t = std::fs::read_to_string(f).map_err(|e| format!("{}: {e}", f.display()))?;
        out.sources
            .extend(Sources::from_json(&t).map_err(|e| e.to_string())?.sources);
    }
    if let Some(a) = &s.aws {
        let p: Vec<&str> = a.split(',').collect();
        if p.len() != 4 {
            return Err("--aws profile,region,key,value".into());
        }
        out.sources.push(cua_chaos::Source::Aws {
            profile: p[0].into(),
            region: p[1].into(),
            tag_key: p[2].into(),
            tag_value: p[3].into(),
        });
    }
    if let Some(h) = &s.hetzner {
        let (f, sel) = h.split_once(',').ok_or("--hetzner token_file,selector")?;
        out.sources.push(cua_chaos::Source::Hetzner {
            token_file: f.into(),
            label_selector: sel.into(),
        });
    }
    Ok(out)
}

fn seeds(s: &str) -> Result<Vec<u64>, String> {
    if let Some((a, b)) = s.split_once("..") {
        let (a, b): (u64, u64) = (
            a.parse().map_err(|_| "bad range")?,
            b.parse().map_err(|_| "bad range")?,
        );
        return Ok((a..b).collect());
    }
    s.split(',')
        .map(|x| x.parse().map_err(|_| format!("bad seed '{x}'")))
        .collect()
}

fn main() {
    let cli = Cli::parse();
    match go(cli.cmd) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("cua-chaos: {e}");
            std::process::exit(2);
        }
    }
}

fn go(cmd: Cmd) -> Result<i32, String> {
    match cmd {
        Cmd::Plan {
            profile: p,
            profile_file,
            seed,
        } => {
            let plan = profile(&p, &profile_file)?.plan(seed);
            println!(
                "{}",
                serde_json::to_string_pretty(&plan).map_err(|e| e.to_string())?
            );
            Ok(0)
        }
        Cmd::Snapshot { scope, label, out } => {
            let inv = collect(&scope.scope, &label, &sources(&scope)?);
            match out {
                Some(p) => std::fs::write(&p, inv.to_json()).map_err(|e| e.to_string())?,
                None => println!("{}", inv.to_json()),
            }
            for w in &inv.warnings {
                eprintln!("warning: {w}");
            }
            Ok(0)
        }
        Cmd::Diff {
            before,
            after,
            wanted,
            strict,
        } => {
            let rd = |p: &PathBuf| -> Result<Inventory, String> {
                Inventory::from_json(
                    &std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?,
                )
                .map_err(|e| e.to_string())
            };
            let w: Wanted = match wanted {
                Some(p) => serde_json::from_str(
                    &std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?,
                )
                .map_err(|e| e.to_string())?,
                None => Wanted::default(),
            };
            let (b, a) = (rd(&before)?, rd(&after)?);
            let r = a.leaks(&b, &w);
            print!("{r}");
            Ok(i32::from(
                !r.is_clean() || (strict && !r.warnings.is_empty()),
            ))
        }
        Cmd::Crosscheck {
            inventory,
            store_kind,
            cloud_kind,
            ignore,
        } => {
            let inv = Inventory::from_json(
                &std::fs::read_to_string(&inventory).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            let ig: BTreeSet<String> = ignore.into_iter().collect();
            let r = cross_check(&inv, &store_kind, &cloud_kind, &ig);
            print!("{r}");
            Ok(i32::from(!r.is_clean()))
        }
        Cmd::Run {
            scope,
            profile: p,
            profile_file,
            seeds: sd,
            dir,
            driver,
            faults,
            wanted_cmd,
            cleanup_cmd,
            settle_s,
            speedup,
            fail_fast,
            strict,
        } => {
            let prof = profile(&p, &profile_file)?;
            let table = match faults {
                Some(f) => FaultTable::from_json(
                    &std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?,
                )
                .map_err(|e| e.to_string())?,
                None => FaultTable::host_defaults(),
            };
            let srcs = sources(&scope)?;
            let mut failed = 0;
            let all = seeds(&sd)?;
            for seed in &all {
                let plan: Plan = prof.plan(*seed);
                let cfg = RunConfig {
                    dir: dir.join(format!("seed-{seed}")),
                    driver: driver.clone(),
                    faults: table.clone(),
                    sources: srcs.clone(),
                    wanted_cmd: wanted_cmd.clone(),
                    cleanup_cmd: cleanup_cmd.clone(),
                    settle: Duration::from_secs(settle_s),
                    step_timeout: Duration::from_secs(300),
                    speedup,
                    env: vec![],
                };
                let res = run(&cfg, &plan, &scope.scope);
                let bad = !res.report.is_clean()
                    || !res.fault_errors.is_empty()
                    || (strict && !res.report.warnings.is_empty());
                println!(
                    "seed {seed}: {} steps ({} killed), faults {:?}, settle {} ms, {} leak(s){}",
                    res.steps.len(),
                    res.killed(),
                    res.faults_injected,
                    res.settle_ms,
                    res.report.leaks.len(),
                    if bad { "  FAILED" } else { "" }
                );
                let _ = std::fs::write(
                    dir.join(format!("seed-{seed}.json")),
                    serde_json::to_string_pretty(&res).unwrap_or_default(),
                );
                if bad {
                    failed += 1;
                    print!("{}", res.report);
                    for e in &res.fault_errors {
                        println!("  fault error: {e}");
                    }
                    println!(
                        "  replay: cua-chaos run --profile {p} --seeds {seed} ... (plan digest {})",
                        res.digest
                    );
                    if fail_fast {
                        break;
                    }
                }
            }
            println!("{} seed(s), {failed} failed", all.len());
            Ok(i32::from(failed > 0))
        }
    }
}
