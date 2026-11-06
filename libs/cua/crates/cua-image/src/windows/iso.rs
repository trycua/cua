//! Finding the Windows ISO a build installs from.
//!
//! Order: the ISO you name (`--iso`), then the cached one, then Microsoft's
//! media (the Evaluation Center, or the consumer download for ARM64 Windows 11) downloaded into the cache (resumable, the digest of what
//! arrived recorded next to it and checked on every later use).
//! Microsoft publishes no checksum for its evaluation ISOs, so the first
//! download is trusted and every later use is held to it; pass `sha256` to
//! pin one yourself.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::table::{Media, WindowsArch, WindowsVersion};
use crate::error::{ImageError, Result};

/// Where the ISO download sits in the whole build's progress (0 to 1).
pub const ISO_SPAN: (f64, f64) = (0.01, 0.40);

/// A real Windows ISO is gigabytes; anything under this is an error page.
pub const MIN_ISO_BYTES: u64 = 1 << 30;

/// Where the ISO came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IsoOrigin {
    /// The path the user gave.
    User,
    /// An earlier download.
    Cached,
    /// Downloaded now.
    Downloaded,
}

/// A usable ISO.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedIso {
    pub path: PathBuf,
    pub origin: IsoOrigin,
}

/// What is asked of [`resolve_iso`].
pub struct IsoRequest<'a> {
    pub version: &'a WindowsVersion,
    pub arch: WindowsArch,
    /// `--iso <path>`.
    pub user_iso: Option<&'a Path>,
    /// The ISO cache directory (`$CUA_HOME/images/windows/iso`).
    pub cache_dir: &'a Path,
    /// A checksum the ISO must have (`--iso-sha256`).
    pub sha256: Option<&'a str>,
}

/// The network side of resolution, so tests need none.
#[async_trait]
pub trait IsoFetcher: Send + Sync {
    /// The Evaluation Center page for `slug`.
    async fn eval_page(&self, slug: &str) -> Result<String>;
    /// Where a go.microsoft.com link ends up.
    async fn final_url(&self, link: &str) -> Result<String>;
    /// A web page or API answer as text, with an optional `Referer`
    /// (Microsoft's download service). Offline by default.
    async fn get_text(&self, url: &str, _referer: Option<&str>) -> Result<String> {
        Err(ImageError::Registry(format!("no network for {url}")))
    }
    /// The size of what `url` serves, when it can be told.
    async fn size_of(&self, _url: &str) -> Option<u64> {
        None
    }
    /// Downloads `url` to `to` (resuming `to` + `.partial`), reporting bytes.
    async fn download(
        &self,
        url: &str,
        to: &Path,
        on_bytes: &mut (dyn FnMut(u64) + Send),
    ) -> Result<()>;
}

/// The cache path of a version's evaluation ISO.
pub fn cache_path(cache_dir: &Path, v: &WindowsVersion, arch: WindowsArch) -> PathBuf {
    cache_dir.join(format!("windows-{}-{}.iso", v.id, arch.as_str()))
}

fn sidecar(iso: &Path) -> PathBuf {
    let mut p = iso.as_os_str().to_owned();
    p.push(".sha256");
    PathBuf::from(p)
}

/// The ISO you registered for a version (`cua sb create windows:11 --iso
/// <path>` remembers it, so the daemon that builds finds it): the pointer
/// file next to the cache, when its target still exists.
pub fn registered(cache_dir: &Path, v: &WindowsVersion, arch: WindowsArch) -> Option<PathBuf> {
    let text = std::fs::read_to_string(pointer(cache_dir, v, arch)).ok()?;
    let p = PathBuf::from(text.trim());
    p.is_file().then_some(p)
}

