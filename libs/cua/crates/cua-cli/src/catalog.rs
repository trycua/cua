//! The sandbox image catalog: `libs/images/sandbox-images.json`, the list the
//! docs (os-image-catalog), the Spaces apps and the samples show. Embedded at
//! build time so `cua images ls` and the MCP `images_list` tool answer
//! offline with exactly what the docs publish.

use cua_sdk::CuaError;
use serde_json::{Value, json};
use std::io::Write;

use crate::util::{self, line};

/// The catalog file, as built into this binary.
pub const CATALOG_JSON: &str = include_str!("../../../../images/sandbox-images.json");

/// Browsers cua-driver's typed browser tools drive (Chrome DevTools Protocol).
const CDP_BROWSERS: &[&str] = &["chromium", "chrome", "edge"];

/// Filters for [`list`].
#[derive(Debug, Default, Clone)]
pub struct Filter {
    /// Include unpublished entries (benchmark images in progress).
    pub all: bool,
    /// Only this OS (`linux`, `windows`, `macos`).
    pub os: Option<String>,
    /// Only images whose browser the cua-driver browser tools can drive.
    pub browser: bool,
}

fn catalog() -> Value {
    serde_json::from_str(CATALOG_JSON).expect("sandbox-images.json is valid JSON")
}

/// True when cua-driver's browser tools can drive a browser in this image:
/// it ships cua-spacesd (which embeds cua-driver) and a Chromium-family
/// browser.
fn browser_tools(image: &Value) -> bool {
    image["spacesd"].as_bool() == Some(true)
        && image["browsers"].as_array().is_some_and(|b| {
            b.iter()
                .any(|b| CDP_BROWSERS.contains(&b.as_str().unwrap_or("")))
        })
}

/// One catalog row as `images_list` / `cua images ls --json` return it: the
/// catalog entry plus `browser_tools`, `variants` (the refs of the same
/// image in its other forms) and the `cua` commands that create it.
fn row(image: &Value, all: &[Value]) -> Value {
    let r = image["ref"].as_str().unwrap_or_default();
    let base = r.trim_end_matches("-disk");
    let variants: Vec<Value> = all
        .iter()
        .filter(|i| {
            let o = i["ref"].as_str().unwrap_or_default();
            o != r && o.trim_end_matches("-disk") == base
        })
        .map(|i| json!({"ref": i["ref"], "variant": i["variant"]}))
        .collect();
    let mut v = image.clone();
    // The catalog file still records a cloud runtime per image; Cua Cloud
    // has ended, so it is not shown.
    if let Some(o) = v.as_object_mut() {
        o.remove("cloud");
    }
    v["browser_tools"] = json!(browser_tools(image));
    v["variants"] = Value::Array(variants);
    let mut create = vec![];
    if image["local"].is_string() {
        create.push(format!("cua sb create {r}"));
    }
    if browser_tools(image) {
        create.push(format!("cua sb create {r} --browser"));
    }
    v["create"] = json!(create);
    v
}

/// The catalog rows matching `f`, in file order, then the images of the
/// local runtime providers installed on this machine.
pub fn list(f: &Filter) -> Vec<Value> {
    list_with(
        f,
        &cua_sandbox_core::external::Registry::global().providers(),
    )
}

/// The rows of `providers`' images: each runs locally on its provider, as
/// the kind the provider presents as. They are Spaces images (they run
/// cua-spacesd).
fn provider_rows(
    providers: &[std::sync::Arc<cua_sandbox_core::external::ExternalProvider>],
) -> Vec<Value> {
    let mut out = vec![];
    for p in providers {
        for i in &p.manifest.images {
            let r = i.reference.as_str();
            out.push(json!({
                "ref": r,
                "os": p.manifest.os.first().cloned().unwrap_or_default(),
                "variant": p.kind.as_str(),
                "local": p.name(),
                "runtime": p.name(),
                "provider": p.manifest.label(),
                "summary": if i.title.is_empty() { p.manifest.label().to_string() } else { i.title.clone() },
                "default": i.default,
                "published": true,
                "spacesd": true,
                "browsers": [],
                "browser_tools": false,
                "variants": [],
                "create": [format!("cua sb create {r} --runtime {}", p.name())],
            }));
        }
    }
    out
}

