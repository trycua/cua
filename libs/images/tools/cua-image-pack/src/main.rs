//! cua-image-pack: containerDisk packing for libs/images via `cua-image`.
//!
//!   cua-image-pack disk   <disk.img> <arch> <out-dir> [--oci-layout DIR] [--push REF]
//!   cua-image-pack index  <REF> <arch>=<disk.img> [<arch>=<disk.img> ...] --work DIR
//!
//! `disk` packs one qcow2 as a single-arch containerDisk (`/disk/disk.img`,
//! uid/gid 107), optionally writing an OCI layout (load with
//! `crane push <dir>` / `skopeo copy oci:<dir> ...`) and/or pushing it.
//! `index` packs several arches and pushes `<REF>-<arch>` plus a multi-arch
//! OCI index at `<REF>` (cua_image::push_multiarch). Registry credentials come
//! from the environment / docker config, as for every cua-image client.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use cua_image::{PackedImage, RegistryClient, containerdisk, push_multiarch};

fn usage() -> ! {
    eprintln!(
        "usage:\n  cua-image-pack disk <disk.img> <arch> <out-dir> [--oci-layout DIR] [--push REF]\n  \
         cua-image-pack index <REF> <arch>=<disk.img>... --work DIR"
    );
    std::process::exit(2)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("disk") => {
            let [disk, arch, out] = [args.get(1), args.get(2), args.get(3)]
                .map(|a| a.cloned().unwrap_or_else(|| usage()));
            let (mut layout, mut push) = (None, None);
            let mut i = 4;
            while i < args.len() {
                match args[i].as_str() {
                    "--oci-layout" => layout = args.get(i + 1).cloned(),
                    "--push" => push = args.get(i + 1).cloned(),
                    _ => usage(),
                }
                i += 2;
            }
            let img = pack(&disk, &arch, &out)?;
            if let Some(dir) = layout {
                img.write_oci_layout(&PathBuf::from(&dir))
                    .context("write OCI layout")?;
                println!("oci-layout {dir}");
            }
            if let Some(reference) = push {
                let d = img
                    .push(&RegistryClient::new(vec![]), &reference)
                    .await
                    .context("push")?;
                println!("pushed {reference}@{}", d.digest);
            }
        }
        Some("index") => {
            let reference = args.get(1).cloned().unwrap_or_else(|| usage());
            let mut work = None;
            let mut disks = Vec::new();
            let mut i = 2;
            while i < args.len() {
                if args[i] == "--work" {
                    work = args.get(i + 1).cloned();
                    i += 2;
                    continue;
                }
                let Some((arch, disk)) = args[i].split_once('=') else {
                    usage()
                };
                disks.push((arch.to_string(), disk.to_string()));
                i += 1;
            }
            let work = work.unwrap_or_else(|| usage());
            if disks.is_empty() {
                bail!("no <arch>=<disk.img> given");
            }
            let mut images = Vec::new();
            for (arch, disk) in &disks {
                images.push(pack(disk, arch, &format!("{work}/{arch}"))?);
            }
            let digest = push_multiarch(&RegistryClient::new(vec![]), &reference, &images)
                .await
                .context("push multi-arch index")?;
            println!("pushed {reference}@{digest}");
        }
        _ => usage(),
    }
    Ok(())
}

fn pack(disk: &str, arch: &str, out: &str) -> Result<PackedImage> {
    let t0 = std::time::Instant::now();
    let img = containerdisk::pack(&PathBuf::from(disk), arch, &PathBuf::from(out))
        .with_context(|| format!("pack {disk}"))?;
    let m = img.manifest();
    let size: u64 = m.layers.iter().map(|l| l.size).sum();
    eprintln!(
        "packed {disk} ({arch}) -> {out}: 1 layer, {size} bytes, {:.1}s",
        t0.elapsed().as_secs_f32()
    );
    Ok(img)
}