/// Remembers `path` as the ISO for a version, after checking it.
pub fn register(
    cache_dir: &Path,
    v: &WindowsVersion,
    arch: WindowsArch,
    path: &Path,
) -> Result<()> {
    check_user_iso(path, arch)?;
    let abs = std::fs::canonicalize(path)?;
    std::fs::create_dir_all(cache_dir)?;
    std::fs::write(
        pointer(cache_dir, v, arch),
        abs.to_string_lossy().trim_start_matches(r"\?\").as_bytes(),
    )?;
    Ok(())
}

fn pointer(cache_dir: &Path, v: &WindowsVersion, arch: WindowsArch) -> PathBuf {
    cache_dir.join(format!("windows-{}-{}.path", v.id, arch.as_str()))
}

/// SHA-256 of a file, lowercase hex.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Checks that `path` looks like an ISO for `arch`: it exists, is big enough,
/// carries the ISO 9660 signature, and its name does not say it is for the
/// other architecture (`arm64`/`a64` against an amd64 build, `x64`/`amd64`
/// against an arm64 one).
pub fn check_user_iso(path: &Path, arch: WindowsArch) -> Result<()> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let bad = |why: String| ImageError::Reference(path.display().to_string(), why);
    let meta = std::fs::metadata(path)
        .map_err(|_| bad("no such file; pass the path of a Windows ISO you downloaded".into()))?;
    if !meta.is_file() {
        return Err(bad("that is not a file".into()));
    }
    if meta.len() < MIN_ISO_BYTES {
        return Err(bad(format!(
            "{} bytes is too small for a Windows ISO",
            meta.len()
        )));
    }
    let mut f = std::fs::File::open(path)?;
    let mut sig = [0u8; 5];
    f.seek(SeekFrom::Start(0x8001))?;
    f.read_exact(&mut sig)?;
    if &sig != b"CD001" {
        return Err(bad("not an ISO 9660 image".into()));
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let arm = name.contains("arm64") || name.contains("a64");
    let x64 = name.contains("x64") || name.contains("amd64");
    match arch {
        WindowsArch::Amd64 if arm && !x64 => Err(bad(
            "this looks like an ARM64 ISO, but this host builds x64 Windows".into(),
        )),
        WindowsArch::Arm64 if x64 && !arm => Err(bad(
            "this looks like an x64 ISO, but this host builds ARM64 Windows".into(),
        )),
        _ => Ok(()),
    }
}

/// The go.microsoft.com ISO links on an Evaluation Center page for `culture`
/// (`en-us`), in page order.
pub fn eval_links(html: &str, culture: &str) -> Vec<String> {
    let needle = "https://go.microsoft.com/fwlink/";
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find(needle) {
        let tail = &rest[i..];
        let end = tail.find(['"', '\'', '<', ' ', ')']).unwrap_or(tail.len());
        let link = tail[..end].replace("&amp;", "&");
        if link.contains("LinkID=") || link.contains("linkid=") {
            let want = format!("culture={culture}");
            if link.to_ascii_lowercase().contains(&want) && !out.contains(&link) {
                out.push(link);
            }
        }
        rest = &tail[end.min(tail.len())..];
    }
    out
}

/// The link for `v`'s ISO: the first one on its page, else its known link id.
pub async fn eval_link(v: &WindowsVersion, fetcher: &dyn IsoFetcher) -> Result<String> {
    let page = fetcher.eval_page(v.eval_slug).await;
    if let Ok(html) = &page
        && let Some(l) = eval_links(html, "en-us").into_iter().next()
    {
        return Ok(l);
    }
    match v.eval_linkid {
        Some(id) => Ok(format!(
            "https://go.microsoft.com/fwlink/?linkid={id}&clcid=0x409&culture=en-us&country=us"
        )),
        None => Err(ImageError::Build {
            step: "windows iso".into(),
            detail: format!(
                "could not find the {} download on Microsoft's Evaluation Center ({}). Download the ISO yourself from \
                 https://www.microsoft.com/evalcenter and pass it with --iso <path>",
                v.name,
                page.err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "the page layout changed".into())
            ),
        }),
    }
}