/// [`list`] with these providers' images.
fn list_with(
    f: &Filter,
    providers: &[std::sync::Arc<cua_sandbox_core::external::ExternalProvider>],
) -> Vec<Value> {
    let c = catalog();
    let all: Vec<Value> = c["images"].as_array().cloned().unwrap_or_default();
    let extra = provider_rows(providers);
    all.iter()
        .filter(|i| f.all || i["published"].as_bool() == Some(true))
        .filter(|i| {
            f.os.as_deref()
                .is_none_or(|os| i["os"].as_str() == Some(os.to_ascii_lowercase().as_str()))
        })
        .filter(|i| !f.browser || browser_tools(i))
        .map(|i| row(i, &all))
        .chain(extra.into_iter().filter(|i| {
            !f.browser
                && f.os
                    .as_deref()
                    .is_none_or(|os| i["os"].as_str() == Some(os.to_ascii_lowercase().as_str()))
        }))
        .collect()
}

/// The catalog entry for `reference` (a catalog ref, or an alias such as
/// `linux` that resolves to one).
pub fn get(reference: &str) -> Result<Value, CuaError> {
    // Aliases resolve to the catalog's own refs, not a CUA_IMAGE_* override.
    // Unpublished entries (a tier, `omarchy`) are still listed here.
    let wanted = match cua_image::canonical::word_reference_with(reference, None, &|_| None) {
        Ok(Some((r, _, _))) => r,
        _ => reference.to_string(),
    };
    let everything = list(&Filter {
        all: true,
        ..Default::default()
    });
    everything
        .into_iter()
        .find(|i| i["ref"].as_str() == Some(reference) || i["ref"].as_str() == Some(&wanted))
        .ok_or_else(|| {
            CuaError::NotFound(format!(
                "{reference} is not in the image catalog (see `cua images ls --all`)"
            ))
        })
}

/// The `images_list` MCP tool result.
pub fn mcp_result(f: &Filter) -> Value {
    let c = catalog();
    json!({
        "images": list(f),
        "groups": c["groups"],
        "hint": "Pick an image with browser_tools=true for web browsing: sandbox_create {\"browser\": true} starts one with a browser the cua-driver browser tools drive (call_tool with space=<ref>).",
        "docs": "https://cua.ai/docs/cua-sdk/reference/os-image-catalog",
    })
}

fn local_runtime(i: &Value) -> &str {
    match i["local"].as_str() {
        Some("container") => "container",
        Some("qemu") => "qemu vm",
        Some("lume") => "lume vm",
        // A local runtime provider's image: its runtime word.
        Some(other) if i["runtime"].as_str() == Some(other) => other,
        _ => "-",
    }
}

