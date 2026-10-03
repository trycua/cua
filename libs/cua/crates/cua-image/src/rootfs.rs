//! gVisor/container rootfs images (the Fleet `docker-*` tags): pack a rootfs
//! tar as a single-layer OCI image, and unpack OCI layers (with whiteouts)
//! into a directory.

use std::io::{BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

use crate::cache::ImageCache;
use crate::digest::HashWriter;
use crate::error::{ImageError, Result};
use crate::layout::PackedImage;
use crate::manifest::{Descriptor, image_config};
use crate::media_types::{OCI_LAYER_GZIP, is_container_layer};
use crate::registry::RegistryClient;

/// Container config applied to a packed rootfs.
#[derive(Clone, Debug, Default)]
pub struct RootfsConfig {
    pub cmd: Option<Vec<String>>,
    pub env: Vec<String>,
    pub exposed_ports: Vec<u16>,
    pub working_dir: Option<String>,
    pub user: Option<String>,
}

/// Gzip an uncompressed rootfs tar (from `docker export`, or `tar -c` over
/// exec) into a single-layer OCI image in `out_dir`.
pub fn pack_tar(
    tar_stream: impl Read,
    arch: &str,
    cfg: &RootfsConfig,
    out_dir: &Path,
) -> Result<PackedImage> {
    std::fs::create_dir_all(out_dir)?;
    let tmp = out_dir.join("layer.tar.gz.partial");
    let compressed = HashWriter::new(std::io::BufWriter::with_capacity(
        1 << 20,
        std::fs::File::create(&tmp)?,
    ));
    let gz = GzEncoder::new(compressed, Compression::new(3));
    let mut uncompressed = HashWriter::new(gz);
    std::io::copy(
        &mut BufReader::with_capacity(1 << 20, tar_stream),
        &mut uncompressed,
    )?;
    let (gz, (diff_id, _)) = uncompressed.finish();
    let (mut file, (digest, size)) = gz.finish()?.finish();
    file.flush()?;
    drop(file);
    let layer_path = out_dir.join(format!("{}.tar.gz", &digest[7..]));
    std::fs::rename(&tmp, &layer_path)?;

    let mut config = image_config(
        arch,
        std::slice::from_ref(&diff_id),
        "rootfs # cua-image",
        cfg.cmd.clone(),
    );
    if !cfg.env.is_empty() {
        config["config"]["Env"] = serde_json::json!(cfg.env);
    }
    if !cfg.exposed_ports.is_empty() {
        let ports: serde_json::Map<String, serde_json::Value> = cfg
            .exposed_ports
            .iter()
            .map(|p| (format!("{p}/tcp"), serde_json::json!({})))
            .collect();
        config["config"]["ExposedPorts"] = serde_json::Value::Object(ports);
    }
    if let Some(w) = &cfg.working_dir {
        config["config"]["WorkingDir"] = serde_json::json!(w);
    }
    if let Some(u) = &cfg.user {
        config["config"]["User"] = serde_json::json!(u);
    }
    let desc = Descriptor {
        media_type: OCI_LAYER_GZIP.into(),
        digest,
        size,
        ..Default::default()
    };
    PackedImage::new(arch, config, vec![(desc, layer_path)])
}

/// Tar a directory (as `docker export` would) and pack it.
pub fn pack_dir(dir: &Path, arch: &str, cfg: &RootfsConfig, out_dir: &Path) -> Result<PackedImage> {
    let tar_path = out_dir.join("rootfs.tar");
    std::fs::create_dir_all(out_dir)?;
    {
        let mut b = tar::Builder::new(std::fs::File::create(&tar_path)?);
        b.follow_symlinks(false);
        b.append_dir_all(".", dir)?;
        b.finish()?;
    }
    let img = pack_tar(std::fs::File::open(&tar_path)?, arch, cfg, out_dir);
    let _ = std::fs::remove_file(&tar_path);
    img
}

fn safe_join(root: &Path, rel: &Path) -> Option<PathBuf> {
    let mut out = root.to_path_buf();
    for c in rel.components() {
        match c {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => return None, // absolute, `..`, prefixes: refuse
        }
    }
    Some(out)
}

/// Apply one OCI layer onto `root`, honouring whiteouts (`.wh.<name>` deletes
/// `<name>`, `.wh..wh..opq` empties the directory).
pub fn apply_layer(layer: impl Read, root: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(layer);
    archive.set_preserve_permissions(true);
    archive.set_overwrite(true);
    archive.set_unpack_xattrs(false);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let parent = path.parent().unwrap_or(Path::new(""));
        if name == ".wh..wh..opq" {
            if let Some(dir) = safe_join(root, parent)
                && let Ok(rd) = std::fs::read_dir(&dir)
            {
                for e in rd.flatten() {
                    let p = e.path();
                    let _ = if p.is_dir() && !p.is_symlink() {
                        std::fs::remove_dir_all(&p)
                    } else {
                        std::fs::remove_file(&p)
                    };
                }
            }
            continue;
        }
        if let Some(target) = name.strip_prefix(".wh.") {
            if let Some(p) = safe_join(root, &parent.join(target)) {
                let _ = if p.is_dir() && !p.is_symlink() {
                    std::fs::remove_dir_all(&p)
                } else {
                    std::fs::remove_file(&p)
                };
            }
            continue;
        }
        if safe_join(root, &path).is_none() {
            return Err(ImageError::Registry(format!(
                "layer entry escapes rootfs: {}",
                path.display()
            )));
        }
        entry.unpack_in(root)?;
    }
    Ok(())
}