/// Downloads `url` to `to`, reporting how far it is to whoever watches the
/// create (bytes, rate and time left) under the phase of a first-time
/// download.
pub(crate) async fn download_reporting(
    fetcher: &dyn IsoFetcher,
    url: &str,
    to: &Path,
    label: &str,
    span: (f64, f64),
    on_bytes: &mut (dyn FnMut(u64) + Send),
) -> Result<()> {
    use cua_vmm::progress::{Meter, Phase, Progress, report};
    let total = fetcher.size_of(url).await.unwrap_or(0);
    let (mut done, mut meter) = (0u64, Meter::new());
    report(
        Progress::phase(Phase::Pulling)
            .detail(label)
            .fraction(span.0),
    );
    fetcher
        .download(url, to, &mut |n| {
            done += n;
            on_bytes(n);
            if let Some(t) = meter.sample(done, total) {
                let mut p = Progress::phase(Phase::Pulling).detail(label);
                // Where this download sits in the whole build, so the bar of the
                // build only ever moves forward.
                if total > 0 {
                    p = p.fraction(span.0 + (span.1 - span.0) * done as f64 / total as f64);
                }
                p.bytes = Some(t);
                report(p);
            }
        })
        .await
}

/// Resolves the ISO to install from. See the module docs for the order.
pub async fn resolve_iso(
    req: &IsoRequest<'_>,
    fetcher: &dyn IsoFetcher,
    on_bytes: &mut (dyn FnMut(u64) + Send),
) -> Result<ResolvedIso> {
    req.version.check_arch(req.arch)?;
    if let Some(p) = req.user_iso {
        check_user_iso(p, req.arch)?;
        if let Some(want) = req.sha256 {
            verify(p, want)?;
        }
        return Ok(ResolvedIso {
            path: p.to_path_buf(),
            origin: IsoOrigin::User,
        });
    }
    let cached = cache_path(req.cache_dir, req.version, req.arch);
    if cached.is_file() {
        match verify_cached(&cached, req.sha256) {
            Ok(()) => {
                return Ok(ResolvedIso {
                    path: cached,
                    origin: IsoOrigin::Cached,
                });
            }
            Err(e) => {
                tracing::warn!("cached {}: {e}; downloading it again", cached.display());
                let _ = std::fs::remove_file(&cached);
                let _ = std::fs::remove_file(sidecar(&cached));
            }
        }
    }
    std::fs::create_dir_all(req.cache_dir)?;
    // The checksum Microsoft publishes for the file, when it publishes one.
    let mut published: Option<String> = None;
    match req.version.media(req.arch) {
        Media::Evaluation => {
            let link = eval_link(req.version, fetcher).await?;
            let url = fetcher.final_url(&link).await.unwrap_or(link);
            let label = format!("Downloading the {} ISO", req.version.name);
            download_reporting(fetcher, &url, &cached, &label, ISO_SPAN, on_bytes).await?;
        }
        Media::Consumer(slug) => {
            let m = super::msdl::resolve(fetcher, slug).await?;
            published = m.sha256;
            let label = format!("Downloading the {} ISO", req.version.name);
            download_reporting(fetcher, &m.url, &cached, &label, ISO_SPAN, on_bytes).await?;
        }
        Media::None => {
            return Err(ImageError::Build {
                step: "windows iso".into(),
                detail: format!(
                    "Microsoft publishes no {} ISO for {}. Download one yourself and pass it with --iso <path>",
                    req.arch.as_str(),
                    req.version.name,
                ),
            });
        }
    }
    let digest = sha256_file(&cached)?;
    // Microsoft's own value is checked too: a truncated or altered download
    // is never installed from.
    if let Some(want) = published.as_deref()
        && !want.eq_ignore_ascii_case(&digest)
    {
        let _ = std::fs::remove_file(&cached);
        return Err(ImageError::Digest {
            what: format!(
                "{} ISO (against the checksum on Microsoft's download page)",
                req.version.name
            ),
            expected: want.to_string(),
            actual: digest,
        });
    }
    if let Some(want) = req.sha256
        && !want.eq_ignore_ascii_case(&digest)
    {
        let _ = std::fs::remove_file(&cached);
        return Err(ImageError::Digest {
            what: format!("{} ISO", req.version.name),
            expected: want.to_string(),
            actual: digest,
        });
    }
    std::fs::write(sidecar(&cached), &digest)?;
    Ok(ResolvedIso {
        path: cached,
        origin: IsoOrigin::Downloaded,
    })
}

