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
    /// A matching ISO the person had already saved (Downloads, or the cache
    /// folder), used because Microsoft would not give a link.
    Detected,
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
    /// Folders searched for an ISO the person saved themselves when Microsoft
    /// will not give a download link (see [`default_scan_dirs`]).
    pub scan_dirs: Vec<PathBuf>,
}

/// How long to wait after Microsoft's per-network limit before asking again.
/// Microsoft does not say; every request made inside the block extends it.
pub const COOLDOWN_SECS: u64 = 60 * 60;

/// The folders an ISO the person downloaded is looked for in: Downloads, and
/// `extra` (the cache folder).
pub fn default_scan_dirs(extra: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        v.push(PathBuf::from(home).join("Downloads"));
    }
    v.push(extra.to_path_buf());
    v
}

fn cooldown_file(cache_dir: &Path) -> PathBuf {
    cache_dir.join("microsoft-limited-until")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Seconds left of the wait after a rejection, when one is running.
pub fn cooldown_left(cache_dir: &Path) -> Option<u64> {
    let until: u64 = std::fs::read_to_string(cooldown_file(cache_dir))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    until.checked_sub(unix_now()).filter(|s| *s > 0)
}

fn start_cooldown(cache_dir: &Path) {
    let _ = std::fs::create_dir_all(cache_dir);
    let _ = std::fs::write(
        cooldown_file(cache_dir),
        (unix_now() + COOLDOWN_SECS).to_string(),
    );
}

/// "about 42 minutes", "about an hour".
fn wait_words(secs: u64) -> String {
    let mins = secs.div_ceil(60);
    if mins >= 55 {
        "about an hour".into()
    } else {
        format!("about {mins} minutes")
    }
}

/// The volume label of an ISO 9660 image (`CCCOMA_A64FRE_EN-US_DV9`), from
/// its primary volume descriptor.
pub fn volume_label(path: &Path) -> Option<String> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    // Type, "CD001", version, then the 32-byte system id and the 32-byte volume id.
    let mut d = [0u8; 72];
    f.seek(SeekFrom::Start(0x8000)).ok()?;
    f.read_exact(&mut d).ok()?;
    (&d[1..6] == b"CD001").then(|| String::from_utf8_lossy(&d[40..72]).trim().to_string())
}

/// A Windows ISO for `arch` the person saved in one of `dirs`: an ISO 9660
/// image over 3 GiB whose volume label carries the architecture Windows
/// media use (`A64` for ARM64, `X64`), newest first.
pub fn detect_iso(dirs: &[PathBuf], arch: WindowsArch) -> Option<PathBuf> {
    let tag = match arch {
        WindowsArch::Arm64 => "A64",
        WindowsArch::Amd64 => "X64",
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("iso")) {
                continue;
            }
            // Through links: a Downloads folder may hold an alias or a symlink.
            let Ok(m) = std::fs::metadata(&p) else {
                continue;
            };
            // Not our own cache copies, not a partial download, not a small disc.
            if !m.is_file() || m.len() < 3 << 30 {
                continue;
            }
            if volume_label(&p).is_some_and(|l| l.to_ascii_uppercase().contains(tag)) {
                found.push((m.modified().unwrap_or(std::time::UNIX_EPOCH), p));
            }
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().next().map(|(_, p)| p)
}

