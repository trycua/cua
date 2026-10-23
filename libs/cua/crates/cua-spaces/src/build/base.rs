//! What a build's `FROM` maps to.
//!
//! A Space needs an image that runs cua-spacesd, so `FROM` is one of:
//!
//! - a Cua image: an alias (`linux`, `linux:22.04`) or anything
//!   [`cua_image::canonical::is_canonical`] accepts (`ghcr.io/trycua/...`),
//!   used as it is;
//! - `ubuntu:<major.minor>` / `ubuntu` / `ubuntu:latest`: Cua's Linux image
//!   of that version (the default version for the unversioned forms);
//! - `debian:*`: Cua's Linux image (Ubuntu), said so in the build's first
//!   progress line;
//! - `python:<version>[-slim...]`: Cua's Linux image with that Python
//!   installed by uv and first on `PATH` as `python`, `python3` and `pip`.
//!
//! Any other base is refused, naming what works: an arbitrary OCI image does
//! not carry cua-spacesd, and running those is a separate decision.

use crate::error::{Error, Result};

/// A resolved `FROM`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    /// The `FROM` reference as written.
    pub from: String,
    /// The Cua image the build starts from.
    pub image: String,
    /// Python to install first (`python:<v>` bases).
    pub python: Option<String>,
    /// What the mapping changed, for the build's first progress line.
    pub note: Option<String>,
}

/// Maps a `FROM` reference (see the module docs).
pub fn classify_base(reference: &str) -> Result<Base> {
    let r = reference.trim();
    if r.is_empty() {
        return Err(Error::invalid("FROM needs an image"));
    }
    if r.eq_ignore_ascii_case("scratch") {
        return Err(Error::invalid(
            "FROM scratch has nothing to run in: a Space starts from an image that runs cua-spacesd (a Cua image, ubuntu, debian or python)",
        ));
    }
    let make = |image: String, python: Option<String>, note: Option<String>| Base {
        from: r.to_string(),
        image,
        python,
        note,
    };
    let linux = |version: Option<&str>| {
        cua_image::canonical::canonical_image("linux", version)
            .map_err(|e| Error::invalid(e.to_string()))
    };
    let short = r
        .strip_prefix("docker.io/library/")
        .or_else(|| r.strip_prefix("index.docker.io/library/"))
        .or_else(|| r.strip_prefix("docker.io/"))
        .unwrap_or(r);
    let without_digest = short.split('@').next().unwrap_or(short);
    let (name, tag) = without_digest
        .split_once(':')
        .unwrap_or((without_digest, "latest"));
    match name {
        "python" => {
            let v = python_version(tag);
            return Ok(make(
                linux(None)?,
                Some(v.clone()),
                Some(format!(
                    "{r}: Cua's Linux image with Python {v} installed by uv (on PATH as python, python3 and pip)"
                )),
            ));
        }
        "ubuntu" => {
            let version = (tag != "latest" && tag != "rolling").then_some(tag);
            if let Some(v) = version
                && !v
                    .split('.')
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
            {
                return Err(Error::invalid(format!(
                    "{r}: ubuntu tags are versions (ubuntu:24.04); use a Cua image (ghcr.io/trycua/linux:...) for anything else"
                )));
            }
            return Ok(make(linux(version)?, None, None));
        }
        "debian" => {
            return Ok(make(
                linux(None)?,
                None,
                Some(format!(
                    "{r}: Debian is not published as a Space image; building on Cua's Linux image (Ubuntu)"
                )),
            ));
        }
        _ => {}
    }
    if let Some((image, _)) = cua_image::canonical::alias_with(r, &|n| std::env::var(n).ok()) {
        return Ok(make(image, None, None));
    }
    if r.starts_with("ghcr.io/trycua/") || cua_image::canonical::is_canonical(r) {
        return Ok(make(r.to_string(), None, None));
    }
    Err(Error::invalid(format!(
        "the base image {r:?} cannot run as a Space: an arbitrary container image does not carry cua-spacesd. \
         Use a Cua image (ghcr.io/trycua/linux:24.04), ubuntu:<version>, debian:<version> or python:<version>"
    )))
}

/// The Python version in a `python:` tag: `3.12.7-slim-bookworm` is
/// `3.12.7`; a tag without a version (`slim`, `latest`) is `3`.
pub fn python_version(tag: &str) -> String {
    let v: String = tag
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let v = v.trim_matches('.').to_string();
    if v.is_empty() { "3".into() } else { v }
}

/// Shell (run as root) that installs Python `v` with uv and puts it first on
/// `PATH` as `python`, `python3` and `pip` in `/usr/local/bin`.
pub fn python_script(v: &str) -> String {
    let minor: String = {
        let parts: Vec<&str> = v.split('.').collect();
        match parts.as_slice() {
            [a, b, ..] => format!("{a}.{b}"),
            [a] => a.to_string(),
            [] => "3".into(),
        }
    };
    format!(
        r#"set -e
V='{v}'
if [ -x /usr/local/bin/python ] && /usr/local/bin/python -c "import sys; v='{minor}'.split('.'); sys.exit(0 if all(a == str(b) for a, b in zip(v, sys.version_info[:len(v)])) else 1)" 2>/dev/null; then exit 0; fi
export UV_PYTHON_INSTALL_DIR=/opt/uv-python
if ! command -v uv >/dev/null 2>&1; then
  curl -LsSf https://astral.sh/uv/install.sh | env UV_INSTALL_DIR=/usr/local/bin sh
fi
uv python install "$V"
P="$(uv python find "$V")"
B="$(dirname "$P")"
"$P" -m ensurepip --upgrade >/dev/null 2>&1 || true
for n in python python3 python{minor}; do ln -sf "$P" /usr/local/bin/$n; done
for n in pip pip3 pip{minor}; do [ -e "$B/$n" ] && ln -sf "$B/$n" /usr/local/bin/$n || true; done
python --version
"#
    )
}