fn verify(path: &Path, want: &str) -> Result<()> {
    let got = sha256_file(path)?;
    if got.eq_ignore_ascii_case(want.trim()) {
        Ok(())
    } else {
        Err(ImageError::Digest {
            what: path.display().to_string(),
            expected: want.trim().to_string(),
            actual: got,
        })
    }
}

/// A cached ISO must be intact: it matches the pinned checksum, or the one
/// recorded when it was downloaded. One with neither is not trusted.
fn verify_cached(path: &Path, pinned: Option<&str>) -> Result<()> {
    if std::fs::metadata(path)?.len() < MIN_ISO_BYTES {
        return Err(ImageError::Spec("too small".into()));
    }
    match pinned {
        Some(want) => verify(path, want),
        None => match std::fs::read_to_string(sidecar(path)) {
            Ok(recorded) => verify(path, recorded.trim()),
            Err(_) => Err(ImageError::Spec("no recorded checksum".into())),
        },
    }
}

/// The real network: Microsoft's pages, and cua's resumable downloader.
pub struct HttpFetcher;

#[async_trait]
impl IsoFetcher for HttpFetcher {
    async fn eval_page(&self, slug: &str) -> Result<String> {
        let url = format!("https://www.microsoft.com/en-us/evalcenter/download-{slug}");
        Ok(cua_vmm::fetch::get_text(&url, std::time::Duration::from_secs(30)).await?)
    }

    async fn final_url(&self, link: &str) -> Result<String> {
        Ok(cua_vmm::fetch::final_url(link, std::time::Duration::from_secs(30)).await?)
    }

    async fn size_of(&self, url: &str) -> Option<u64> {
        cua_vmm::fetch::content_length(url, std::time::Duration::from_secs(30)).await
    }

    async fn get_text(&self, url: &str, referer: Option<&str>) -> Result<String> {
        Ok(cua_vmm::fetch::get_text_from(url, referer, std::time::Duration::from_secs(30)).await?)
    }