/// Unpack a list of (possibly gzipped) layer files, bottom first.
pub fn unpack_layers(layers: &[PathBuf], root: &Path) -> Result<()> {
    std::fs::create_dir_all(root)?;
    for l in layers {
        let mut f = std::fs::File::open(l)?;
        let mut magic = [0u8; 2];
        let n = f.read(&mut magic)?;
        let f = BufReader::with_capacity(1 << 20, std::fs::File::open(l)?);
        if n == 2 && magic == [0x1f, 0x8b] {
            apply_layer(GzDecoder::new(f), root)?;
        } else {
            apply_layer(f, root)?;
        }
    }
    Ok(())
}

/// Pull a rootfs image for `linux/<arch>` and unpack it into the cache
/// (`~/.cua/images/rootfs/<manifest-hex>`); returns the directory.
pub async fn pull(
    client: &RegistryClient,
    cache: &ImageCache,
    reference: &str,
    arch: &str,
) -> Result<PathBuf> {
    let (pinned, manifest, digest) = client.resolve_platform(reference, arch).await?;
    let dest = cache.rootfs_dir(&digest)?;
    if dest.join(".complete").exists() {
        cua_vmm::disk::mark_used(&dest);
        return Ok(dest);
    }
    // Layers plus the unpacked tree (about twice the compressed size).
    let expected = manifest
        .layers
        .iter()
        .map(|l| l.size)
        .sum::<u64>()
        .saturating_mul(3);
    cua_vmm::disk::ensure_space(cache.root(), expected, &format!("pull {reference}"))
        .map_err(cua_vmm::VmmError::from)?;
    let mut files = Vec::new();
    for layer in &manifest.layers {
        if !is_container_layer(&layer.media_type) {
            return Err(ImageError::WrongFormat {
                reference: reference.into(),
                expected: "container rootfs",
                detail: format!("layer media type {}", layer.media_type),
            });
        }
        let blob = cache.blob_path(&layer.digest)?;
        if !blob.exists() {
            client.blob_to_file(&pinned, layer, &blob).await?;
        }
        files.push(blob);
    }
    let d = dest.clone();
    tokio::task::spawn_blocking(move || unpack_layers(&files, &d))
        .await
        .map_err(|e| ImageError::Registry(e.to_string()))??;
    std::fs::write(dest.join(".complete"), &digest)?;
    cache.record_ref(reference, &digest)?;
    cua_vmm::disk::mark_used(&dest);
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(entries: &[(&str, Option<&[u8]>)]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (path, data) in entries {
            let mut h = tar::Header::new_gnu();
            match data {
                Some(d) => {
                    h.set_entry_type(tar::EntryType::Regular);
                    h.set_size(d.len() as u64);
                    h.set_mode(0o644);
                    b.append_data(&mut h, path, *d).unwrap();
                }
                None => {
                    h.set_entry_type(tar::EntryType::Directory);
                    h.set_size(0);
                    h.set_mode(0o755);
                    b.append_data(&mut h, path, std::io::empty()).unwrap();
                }
            }
        }
        b.into_inner().unwrap()
    }

    #[test]
    fn layers_apply_with_whiteouts_and_opaque_dirs() {
        let d = tempfile::tempdir().unwrap();
        let l1 = d.path().join("l1.tar");
        let l2 = d.path().join("l2.tar.gz");
        std::fs::write(
            &l1,
            layer(&[
                ("etc/", None),
                ("etc/keep", Some(b"k")),
                ("etc/gone", Some(b"g")),
                ("var/", None),
                ("var/cache/", None),
                ("var/cache/a", Some(b"a")),
            ]),
        )
        .unwrap();
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&layer(&[
            ("etc/.wh.gone", Some(b"")),
            ("var/cache/.wh..wh..opq", Some(b"")),
            ("var/cache/b", Some(b"b")),
            ("etc/new", Some(b"n")),
        ]))
        .unwrap();
        std::fs::write(&l2, gz.finish().unwrap()).unwrap();
        let root = d.path().join("root");
        unpack_layers(&[l1, l2], &root).unwrap();
        assert!(root.join("etc/keep").exists());
        assert!(!root.join("etc/gone").exists());
        assert!(root.join("etc/new").exists());
        assert!(!root.join("var/cache/a").exists());
        assert_eq!(std::fs::read(root.join("var/cache/b")).unwrap(), b"b");
    }

    #[test]
    fn traversal_is_rejected() {
        let d = tempfile::tempdir().unwrap();
        let mut b = tar::Builder::new(Vec::new());
        let mut h = tar::Header::new_gnu();
        h.set_size(1);
        h.set_mode(0o644);
        h.set_entry_type(tar::EntryType::Regular);
        // tar::Builder refuses `..` in append_data paths, so write the name raw.
        h.as_gnu_mut().unwrap().name[..9].copy_from_slice(b"../escape");
        h.set_cksum();
        b.append(&h, &b"x"[..]).unwrap();
        let bytes = b.into_inner().unwrap();
        assert!(apply_layer(&bytes[..], &d.path().join("r")).is_err());
        assert!(!d.path().join("escape").exists());
    }

    #[test]
    fn pack_dir_produces_config_with_cmd_env_ports() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        std::fs::create_dir_all(src.join("bin")).unwrap();
        std::fs::write(src.join("bin/hello"), b"#!/bin/sh\necho hi\n").unwrap();
        let cfg = RootfsConfig {
            cmd: Some(vec!["/bin/hello".into()]),
            env: vec!["A=b".into()],
            exposed_ports: vec![8080],
            ..Default::default()
        };
        let img = pack_dir(&src, "amd64", &cfg, &d.path().join("out")).unwrap();
        let c: serde_json::Value = serde_json::from_slice(&img.config).unwrap();
        assert_eq!(c["architecture"], "amd64");
        assert_eq!(c["config"]["Cmd"][0], "/bin/hello");
        assert_eq!(c["config"]["Env"][0], "A=b");
        assert!(c["config"]["ExposedPorts"].get("8080/tcp").is_some());
        let root = d.path().join("unpacked");
        unpack_layers(&[img.layers[0].1.clone()], &root).unwrap();
        assert_eq!(
            std::fs::read(root.join("bin/hello")).unwrap(),
            b"#!/bin/sh\necho hi\n"
        );
        // OCI layout export.
        let layout = d.path().join("layout");
        img.write_oci_layout(&layout).unwrap();
        assert!(layout.join("index.json").exists());
        assert!(
            layout
                .join("blobs/sha256")
                .join(&img.layers[0].0.digest[7..])
                .exists()
        );
    }
}
