// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-spacesd-html5`: the standalone viewer server over a fixed list of
//! spacesd endpoints. `cua viewer` is the same server over every sandbox the
//! cua SDK knows; this binary is for gateways and development.
//!
//! ```text
//! cua-spacesd-html5 --listen 127.0.0.1:8211 \
//!     --sandbox dev=http://127.0.0.1:3211 --sandbox ci=https://gw/api/svc/ns/ci/env
//! ```
//!
//! Each sandbox's viewer ticket comes from `CUA_VIEWER_TICKET_<NAME>` (upper
//! case, `-` as `_`), minted beforehand with `SystemService.CreateViewerTicket`;
//! `--header NAME=Header: value` adds a transport header for one sandbox.

use std::collections::BTreeMap;
use std::sync::Arc;

use cua_spacesd_html5::standalone::{self, SandboxEntry, Sandboxes, Upstream};
use http::{HeaderMap, HeaderName, HeaderValue};

struct Fixed(BTreeMap<String, Upstream>);

#[async_trait::async_trait]
impl Sandboxes for Fixed {
    async fn list(&self) -> Result<Vec<SandboxEntry>, String> {
        Ok(self
            .0
            .iter()
            .map(|(id, u)| SandboxEntry {
                id: id.clone(),
                name: id.clone(),
                detail: u.base.to_string(),
            })
            .collect())
    }
    async fn upstream(&self, id: &str) -> Result<Upstream, String> {
        self.0
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown sandbox {id}"))
    }
    async fn viewer_fragment(&self, id: &str) -> Result<String, String> {
        let var = format!("CUA_VIEWER_TICKET_{}", id.to_uppercase().replace('-', "_"));
        let ticket = std::env::var(&var).map_err(|_| format!("set {var} to a viewer ticket"))?;
        Ok(format!("ticket={ticket}"))
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: cua-spacesd-html5 [--listen ADDR] --sandbox NAME=URL [--header NAME=Header: value]..."
    );
    std::process::exit(2)
}

#[tokio::main]
async fn main() {
    let mut listen = "127.0.0.1:0".to_owned();
    let mut sandboxes = BTreeMap::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().unwrap_or_else(|| usage());
        match arg.as_str() {
            "--listen" => listen = value,
            "--sandbox" => {
                let (name, url) = value.split_once('=').unwrap_or_else(|| usage());
                let base = url::Url::parse(url).unwrap_or_else(|e| {
                    eprintln!("bad URL {url}: {e}");
                    std::process::exit(2)
                });
                sandboxes.insert(
                    name.to_owned(),
                    Upstream {
                        base,
                        headers: HeaderMap::new(),
                    },
                );
            }
            "--header" => {
                let (name, header) = value.split_once('=').unwrap_or_else(|| usage());
                let (key, val) = header.split_once(':').unwrap_or_else(|| usage());
                let upstream: &mut Upstream = sandboxes.get_mut(name).unwrap_or_else(|| usage());
                upstream.headers.insert(
                    HeaderName::try_from(key.trim()).unwrap_or_else(|_| usage()),
                    HeaderValue::from_str(val.trim()).unwrap_or_else(|_| usage()),
                );
            }
            _ => usage(),
        }
    }
    if sandboxes.is_empty() {
        usage();
    }
    let key = standalone::random_key();
    let app = standalone::router(Arc::new(Fixed(sandboxes)), key.clone());
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .unwrap_or_else(|e| {
            eprintln!("bind {listen}: {e}");
            std::process::exit(1)
        });
    let addr = listener.local_addr().expect("bound");
    println!("Viewer server: http://{addr}/?key={key}");
    axum::serve(listener, app).await.expect("serve");
}
