//! KubeVirt containerDisk images: `FROM scratch` + `/disk/disk.img` (qcow2,
//! owned by uid/gid 107). This is the format Fleet boots, so the same ref
//! runs locally under QEMU.

use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

use crate::cache::{CacheLock, ImageCache};
use crate::digest::HashWriter;
use crate::error::{ImageError, Result};
use crate::layout::PackedImage;
use crate::manifest::{Descriptor, image_config};
use crate::media_types::{
    CONTAINER_DISK_PATH, CONTAINER_DISK_UID, OCI_LAYER_GZIP, is_vm_media_type,
};
use crate::registry::RegistryClient;

/// Open a layer file, transparently gunzipping it (by magic bytes). `read`
/// counts the layer's bytes consumed so far.
fn open_layer(path: &Path, read: Arc<AtomicU64>) -> Result<Box<dyn Read>> {
    let mut f = std::fs::File::open(path)?;
    let mut magic = [0u8; 2];
    let n = f.read(&mut magic)?;
    let f = CountingReader {
        inner: std::fs::File::open(path)?,
        read,
    };
    if n == 2 && magic == [0x1f, 0x8b] {
        Ok(Box::new(GzDecoder::new(BufReader::with_capacity(
            1 << 20,
            f,
        ))))
    } else {
        Ok(Box::new(BufReader::with_capacity(1 << 20, f)))
    }
}

