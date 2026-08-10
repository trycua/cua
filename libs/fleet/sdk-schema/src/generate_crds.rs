use cyclops_sdk_schema::generate::{check_crds, write_crds};
use std::{env, path::PathBuf, process};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error:#}");
        process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let mut check = false;
    let mut output = PathBuf::from("clusters/base/osgym/crd.yaml");
    let mut arguments = env::args_os().skip(1);

    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--check") => check = true,
            Some("--output") => {
                output = arguments
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow::anyhow!("--output requires a path"))?;
            }
            Some("--help") | Some("-h") => {
                println!("Usage: generate-crds [--check] [--output <path>]");
                return Ok(());
            }
            _ => anyhow::bail!("unknown argument: {}", argument.to_string_lossy()),
        }
    }

    if check {
        return check_crds(output);
    }

    write_crds(&output)?;
    println!("wrote {}", output.display());
    Ok(())
}