    async fn download(
        &self,
        url: &str,
        to: &Path,
        on_bytes: &mut (dyn FnMut(u64) + Send),
    ) -> Result<()> {
        cua_vmm::fetch::fetch(
            &cua_vmm::fetch::Fetch {
                url,
                bearer: None,
                size: None,
                sha256: "",
            },
            to,
            &cua_vmm::fetch::Policy::default(),
            on_bytes,
        )
        .await
        .map_err(|e| ImageError::Vmm(e.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::table::lookup;
    use std::sync::Mutex;

    /// An ISO-shaped sparse file: the signature at 0x8001, past 1 GiB.
    fn fake_iso(path: &Path) {
        use std::io::{Seek as _, SeekFrom, Write as _};
        let mut f = std::fs::File::create(path).unwrap();
        f.seek(SeekFrom::Start(0x8001)).unwrap();
        f.write_all(b"CD001").unwrap();
        f.set_len(MIN_ISO_BYTES + 10).unwrap();
    }

    #[derive(Default)]
    struct Fake {
        page: Option<String>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl IsoFetcher for Fake {
        async fn eval_page(&self, slug: &str) -> Result<String> {
            self.calls.lock().unwrap().push(format!("page {slug}"));
            self.page
                .clone()
                .ok_or_else(|| ImageError::Registry("offline".into()))
        }
        async fn final_url(&self, link: &str) -> Result<String> {
            self.calls.lock().unwrap().push(format!("resolve {link}"));
            Ok("https://download.example/ws.iso".into())
        }
        async fn download(
            &self,
            url: &str,
            to: &Path,
            on: &mut (dyn FnMut(u64) + Send),
        ) -> Result<()> {
            self.calls.lock().unwrap().push(format!("get {url}"));
            fake_iso(to);
            on(5);
            Ok(())
        }
    }

    const PAGE: &str = r#"<a href="https://go.microsoft.com/fwlink/p/?LinkID=111&amp;clcid=0x409&amp;culture=en-us&amp;country=US">64-bit</a>
        <a href="https://go.microsoft.com/fwlink/p/?LinkID=222&amp;clcid=0x407&amp;culture=de-de&amp;country=DE">de</a>"#;

    fn req<'a>(v: &'a str, user: Option<&'a Path>, dir: &'a Path) -> IsoRequest<'a> {
        IsoRequest {
            version: lookup(v).unwrap(),
            arch: WindowsArch::Amd64,
            user_iso: user,
            cache_dir: dir,
            sha256: None,
        }
    }

    #[test]
    fn links_are_the_requested_culture_unescaped() {
        let l = eval_links(PAGE, "en-us");
        assert_eq!(
            l,
            ["https://go.microsoft.com/fwlink/p/?LinkID=111&clcid=0x409&culture=en-us&country=US"]
        );
        assert!(eval_links(PAGE, "fr-fr").is_empty());
    }

    #[tokio::test]
    async fn your_iso_wins_and_nothing_is_fetched() {
        let d = tempfile::tempdir().unwrap();
        let iso = d.path().join("my.iso");
        fake_iso(&iso);
        let f = Fake::default();
        let r = resolve_iso(&req("2025", Some(&iso), d.path()), &f, &mut |_| {})
            .await
            .unwrap();
        assert_eq!((r.path, r.origin), (iso, IsoOrigin::User));
        assert!(f.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_download_is_cached_and_the_second_use_needs_no_network() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake {
            page: Some(PAGE.into()),
            ..Default::default()
        };
        let mut bytes = 0;
        let a = resolve_iso(&req("2025", None, d.path()), &f, &mut |n| bytes += n)
            .await
            .unwrap();
        assert_eq!(a.origin, IsoOrigin::Downloaded);
        assert_eq!(bytes, 5);
        assert!(a.path.ends_with("windows-2025-amd64.iso"));
        let n = f.calls.lock().unwrap().len();
        assert_eq!(n, 3, "page, resolve, get: {:?}", f.calls.lock().unwrap());
        let b = resolve_iso(&req("2025", None, d.path()), &f, &mut |_| {})
            .await
            .unwrap();
        assert_eq!(b.origin, IsoOrigin::Cached);
        assert_eq!(f.calls.lock().unwrap().len(), n);
    }

    #[tokio::test]
    async fn a_cached_iso_that_changed_is_downloaded_again() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake {
            page: Some(PAGE.into()),
            ..Default::default()
        };
        let a = resolve_iso(&req("2025", None, d.path()), &f, &mut |_| {})
            .await
            .unwrap();
        // Same size, different bytes: the recorded digest no longer matches.
        use std::io::{Seek as _, SeekFrom, Write as _};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&a.path)
            .unwrap();
        file.seek(SeekFrom::Start(100)).unwrap();
        file.write_all(b"x").unwrap();
        drop(file);
        let b = resolve_iso(&req("2025", None, d.path()), &f, &mut |_| {})
            .await
            .unwrap();
        assert_eq!(b.origin, IsoOrigin::Downloaded);
    }

    #[tokio::test]
    async fn a_wrong_pinned_checksum_deletes_the_download() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake {
            page: Some(PAGE.into()),
            ..Default::default()
        };
        let mut r = req("2025", None, d.path());
        r.sha256 = Some("00");
        let e = resolve_iso(&r, &f, &mut |_| {}).await.unwrap_err();
        assert!(matches!(e, ImageError::Digest { .. }), "{e}");
        assert!(!cache_path(d.path(), r.version, r.arch).exists());
    }