/// A reader that counts the bytes read through it.
struct CountingReader<R> {
    inner: R,
    read: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

fn is_disk_entry(path: &Path) -> bool {
    let s = path.to_string_lossy();
    let s = s.trim_start_matches("./").trim_start_matches('/');
    s == CONTAINER_DISK_PATH
}

/// Extract `disk/disk.img` from a (possibly gzipped) layer tar into `dest`.
/// Returns `false` when the layer does not contain it.
pub fn extract_disk(layer: &Path, dest: &Path) -> Result<bool> {
    extract_disk_counting(layer, dest, Arc::default())
}

/// [`extract_disk`], counting the layer bytes read into `read`.
fn extract_disk_counting(layer: &Path, dest: &Path, read: Arc<AtomicU64>) -> Result<bool> {
    let mut archive = tar::Archive::new(open_layer(layer, read)?);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_file() && is_disk_entry(&entry.path()?) {
            let tmp = dest.with_extension("partial");
            {
                let mut out =
                    std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp)?);
                std::io::copy(&mut entry, &mut out)?;
                out.flush()?;
            }
            std::fs::rename(tmp, dest)?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Pack a qcow2 disk as a containerDisk layer (`disk/` + `disk/disk.img`,
/// uid/gid 107) into `out_dir`, returning a pushable image.
pub fn pack(disk: &Path, arch: &str, out_dir: &Path) -> Result<PackedImage> {
    std::fs::create_dir_all(out_dir)?;
    let tmp = out_dir.join("layer.tar.gz.partial");
    let compressed = HashWriter::new(std::io::BufWriter::with_capacity(
        1 << 20,
        std::fs::File::create(&tmp)?,
    ));
    // qcow2 from `qemu-img convert -c` is already compressed; fast gzip keeps
    // the layer a standard tar+gzip without spending minutes re-compressing.
    let gz = GzEncoder::new(compressed, Compression::fast());
    let uncompressed = HashWriter::new(gz);
    let mut builder = tar::Builder::new(uncompressed);
    builder.mode(tar::HeaderMode::Deterministic);

    let mut dir = tar::Header::new_gnu();
    dir.set_entry_type(tar::EntryType::Directory);
    dir.set_mode(0o555);
    dir.set_uid(CONTAINER_DISK_UID);
    dir.set_gid(CONTAINER_DISK_UID);
    dir.set_size(0);
    dir.set_mtime(0);
    builder.append_data(&mut dir, "disk/", std::io::empty())?;

    let meta = std::fs::metadata(disk)?;
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Regular);
    h.set_mode(0o444);
    h.set_uid(CONTAINER_DISK_UID);
    h.set_gid(CONTAINER_DISK_UID);
    h.set_size(meta.len());
    h.set_mtime(0);
    builder.append_data(
        &mut h,
        CONTAINER_DISK_PATH,
        BufReader::with_capacity(1 << 20, std::fs::File::open(disk)?),
    )?;

    let uncompressed = builder.into_inner()?;
    let (gz, (diff_id, _)) = uncompressed.finish();
    let compressed = gz.finish()?;
    let (mut file, (digest, size)) = compressed.finish();
    file.flush()?;
    drop(file);
    let layer_path = out_dir.join(format!("{}.tar.gz", &digest[7..]));
    std::fs::rename(&tmp, &layer_path)?;

    let config = image_config(
        arch,
        std::slice::from_ref(&diff_id),
        "COPY disk.img /disk/disk.img # cua-image",
        None,
    );
    let desc = Descriptor {
        media_type: OCI_LAYER_GZIP.into(),
        digest,
        size,
        ..Default::default()
    };
    PackedImage::new(arch, config, vec![(desc, layer_path)])
}

/// Pull a containerDisk for `linux/<arch>` and return the cached qcow2.
///
/// Cached per platform-manifest digest at `~/.cua/images/disks/<hex>/disk.qcow2`
/// (so a moving tag re-pulls only when it changes). The compressed layer blob
/// is deleted after extraction unless `keep_blobs` (disks are GBs).
pub async fn pull(
    client: &RegistryClient,
    cache: &ImageCache,
    reference: &str,
    arch: &str,
    keep_blobs: bool,
) -> Result<PathBuf> {
    let (pinned, manifest, digest) = client.resolve_platform(reference, arch).await?;
    let dir = cache.disk_dir(&digest)?;
    let disk = dir.join("disk.qcow2");
    if disk.exists() {
        cache.record_ref(reference, &digest)?;
        cua_vmm::disk::mark_used(&dir);
        return Ok(disk);
    }
    let lock_path = dir.join(".lock");
    let disk_done = disk.clone();
    let Some(_lock) = CacheLock::acquire(
        &lock_path,
        move || disk_done.exists(),
        Duration::from_secs(6 * 3600),
    )
    .await?
    else {
        cua_vmm::disk::mark_used(&dir);
        return Ok(disk);
    };
    if disk.exists() {
        cua_vmm::disk::mark_used(&dir);
        return Ok(disk);
    }
    // The compressed layer and the extracted disk exist side by side until
    // the layer is deleted; qcow2 disks are already compressed, so the
    // extracted disk is about the layer's size.
    let expected: u64 = manifest
        .layers
        .iter()
        .filter(|l| !is_vm_media_type(&l.media_type))
        .map(|l| l.size)
        .sum::<u64>()
        .saturating_mul(2);
    cua_vmm::disk::ensure_space(cache.root(), expected, &format!("pull {reference}"))
        .map_err(cua_vmm::VmmError::from)?;
    // The pull reports one fraction over downloading and then extracting
    // each layer, by bytes (see `PullProgress`).
    let mut progress = PullProgress::new(reference, &manifest.layers);
    // The disk is normally in the last layer; search from the top.
    for layer in manifest.layers.iter().rev() {
        if is_vm_media_type(&layer.media_type) {
            continue;
        }
        let blob = cache.blob_path(&layer.digest)?;
        if !blob.exists() {
            tracing::info!(reference, digest = %layer.digest, size = layer.size, "pulling containerDisk layer");
            client
                .blob_to_file_with_progress(&pinned, layer, &blob, |n| progress.downloaded(n))
                .await?;
        }
        progress.download_done(layer.size);
        let (b, d) = (blob.clone(), disk.clone());
        std::fs::create_dir_all(&dir)?;
        let read = Arc::new(AtomicU64::new(0));
        let counter = read.clone();
        let mut extract =
            tokio::task::spawn_blocking(move || extract_disk_counting(&b, &d, counter));
        let found = loop {
            tokio::select! {
                done = &mut extract => break done,
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    progress.extracted(read.load(Ordering::Relaxed));
                }
            }
        }
        .map_err(|e| ImageError::Registry(e.to_string()))??;
        progress.extract_done(layer.size);
        if !keep_blobs {
            let _ = std::fs::remove_file(&blob);
        }
        if found {
            cache.record_ref(reference, &digest)?;
            cua_vmm::disk::mark_used(&dir);
            return Ok(disk);
        }
    }
    Err(ImageError::WrongFormat {
        reference: reference.into(),
        expected: "KubeVirt containerDisk",
        detail: format!("no {CONTAINER_DISK_PATH} in any layer"),
    })
}

/// Share of a containerDisk pull spent downloading (the rest is extracting
/// the disk from the layer), from measured cold pulls on an Apple silicon
/// Mac: `omarchy:edge` (1.6 GB) 54 s down, 9 s extracting;
/// `linux:24.04-slim-disk` (0.55 GB) 17 s down, 3 s extracting.
const DOWNLOAD_SHARE: f64 = 0.85;

