// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua viewer`: the cua-spacesd HTML5 viewer's standalone server over every
//! sandbox this SDK knows (local, Fleet, direct). Each sandbox's page is
//! served under `/s/<id>/viewer/`; its calls are piped to the sandbox's
//! `env` service URL (the published loopback port, or a signed Fleet
//! service URL that needs no bearer). The viewer ticket minted for the
//! browser authorizes every call at the sandbox.

use std::io::Write;
use std::sync::Arc;

use cua_sdk::{Cua, CuaError};
use cua_spacesd_html5::standalone::{self, SandboxEntry, Sandboxes, Upstream};

use cua_cli::util::{self, internal, line};

struct SdkSandboxes(Arc<Cua>, UpstreamCache);

/// Upstreams resolved in the last minute (every proxied call asks).
type UpstreamCache =
    std::sync::Mutex<std::collections::HashMap<String, (Upstream, std::time::Instant)>>;
const UPSTREAM_TTL: std::time::Duration = std::time::Duration::from_secs(60);

fn err(e: CuaError) -> String {
    e.to_string()
}

#[async_trait::async_trait]
impl Sandboxes for SdkSandboxes {
    async fn list(&self) -> Result<Vec<SandboxEntry>, String> {
        let list = self.0.sandboxes().list(None).await.map_err(err)?;
        Ok(list
            .into_iter()
            .map(|i| SandboxEntry {
                detail: format!("{} · {:?}", i.location, i.phase).to_lowercase(),
                name: i.name,
                id: i.id,
            })
            .collect())
    }

    async fn upstream(&self, id: &str) -> Result<Upstream, String> {
        if let Some((u, at)) = self.1.lock().unwrap().get(id)
            && at.elapsed() < UPSTREAM_TTL
        {
            return Ok(u.clone());
        }
        let sandbox = self
            .0
            .sandboxes()
            .connect(id.to_string())
            .await
            .map_err(err)?;
        let base = sandbox
            .service("env".into())
            .map_err(err)?
            .url()
            .await
            .map_err(err)?;
        let base = url::Url::parse(&format!("{}/", base.trim_end_matches('/')))
            .map_err(|e| e.to_string())?;
        let upstream = Upstream {
            base,
            headers: axum::http::HeaderMap::new(),
        };
        self.1.lock().unwrap().insert(
            id.to_string(),
            (upstream.clone(), std::time::Instant::now()),
        );
        Ok(upstream)
    }

    async fn viewer_fragment(&self, id: &str) -> Result<String, String> {
        let link = cua_cli::sandbox::viewer_link(&self.0, id, None, false, None)
            .await
            .map_err(err)?;
        link.url
            .split_once('#')
            .map(|(_, f)| f.to_string())
            .ok_or_else(|| "the viewer link has no ticket".into())
    }
}

/// Serves until Ctrl-C.
pub async fn serve(
    cua: Arc<Cua>,
    listen: &str,
    no_open: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let addr: std::net::SocketAddr = listen
        .parse()
        .map_err(|e| CuaError::InvalidArgument(format!("--listen {listen}: {e}")))?;
    if !addr.ip().is_loopback() {
        return Err(CuaError::InvalidArgument(
            "the viewer server listens on loopback only".into(),
        ));
    }
    let key = standalone::random_key();
    let app = standalone::router(Arc::new(SdkSandboxes(cua, Default::default())), key.clone());
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(internal)?;
    let bound = listener.local_addr().map_err(internal)?;
    let url = format!("http://{bound}/?key={key}");
    line(out, format!("Viewer server: {url}"));
    line(out, "Press Ctrl-C to stop.");
    let _ = out.flush();
    if !no_open {
        util::open_browser(&url);
    }
    tokio::select! {
        r = axum::serve(listener, app) => r.map_err(internal)?,
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(0)
}
