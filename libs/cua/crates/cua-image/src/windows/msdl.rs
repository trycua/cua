//! Microsoft's software-download service: the official, account-free way to
//! get the Windows 11 ISO for ARM64 (Microsoft publishes no ARM64 evaluation
//! ISO). It is what the "Download Windows 11" web pages themselves call.
//!
//! 1. The download page lists the product edition id of the current release
//!    (`<option value="3816">Windows 11 (multi-edition ISO for Arm64)`); ids
//!    change with every release, so they are read, never hard-coded.
//! 2. A session id (a random GUID) is registered with Microsoft's
//!    anti-abuse endpoints, which only need to be called, not answered.
//! 3. `getskuinformationbyproductedition` lists the languages (SKU ids) and
//!    `GetProductDownloadLinksBySku` returns a link on
//!    `software.download.prss.microsoft.com`, valid for 24 hours.
//! 4. The same page publishes the SHA-256 of every language's ISO, so the
//!    download is checked against Microsoft's own value.
//!
//! Nothing here stores, mirrors or republishes the ISO: it is downloaded from
//! Microsoft to this computer, for the person who asked for it.

use super::iso::IsoFetcher;
use crate::error::{ImageError, Result};

const API: &str = "https://www.microsoft.com/software-download-connector/api";
const PROFILE: &str = "606624d44113";
const ORG_ID: &str = "y6jn8c31";
const INSTANCE_ID: &str = "560dc9f3-1aa5-4a2f-b63c-9e18f8d0e175";
const LANGUAGE: &str = "English";

/// The words of a refusal by Microsoft's per-network limit: the app and the
/// cooldown recognise a failure by them.
pub const LIMITED: &str = "Microsoft is limiting Windows downloads from this network";

/// Whether the service's error text is that limit ("Sentinel marked this
/// request as rejected", or a ban notice).
pub fn is_rate_limit(text: &str) -> bool {
    let l = text.to_ascii_lowercase();
    l.contains("sentinel") || l.contains("banned") || l.contains("rejected")
}

/// A consumer ISO Microsoft serves: where from, and the checksum it publishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsumerMedia {
    pub url: String,
    /// SHA-256 from the download page, lowercase hex, when the page has it.
    pub sha256: Option<String>,
    /// Microsoft's name for the release ("Windows 11 ARM 64 26H2 26300.9457").
    pub product: String,
}

fn fail(detail: impl Into<String>) -> ImageError {
    ImageError::Build {
        step: "windows iso".into(),
        detail: detail.into(),
    }
}

/// The product edition id on a download page.
pub fn edition_id(html: &str) -> Option<u32> {
    let mut rest = html;
    while let Some(i) = rest.find("<option value=\"") {
        let tail = &rest[i + "<option value=\"".len()..];
        let end = tail.find('"')?;
        if let Ok(n) = tail[..end].parse::<u32>()
            && n > 0
        {
            return Some(n);
        }
        rest = &tail[end..];
    }
    None
}

/// The SKU id of `LANGUAGE` in a `getskuinformationbyproductedition` answer.
pub fn sku_id(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v.get("Skus")?.as_array()?.iter().find_map(|s| {
        (s.get("Language")?.as_str()? == LANGUAGE)
            .then(|| s.get("Id")?.as_str().map(String::from))?
    })
}

/// Whether `url` is an https link on a Microsoft domain (`microsoft.com` or a
/// subdomain). The ISO is only ever fetched from there: the link comes out of
/// a web service's JSON answer, so it is checked, not trusted.
pub fn is_microsoft_https(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains(['@', '\\', ' ']) {
        return false;
    }
    let host = authority
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    host == "microsoft.com" || host.ends_with(".microsoft.com")
}

/// The download link and product name in a `GetProductDownloadLinksBySku` answer.
pub fn download_link(json: &str) -> std::result::Result<(String, String), String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if let Some(e) = v
        .pointer("/Errors/0/Value")
        .or_else(|| v.pointer("/ValidationContainer/Errors/0/Value"))
        .and_then(|e| e.as_str())
    {
        return Err(e.to_string());
    }
    let o = v
        .get("ProductDownloadOptions")
        .and_then(|o| o.as_array())
        .and_then(|a| a.first())
        .ok_or("no download options in the answer")?;
    let url = o
        .get("Uri")
        .and_then(|u| u.as_str())
        .ok_or("no link in the answer")?;
    if !is_microsoft_https(url) {
        return Err(format!(
            "the download link is not an https link on a Microsoft domain ({url:?}); refusing it"
        ));
    }
    let name = o
        .get("ProductDisplayName")
        .and_then(|u| u.as_str())
        .unwrap_or("Windows");
    Ok((url.to_string(), name.to_string()))
}