/// A containerDisk pull's progress, reported as `pulling` with one fraction
/// (downloading, then extracting, each by bytes), at most once per percent.
struct PullProgress<'a> {
    reference: &'a str,
    /// Bytes of every layer that may be pulled.
    total: u64,
    /// Bytes of finished layers (downloads, extractions).
    downloads: u64,
    extractions: u64,
    last: f64,
    /// The download's bytes, a few reports a second.
    meter: cua_vmm::progress::Meter,
}

impl<'a> PullProgress<'a> {
    fn new(reference: &'a str, layers: &[Descriptor]) -> Self {
        let total = layers
            .iter()
            .filter(|l| !is_vm_media_type(&l.media_type))
            .map(|l| l.size)
            .sum();
        cua_vmm::progress::report(
            cua_vmm::progress::Progress::phase(cua_vmm::progress::Phase::Pulling).detail(reference),
        );
        Self {
            reference,
            total,
            downloads: 0,
            extractions: 0,
            last: -1.0,
            meter: cua_vmm::progress::Meter::new(),
        }
    }

    fn report(&mut self, download: u64, extract: u64) {
        if self.total == 0 {
            return;
        }
        let t = self.total as f64;
        let f = DOWNLOAD_SHARE * (download as f64 / t).min(1.0)
            + (1.0 - DOWNLOAD_SHARE) * (extract as f64 / t).min(1.0);
        let downloaded = download.min(self.total);
        // Bytes still arriving: the meter's pace; then (extracting) a
        // report per percent.
        let transfer = self.meter.sample(downloaded, self.total);
        if transfer.is_some() || f - self.last >= 0.01 || (f >= 1.0 && self.last < 1.0) {
            self.last = self.last.max(f);
            cua_vmm::progress::report(
                cua_vmm::progress::Progress::phase(cua_vmm::progress::Phase::Pulling)
                    .fraction(f)
                    .detail(self.reference)
                    .bytes(transfer.unwrap_or_else(|| self.meter.peek(downloaded, self.total))),
            );
        }
    }

    fn downloaded(&mut self, bytes: u64) {
        self.report(self.downloads + bytes, self.extractions);
    }

    fn download_done(&mut self, size: u64) {
        self.downloads += size;
        self.report(self.downloads, self.extractions);
    }

    fn extracted(&mut self, bytes: u64) {
        self.report(self.downloads, self.extractions + bytes);
    }

    fn extract_done(&mut self, size: u64) {
        self.extractions += size;
        self.report(self.downloads, self.extractions);
    }
}

/// [`cua_vmm::DiskResolver`] that pulls containerDisks into the image cache,
/// so `ImageSource::Oci` works with the QEMU backend.
#[derive(Clone, Default)]
pub struct ContainerDiskResolver {
    pub client: RegistryClient,
    pub cache: ImageCache,
}

impl ContainerDiskResolver {
    pub fn shared() -> Arc<dyn cua_vmm::DiskResolver> {
        Arc::new(Self::default())
    }
}

#[async_trait::async_trait]
impl cua_vmm::DiskResolver for ContainerDiskResolver {
    async fn resolve(&self, reference: &str, arch: cua_vmm::Arch) -> cua_vmm::Result<PathBuf> {
        Ok(pull(&self.client, &self.cache, reference, arch.oci(), false).await?)
    }