/// The ISO you saved yourself instead of the download Microsoft refused, or
/// the plain error that says when to retry and what to do meanwhile.
fn saved_or_limited(req: &IsoRequest<'_>, left: Option<u64>, slug: &str) -> Result<ResolvedIso> {
    let mut why_not = String::new();
    if let Some(p) = detect_iso(&req.scan_dirs, req.arch) {
        match check_user_iso(&p, req.arch) {
            Ok(()) => {
                // Remembered, so the next create needs nothing.
                if let Err(e) = register(req.cache_dir, req.version, req.arch, &p) {
                    tracing::warn!("could not remember {}: {e}", p.display());
                }
                tracing::info!(iso = %p.display(), "using the Windows ISO found on this computer");
                return Ok(ResolvedIso {
                    path: p,
                    origin: IsoOrigin::Detected,
                });
            }
            Err(e) => why_not = format!(" ({} was found but not used: {e})", p.display()),
        }
    }
    let wait = wait_words(left.unwrap_or(COOLDOWN_SECS));
    Err(ImageError::Build {
        step: "windows iso".into(),
        detail: format!(
            "{} right now{why_not}. Try again in {wait}, or download the Windows ISO yourself \
             (https://www.microsoft.com/software-download/{slug}) and save it in your Downloads folder: Cua finds it. \
             With the command line: cua sb create windows:{} --iso ~/Downloads/<the file>.iso",
            super::msdl::LIMITED,
            req.version.id
        ),
    })
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
            // Inside the wait after a rejection no request is made (each one
            // extends the block); a saved ISO is used if there is one.
            if let Some(left) = cooldown_left(req.cache_dir) {
                return saved_or_limited(req, Some(left), slug);
            }
            let m = match super::msdl::resolve(fetcher, slug).await {
                Ok(m) => m,
                Err(ImageError::Build { detail, .. }) if detail.contains(super::msdl::LIMITED) => {
                    start_cooldown(req.cache_dir);
                    return saved_or_limited(req, Some(COOLDOWN_SECS), slug);
                }
                Err(e) => return Err(with_iso_hint(e)),
            };
            published = m.sha256;
            let label = format!("Downloading the {} ISO", req.version.name);
            download_reporting(fetcher, &m.url, &cached, &label, ISO_SPAN, on_bytes)
                .await
                .map_err(with_iso_hint)?;
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

/// The way past a failed download, named in the CLI's own words.
fn with_iso_hint(e: ImageError) -> ImageError {
    match e {
        ImageError::Build { step, detail } => ImageError::Build {
            step,
            detail: if detail.contains("--iso") {
                detail
            } else {
                format!(
                    "{detail}. Download the Windows ISO from Microsoft yourself and pass it with: cua sb create <windows image> --iso <path to the .iso>"
                )
            },
        },
        d @ ImageError::Digest { .. } => d,
        other => with_iso_hint(ImageError::Build {
            step: "windows iso".into(),
            detail: other.to_string(),
        }),
    }
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
            scan_dirs: vec![],
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
        /// Answer the link request with Microsoft's per-network rejection.
        limited: bool,
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
            } else if url.contains("GetProductDownloadLinksBySku") && self.limited {
                r#"{"Errors":[{"Key":"ErrorSettings.SentinelReject","Value":"Sentinel marked this request as rejected.","Type":9}]}"#.into()
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
            limited: false,
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
            limited: false,
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
            limited: false,
        };
        let e = resolve_iso(&arm_req(d.path()), &ms, &mut |_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("Microsoft"), "{e}");
        assert!(!cache_path(d.path(), lookup("11").unwrap(), WindowsArch::Arm64).exists());
    }

    /// A sparse file shaped like a Windows ARM64 ISO: the volume label Windows
    /// media carry, over 3 GiB.
    fn windows_iso_saved(path: &Path, label: &str) {
        use std::io::{Seek as _, SeekFrom, Write as _};
        let mut f = std::fs::File::create(path).unwrap();
        f.seek(SeekFrom::Start(0x8000)).unwrap();
        let mut d = vec![b' '; 72];
        d[0] = 1;
        d[1..6].copy_from_slice(b"CD001");
        d[40..40 + label.len()].copy_from_slice(label.as_bytes());
        f.write_all(&d).unwrap();
        f.set_len((3 << 30) + 4096).unwrap();
    }

    fn limited_ms() -> Microsoft {
        Microsoft {
            sha256: String::new(),
            calls: Mutex::new(vec![]),
            limited: true,
        }
    }

    #[tokio::test]
    async fn a_rejection_by_microsoft_waits_and_asks_only_once() {
        let d = tempfile::tempdir().unwrap();
        let ms = limited_ms();
        let e = resolve_iso(&arm_req(d.path()), &ms, &mut |_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains(super::super::msdl::LIMITED), "{e}");
        assert!(e.contains("Try again in about an hour"), "{e}");
        assert!(e.contains("--iso ~/Downloads/"), "{e}");
        let asked = ms.calls.lock().unwrap().len();
        // Inside the wait: no request at all, and the time left is said.
        let again = limited_ms();
        let e2 = resolve_iso(&arm_req(d.path()), &again, &mut |_| {})
            .await
            .unwrap_err()
            .to_string();
        assert!(again.calls.lock().unwrap().is_empty(), "no new request");
        assert!(e2.contains("Try again in about"), "{e2}");
        assert!(asked > 0);
        // One request for a link per attempt.
        let links = ms
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.contains("GetProductDownloadLinksBySku"))
            .count();
        assert_eq!(links, 1);
    }

    #[tokio::test]
    async fn an_iso_saved_in_downloads_is_used_when_microsoft_refuses_and_remembered() {
        let d = tempfile::tempdir().unwrap();
        let downloads = d.path().join("Downloads");
        std::fs::create_dir(&downloads).unwrap();
        windows_iso_saved(
            &downloads.join("Win11_25H2_English_Arm64_v2.iso"),
            "CCCOMA_A64FRE_EN-US_DV9",
        );
        // An x64 disc and a small file are not it.
        windows_iso_saved(&downloads.join("Win11_x64.iso"), "CCCOMA_X64FRE_EN-US_DV9");
        std::fs::write(downloads.join("tiny.iso"), b"x").unwrap();
        let cache = d.path().join("cache");
        let mut r = arm_req(&cache);
        r.scan_dirs = vec![downloads.clone()];
        let got = resolve_iso(&r, &limited_ms(), &mut |_| {}).await.unwrap();
        assert_eq!(got.origin, IsoOrigin::Detected);
        assert!(got.path.ends_with("Win11_25H2_English_Arm64_v2.iso"));
        // Remembered for the next create.
        let v = lookup("11").unwrap();
        assert!(registered(&cache, v, WindowsArch::Arm64).is_some());
    }

    #[test]
    fn a_linked_iso_in_downloads_is_found_and_an_x64_one_is_not_taken_for_arm64() {
        let d = tempfile::tempdir().unwrap();
        let real = d.path().join("real.iso");
        windows_iso_saved(&real, "CCCOMA_A64FRE_EN-US_DV9");
        let dl = d.path().join("Downloads");
        std::fs::create_dir(&dl).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, dl.join("Win11_Arm64.iso")).unwrap();
        #[cfg(not(unix))]
        std::fs::copy(&real, dl.join("Win11_Arm64.iso")).unwrap();
        assert!(detect_iso(std::slice::from_ref(&dl), WindowsArch::Arm64).is_some());
        assert!(detect_iso(&[dl], WindowsArch::Amd64).is_none());
    }

    /// Against a real Windows ISO: `CUA_REAL_ISO_DIR=<folder with it> cargo test -- --ignored real_iso`.
    #[test]
    #[ignore = "needs a real Windows ISO"]
    fn real_iso_in_a_folder_is_detected_by_its_label() {
        let Some(dir) = std::env::var_os("CUA_REAL_ISO_DIR") else {
            return;
        };
        let dirs = [PathBuf::from(dir)];
        let found = detect_iso(&dirs, WindowsArch::Arm64);
        eprintln!("found {found:?}");
        let p = found.expect("an ARM64 ISO");
        eprintln!("label {:?}", volume_label(&p));
        check_user_iso(&p, WindowsArch::Arm64).unwrap();
        assert!(detect_iso(&dirs, WindowsArch::Amd64).is_none());
    }

    #[test]
    fn the_volume_label_is_read_from_the_iso() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.iso");
        windows_iso_saved(&p, "CCCOMA_A64FRE_EN-US_DV9");
        assert_eq!(volume_label(&p).as_deref(), Some("CCCOMA_A64FRE_EN-US_DV9"));
        assert_eq!(volume_label(&d.path().join("none.iso")), None);
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

    /// A failed download names the way past it in the CLI's own words.
    #[test]
    fn a_failed_download_points_at_the_iso_flag_once() {
        let e = with_iso_hint(ImageError::Build {
            step: "windows iso".into(),
            detail: "Microsoft's download service did not answer (timed out)".into(),
        });
        let text = e.to_string();
        assert!(text.contains("--iso <path to the .iso>"), "{text}");
        assert!(text.starts_with("build failed at windows iso: Microsoft's download service"));
        // Already says it: not added twice.
        assert_eq!(with_iso_hint(e).to_string(), text);
        // Another kind of error becomes the same step, with the hint.
        let io =
            with_iso_hint(ImageError::Io(std::io::Error::other("connection reset"))).to_string();
        assert!(
            io.contains("build failed at windows iso: ") && io.contains("--iso"),
            "{io}"
        );
        // A checksum mismatch keeps its own words.
        let d = with_iso_hint(ImageError::Digest {
            what: "x".into(),
            expected: "a".into(),
            actual: "b".into(),
        })
        .to_string();
        assert!(!d.contains("--iso"));
    }
}