/// The English 64-bit ISO's SHA-256 from the hash table on a download page.
pub fn page_sha256(html: &str) -> Option<String> {
    let mut rest = html;
    while let Some(i) = rest.find("<tr><td>") {
        let tail = &rest[i + "<tr><td>".len()..];
        let end = tail.find("</tr>")?;
        let row = &tail[..end];
        rest = &tail[end..];
        let (label, hash) = row.split_once("</td><td>")?;
        let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
        let hash = hash.trim_end_matches("</td>").trim();
        if label == "English 64-bit"
            && hash.len() == 64
            && hash.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

fn guid() -> String {
    // 16 random bytes shaped as a version-4 GUID.
    let mut b = [0u8; 16];
    use std::io::Read as _;
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_err()
    {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        b = n.to_le_bytes();
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Resolves the current consumer ISO of a download page (`windows11arm64`).
pub async fn resolve(fetcher: &dyn IsoFetcher, page_slug: &str) -> Result<ConsumerMedia> {
    let page_url = format!("https://www.microsoft.com/en-us/software-download/{page_slug}");
    let page = fetcher
        .get_text(&page_url, None)
        .await
        .map_err(|e| fail(format!("could not read Microsoft's download page ({e})")))?;
    let edition = edition_id(&page).ok_or_else(|| {
        fail("Microsoft's download page has no edition to download (the page layout changed)")
    })?;
    let sid = guid();
    // Registration calls: their answers do not matter, only that they were made.
    let _ = fetcher
        .get_text(
            &format!("https://vlscppe.microsoft.com/tags?org_id={ORG_ID}&session_id={sid}"),
            None,
        )
        .await;
    if let Ok(js) = fetcher
        .get_text(
            &format!(
                "https://ov-df.microsoft.com/mdt.js?instanceId={INSTANCE_ID}&PageId=si&session_id={sid}"
            ),
            None,
        )
        .await
    {
        // `w` is an upper-case hex string after `?w=` or `&w=`; `rticks` a number.
        let after = |keys: &[&str]| {
            keys.iter().find_map(|key| {
                let i = js.find(key)? + key.len();
                let t = &js[i..];
                let e = t.find(|c: char| !c.is_ascii_alphanumeric())?;
                (e > 0).then(|| t[..e].to_string())
            })
        };
        if let (Some(w), Some(r)) = (after(&["?w=", "&w="]), after(&["rticks=\"+", "rticks=\""])) {
            let mdt = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let _ = fetcher
                .get_text(
                    &format!(
                        "https://ov-df.microsoft.com/?session_id={sid}&CustomerId={INSTANCE_ID}&PageId=si&w={w}&mdt={mdt}&rticks={r}"
                    ),
                    None,
                )
                .await;
        }
    }
    let skus = fetcher
        .get_text(
            &format!(
                "{API}/getskuinformationbyproductedition?profile={PROFILE}&productEditionId={edition}&SKU=undefined&friendlyFileName=undefined&Locale=en-US&sessionID={sid}"
            ),
            None,
        )
        .await
        .map_err(|e| fail(format!("Microsoft's download service did not answer ({e})")))?;
    let sku = sku_id(&skus)
        .ok_or_else(|| fail("Microsoft's download service lists no English release"))?;
    let links = fetcher
        .get_text(
            &format!(
                "{API}/GetProductDownloadLinksBySku?profile={PROFILE}&productEditionId=undefined&SKU={sku}&friendlyFileName=undefined&Locale=en-US&sessionID={sid}"
            ),
            Some("https://www.microsoft.com/software-download/windows11"),
        )
        .await
        .map_err(|e| fail(format!("Microsoft's download service did not answer ({e})")))?;
    let (url, product) = download_link(&links).map_err(|e| {
        if is_rate_limit(&e) {
            return fail(format!("{LIMITED} ({e})"));
        }
        fail(format!(
            "Microsoft would not give a download link ({e}). It limits downloads per network; try again later, or download the ISO in a browser and pass it with --iso <path>"
        ))
    })?;
    Ok(ConsumerMedia {
        url,
        sha256: page_sha256(&page),
        product,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<select><option value="">Select Download</option><option value="3816">Windows 11 (multi-edition ISO for Arm64)</option></select>
<table><tbody><tr><th>Country Locale</th><th>Hash Code</th></tr><tr><td>English  International  64-bit</td><td>500BCB6F272B7C455156470EA06816F558F13B885723F84EE9EAB7534C7DEE61</td></tr><tr><td>English  64-bit</td><td>8992F2D2CFD2FA647E579E9EB31FC21F34DA8333C29B6170FDE17D2ED6623E9C</td></tr></tbody></table>"#;

    #[test]
    fn only_https_links_on_microsoft_domains_are_followed() {
        for ok in [
            "https://software.download.prss.microsoft.com/x.iso?t=1",
            "https://download.microsoft.com/a",
            "https://MICROSOFT.com/a",
        ] {
            assert!(is_microsoft_https(ok), "{ok}");
        }
        for bad in [
            "http://software.download.prss.microsoft.com/x.iso",
            "https://microsoft.com.evil.example/x.iso",
            "https://evil.example/microsoft.com/x.iso",
            "https://notmicrosoft.com/x.iso",
            "https://software.download.prss.microsoft.com@evil.example/x.iso",
            "https://evil.example\\@x.microsoft.com/",
            "ftp://x.microsoft.com/x",
            "//x.microsoft.com/x",
            "",
        ] {
            assert!(!is_microsoft_https(bad), "{bad}");
        }
        let evil = r#"{"ProductDownloadOptions":[{"Uri":"https://evil.example/w.iso","ProductDisplayName":"Windows 11"}]}"#;
        assert!(
            download_link(evil)
                .unwrap_err()
                .contains("Microsoft domain")
        );
    }

    #[test]
    fn the_edition_and_checksum_come_from_the_page() {
        assert_eq!(edition_id(PAGE), Some(3816));
        assert_eq!(
            page_sha256(PAGE).as_deref(),
            Some("8992f2d2cfd2fa647e579e9eb31fc21f34da8333c29b6170fde17d2ed6623e9c")
        );
        assert_eq!(edition_id("<p>no select</p>"), None);
        assert_eq!(
            page_sha256("<tr><td>German 64-bit</td><td>AA</td></tr>"),
            None
        );
    }

    #[test]
    fn the_english_sku_and_the_link_are_read_from_the_answers() {
        let skus =
            r#"{"Skus":[{"Id":"27163","Language":"Arabic"},{"Id":"27200","Language":"English"}]}"#;
        assert_eq!(sku_id(skus).as_deref(), Some("27200"));
        let ok = r#"{"ProductDownloadOptions":[{"Uri":"https://software.download.prss.microsoft.com/x.iso?t=1","ProductDisplayName":"Windows 11 ARM 64 26H2"}],"Errors":[]}"#;
        let (u, n) = download_link(ok).unwrap();
        assert!(u.starts_with("https://software.download.prss.microsoft.com/"));
        assert_eq!(n, "Windows 11 ARM 64 26H2");
        let banned = r#"{"Errors":[{"Key":"ErrorSettings.SentinelReject","Value":"Your IP address has been banned","Type":9}]}"#;
        assert!(download_link(banned).unwrap_err().contains("banned"));
    }

    #[test]
    fn guids_have_the_version_4_shape() {
        let g = guid();
        assert_eq!(g.len(), 36);
        assert_eq!(g.as_bytes()[14], b'4');
        assert_ne!(g, guid());
    }

    /// The real service, link and checksum only (nothing is downloaded). Run by
    /// hand from a network Microsoft does not limit:
    /// `cargo test -p cua-image --lib live_microsoft -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "needs the internet and a network Microsoft does not limit"]
    async fn live_microsoft_gives_a_link_and_a_checksum_for_windows_11_arm64() {
        match resolve(&super::super::iso::HttpFetcher, "windows11arm64").await {
            Ok(m) => {
                println!("{} {} {:?}", m.product, m.url, m.sha256);
                assert!(m.url.contains("prss.microsoft.com"), "{}", m.url);
                assert!(m.sha256.is_some());
            }
            Err(e) => {
                println!("{e}");
                assert!(e.to_string().contains(LIMITED), "{e}");
            }
        }
    }
}