    /// Pulls a private containerDisk with the caller's credentials (a
    /// client scoped to this pull; the shared client keeps its own auth).
    async fn resolve_with_credentials(
        &self,
        reference: &str,
        arch: cua_vmm::Arch,
        creds: Option<&cua_vmm::RegistryCredentials>,
    ) -> cua_vmm::Result<PathBuf> {
        match creds {
            None => self.resolve(reference, arch).await,
            Some(c) => {
                let client = RegistryClient::with_credentials(c.clone());
                Ok(pull(&client, &self.cache, reference, arch.oci(), false).await?)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_pull_reports_one_fraction_over_download_then_extraction() {
        use cua_vmm::progress::{Phase, Progress, scope};
        let heard: Arc<std::sync::Mutex<Vec<Progress>>> = Arc::default();
        let sink = heard.clone();
        let layer = |size: u64| Descriptor {
            media_type: OCI_LAYER_GZIP.into(),
            digest: format!("sha256:{}", "a".repeat(64)),
            size,
            ..Default::default()
        };
        scope(
            Arc::new(move |p: &Progress| sink.lock().unwrap().push(p.clone())),
            async {
                let layers = [layer(1000)];
                let mut p = PullProgress::new("r", &layers);
                for n in (0..=1000).step_by(100) {
                    p.downloaded(n);
                }
                p.download_done(1000);
                p.extracted(500);
                p.extract_done(1000);
            },
        )
        .await;
        let heard = heard.lock().unwrap();
        assert!(
            heard
                .iter()
                .all(|p| p.phase == Phase::Pulling && p.detail == "r")
        );
        let fractions: Vec<f64> = heard.iter().filter_map(|p| p.fraction).collect();
        assert!(fractions.windows(2).all(|w| w[0] < w[1]), "{fractions:?}");
        // The download's bytes ride along: the first sample and the end
        // (the meter throttles the instant samples between them).
        let bytes: Vec<(u64, u64)> = heard
            .iter()
            .filter_map(|p| p.bytes.map(|b| (b.done, b.total)))
            .collect();
        assert_eq!(bytes.first(), Some(&(0, 1000)), "{bytes:?}");
        assert!(bytes.contains(&(1000, 1000)), "{bytes:?}");
        assert!(bytes.windows(2).all(|w| w[0].0 <= w[1].0), "{bytes:?}");
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(
            fractions.iter().any(|f| near(*f, DOWNLOAD_SHARE)),
            "downloaded"
        );
        assert!(
            fractions
                .iter()
                .any(|f| near(*f, DOWNLOAD_SHARE + (1.0 - DOWNLOAD_SHARE) / 2.0))
        );
        assert!(near(*fractions.last().unwrap(), 1.0));
    }

    #[test]
    fn pack_then_extract_round_trips_with_kubevirt_ownership() {
        let d = tempfile::tempdir().unwrap();
        let disk = d.path().join("in.qcow2");
        let payload: Vec<u8> = (0..300_000u32).flat_map(|i| i.to_le_bytes()).collect();
        std::fs::write(&disk, &payload).unwrap();
        let img = pack(&disk, "aarch64", &d.path().join("out")).unwrap();
        assert_eq!(img.arch, "arm64");
        let (desc, layer) = &img.layers[0];
        assert_eq!(desc.size, std::fs::metadata(layer).unwrap().len());
        assert_eq!(crate::digest::sha256_file(layer).unwrap().0, desc.digest);

        // Ownership / modes as KubeVirt expects.
        let mut ar = tar::Archive::new(open_layer(layer, Arc::default()).unwrap());
        let entries: Vec<(String, u64, u32)> = ar
            .entries()
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.path().unwrap().display().to_string(),
                    e.header().uid().unwrap(),
                    e.header().mode().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            entries,
            vec![
                ("disk/".into(), 107, 0o555),
                ("disk/disk.img".into(), 107, 0o444)
            ]
        );

        // Config diff_id is the uncompressed tar digest.
        let cfg: serde_json::Value = serde_json::from_slice(&img.config).unwrap();
        let mut raw = Vec::new();
        open_layer(layer, Arc::default())
            .unwrap()
            .read_to_end(&mut raw)
            .unwrap();
        assert_eq!(
            cfg["rootfs"]["diff_ids"][0],
            crate::digest::sha256_bytes(&raw)
        );
        assert!(crate::media_types::is_container_disk_config(&cfg));

        let out = d.path().join("disk.qcow2");
        assert!(extract_disk(layer, &out).unwrap());
        assert_eq!(std::fs::read(&out).unwrap(), payload);

        // Deterministic: packing again yields the same digest.
        let again = pack(&disk, "arm64", &d.path().join("out2")).unwrap();
        assert_eq!(again.layers[0].0.digest, desc.digest);
    }

    #[test]
    fn extract_reports_missing_disk() {
        let d = tempfile::tempdir().unwrap();
        let layer = d.path().join("l.tar");
        let mut b = tar::Builder::new(std::fs::File::create(&layer).unwrap());
        let mut h = tar::Header::new_gnu();
        h.set_size(1);
        h.set_mode(0o644);
        b.append_data(&mut h, "etc/hostname", &b"x"[..]).unwrap();
        b.finish().unwrap();
        assert!(!extract_disk(&layer, &d.path().join("o")).unwrap());
    }
}