/// `cua images ls`.
pub fn print_list(f: &Filter, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let rows = list(f);
    if json {
        line(out, Value::Array(rows).to_string());
        return Ok(0);
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|i| {
            let local = local_runtime(i);
            let browsers: Vec<&str> = i["browsers"]
                .as_array()
                .map(|b| b.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            vec![
                i["ref"].as_str().unwrap_or_default().to_string(),
                i["os"].as_str().unwrap_or_default().to_string(),
                i["tier"].as_str().unwrap_or("-").to_string(),
                i["variant"].as_str().unwrap_or_default().to_string(),
                local.to_string(),
                if i["spacesd"].as_bool() == Some(true) {
                    "yes".into()
                } else {
                    "no".into()
                },
                if browsers.is_empty() {
                    "-".into()
                } else {
                    browsers.join(",")
                },
                i["summary"].as_str().unwrap_or_default().to_string(),
            ]
        })
        .collect();
    util::table(
        out,
        &[
            "REF", "OS", "TIER", "KIND", "LOCAL", "SPACESD", "BROWSERS", "SUMMARY",
        ],
        &table,
    );
    line(
        out,
        "\nWeb browsing: `cua sb create linux --browser` (Chromium, driven by the cua-driver browser tools).\nTiers: `cua sb create linux --tier slim` (full is the default; Linux also has --tier docker, a VM with dockerd; macOS --tier xcode).",
    );
    Ok(0)
}

/// `cua images info REF`.
pub fn print_info(reference: &str, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let i = get(reference)?;
    if json {
        line(out, i.to_string());
    } else {
        line(out, serde_json::to_string_pretty(&i).unwrap_or_default());
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_lists_published_images_with_browser_facts() {
        let rows = list(&Filter::default());
        assert!(rows.iter().all(|r| r["published"] == true));
        let linux = rows
            .iter()
            .find(|r| r["ref"] == "ghcr.io/trycua/linux:24.04")
            .expect("canonical linux image");
        assert_eq!(linux["browser_tools"], true);
        assert_eq!(
            linux["variants"][0]["ref"],
            "ghcr.io/trycua/linux:24.04-disk"
        );
        assert!(
            linux["create"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c.as_str().unwrap().ends_with("--browser"))
        );
        // macOS: the tiers that ship cua-spacesd and Chrome have the browser
        // tools; one without cua-spacesd (Safari only) does not.
        let macs: Vec<_> = rows.iter().filter(|r| r["os"] == "macos").collect();
        let driven = |r: &Value| {
            r["spacesd"] == true
                && r["browsers"]
                    .as_array()
                    .is_some_and(|b| b.iter().any(|b| b == "chrome"))
        };
        assert!(macs.iter().any(|r| driven(r)), "a macOS image with Chrome");
        assert!(macs.iter().any(|r| !driven(r)), "a macOS image without");
        for r in &macs {
            assert_eq!(r["browser_tools"], driven(r), "{}", r["ref"]);
        }
        // Every published ref the canonical resolver defaults to is listed.
        let (default_linux, _) =
            crate::sandbox::resolve_image_with("linux", None, |_| None).unwrap();
        assert!(rows.iter().any(|r| r["ref"] == default_linux.as_str()));
    }

    #[test]
    fn provider_images_are_listed_only_when_a_provider_is_installed() {
        use cua_sandbox_core::external::ExternalProvider;
        let f = Filter::default();
        let alone = list_with(&f, &[]);
        assert!(
            alone.iter().all(|r| r.get("runtime").is_none()),
            "no provider: the catalog alone"
        );
        let p = std::sync::Arc::new(ExternalProvider {
            manifest: serde_json::from_value(json!({
                "protocol": 1, "name": "acme", "display_name": "macOS VMs", "kind": "vm",
                "os": ["macos"],
                "images": [{"reference": "registry.example/acme/macos:15", "title": "macOS 15", "default": true}]
            }))
            .unwrap(),
            path: "/x/cua-runtime-acme".into(),
            kind: cua_sandbox_core::placement::Kind::Vm,
        });
        let rows = list_with(&f, std::slice::from_ref(&p));
        let row = rows.last().unwrap();
        assert_eq!(row["ref"], "registry.example/acme/macos:15");
        assert_eq!(row["local"], "acme");
        assert_eq!(row["variant"], "vm");
        assert_eq!(
            row["create"][0],
            "cua sb create registry.example/acme/macos:15 --runtime acme"
        );
        assert_eq!(local_runtime(row), "acme");
        let linux = Filter {
            os: Some("linux".into()),
            ..Default::default()
        };
        assert!(list_with(&linux, &[p]).iter().all(|r| r["os"] == "linux"));
    }

    #[test]
    fn filters_and_lookup() {
        let browser = list(&Filter {
            browser: true,
            ..Default::default()
        });
        assert!(!browser.is_empty());
        assert!(browser.iter().all(|r| r["browser_tools"] == true));
        let mac = list(&Filter {
            os: Some("MACOS".into()),
            ..Default::default()
        });
        assert!(mac.iter().all(|r| r["os"] == "macos"));
        assert!(
            list(&Filter::default()).len()
                < list(&Filter {
                    all: true,
                    ..Default::default()
                })
                .len()
        );
        assert_eq!(get("linux").unwrap()["ref"], "ghcr.io/trycua/linux:24.04");
        assert!(matches!(
            get("ghcr.io/nope/x:1"),
            Err(CuaError::NotFound(_))
        ));
    }
}