    #[tokio::test]
    async fn without_the_page_the_known_link_is_used_else_the_error_names_iso() {
        let d = tempfile::tempdir().unwrap();
        let f = Fake::default(); // offline
        let r = resolve_iso(&req("2025", None, d.path()), &f, &mut |_| {})
            .await
            .unwrap();
        assert_eq!(r.origin, IsoOrigin::Downloaded);
        assert!(
            f.calls.lock().unwrap()[1].contains("linkid=2345730"),
            "{:?}",
            f.calls.lock().unwrap()
        );
        let e = resolve_iso(&req("2022", None, d.path()), &f, &mut |_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("--iso"), "{e}");
    }

    /// Microsoft's download service, answered from memory: the page, the
    /// session calls, the SKU list and the link.
    struct Microsoft {
        sha256: String,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl IsoFetcher for Microsoft {
        async fn eval_page(&self, _: &str) -> Result<String> {
            Err(ImageError::Registry(
                "no evaluation page in this test".into(),
            ))
        }
        async fn final_url(&self, l: &str) -> Result<String> {
            Ok(l.into())
        }
        async fn get_text(&self, url: &str, _referer: Option<&str>) -> Result<String> {
            self.calls.lock().unwrap().push(url.to_string());
            Ok(if url.contains("software-download/windows11arm64") {
                format!(
                    r#"<option value="3816">Windows 11 (multi-edition ISO for Arm64)</option><table><tr><td>English  64-bit</td><td>{}</td></tr></table>"#,
                    self.sha256.to_ascii_uppercase()
                )
            } else if url.contains("getskuinformationbyproductedition") {
                assert!(url.contains("productEditionId=3816"), "{url}");
                r#"{"Skus":[{"Id":"27200","Language":"English"}]}"#.into()
            } else if url.contains("GetProductDownloadLinksBySku") {
                assert!(url.contains("SKU=27200"), "{url}");
                r#"{"ProductDownloadOptions":[{"Uri":"https://software.download.prss.microsoft.com/w11arm.iso?t=1","ProductDisplayName":"Windows 11 ARM 64"}]}"#.into()
            } else {
                String::new()
            })
        }
        async fn size_of(&self, _: &str) -> Option<u64> {
            Some(MIN_ISO_BYTES + 10)
        }
        async fn download(
            &self,
            url: &str,
            to: &Path,
            on: &mut (dyn FnMut(u64) + Send),
        ) -> Result<()> {
            self.calls.lock().unwrap().push(format!("get {url}"));
            fake_iso(to);
            on(MIN_ISO_BYTES + 10);
            Ok(())
        }
    }

    fn arm_req<'a>(dir: &'a Path) -> IsoRequest<'a> {
        let mut r = req("11", None, dir);
        r.arch = WindowsArch::Arm64;
        r
    }

    #[tokio::test]
    async fn arm_windows_11_is_downloaded_from_microsoft_and_checked_against_its_checksum() {
        let d = tempfile::tempdir().unwrap();
        // The digest of the fake ISO the download writes.
        let probe = d.path().join("probe.iso");
        fake_iso(&probe);
        let sha = sha256_file(&probe).unwrap();
        let ms = Microsoft {
            sha256: sha,
            calls: Mutex::new(vec![]),
        };
        let r = resolve_iso(&arm_req(d.path()), &ms, &mut |_| {})
            .await
            .unwrap();
        assert_eq!(r.origin, IsoOrigin::Downloaded);
        assert!(r.path.ends_with("windows-11-arm64.iso"));
        let calls = ms.calls.lock().unwrap().clone();
        assert!(
            calls
                .iter()
                .any(|c| c.contains("software.download.prss.microsoft.com"))
        );
        // The second use is the cache: no network at all.
        let again = Microsoft {
            sha256: String::new(),
            calls: Mutex::new(vec![]),
        };
        let r = resolve_iso(&arm_req(d.path()), &again, &mut |_| {})
            .await
            .unwrap();
        assert_eq!(r.origin, IsoOrigin::Cached);
        assert!(again.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_download_that_does_not_match_microsofts_checksum_is_never_kept() {
        let d = tempfile::tempdir().unwrap();
        let ms = Microsoft {
            sha256: "00".repeat(32),
            calls: Mutex::new(vec![]),
        };
        let e = resolve_iso(&arm_req(d.path()), &ms, &mut |_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("Microsoft"), "{e}");
        assert!(!cache_path(d.path(), lookup("11").unwrap(), WindowsArch::Arm64).exists());
    }

    #[tokio::test]
    async fn offline_arm_windows_11_says_to_pass_an_iso() {
        let d = tempfile::tempdir().unwrap();
        let e = resolve_iso(&arm_req(d.path()), &Fake::default(), &mut |_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("Microsoft"), "{e}");
    }

    #[tokio::test]
    async fn a_version_microsoft_offers_nothing_for_asks_for_your_iso() {
        let d = tempfile::tempdir().unwrap();
        let mut r = req("2025", None, d.path());
        r.arch = WindowsArch::Arm64;
        // Server has no ARM64 build at all: refused before any network.
        assert!(
            resolve_iso(&r, &Fake::default(), &mut |_| {})
                .await
                .is_err()
        );
    }

    #[test]
    fn a_registered_iso_is_remembered_until_it_goes() {
        let d = tempfile::tempdir().unwrap();
        let v = lookup("11").unwrap();
        let iso = d.path().join("mine.iso");
        fake_iso(&iso);
        let cache = d.path().join("cache");
        assert_eq!(registered(&cache, v, WindowsArch::Amd64), None);
        register(&cache, v, WindowsArch::Amd64, &iso).unwrap();
        let got = registered(&cache, v, WindowsArch::Amd64).unwrap();
        assert_eq!(
            std::fs::canonicalize(&got).unwrap(),
            std::fs::canonicalize(&iso).unwrap()
        );
        assert!(register(&cache, v, WindowsArch::Amd64, &d.path().join("none.iso")).is_err());
        std::fs::remove_file(&iso).unwrap();
        assert_eq!(registered(&cache, v, WindowsArch::Amd64), None);
    }

    #[test]
    fn user_iso_checks() {
        let d = tempfile::tempdir().unwrap();
        let missing = check_user_iso(&d.path().join("no.iso"), WindowsArch::Amd64);
        assert!(missing.unwrap_err().to_string().contains("no such file"));
        let small = d.path().join("small.iso");
        std::fs::write(&small, b"hello").unwrap();
        assert!(check_user_iso(&small, WindowsArch::Amd64).is_err());
        let arm = d.path().join("Win11_ARM64.iso");
        fake_iso(&arm);
        assert!(check_user_iso(&arm, WindowsArch::Arm64).is_ok());
        let e = check_user_iso(&arm, WindowsArch::Amd64)
            .unwrap_err()
            .to_string();
        assert!(e.contains("ARM64 ISO"), "{e}");
        let x64 = d.path().join("Win11_x64.iso");
        fake_iso(&x64);
        assert!(check_user_iso(&x64, WindowsArch::Arm64).is_err());
        let notiso = d.path().join("notiso.iso");
        std::fs::File::create(&notiso)
            .unwrap()
            .set_len(MIN_ISO_BYTES)
            .unwrap();
        assert!(check_user_iso(&notiso, WindowsArch::Amd64).is_err());
    }
}
