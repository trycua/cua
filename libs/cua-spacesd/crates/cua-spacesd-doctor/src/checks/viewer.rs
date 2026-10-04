// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `viewer`: the HTML5 viewer at `/viewer` is served, and a viewer ticket
//! is scoped: it reaches its files root and nothing outside the viewer's
//! methods.

use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::{pb, ConnectOptions, SpacesdClient};

use crate::{Ctx, Recorder};

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("viewer") {
        return;
    }
    rec.run("viewer.page", &["core"], Duration::from_secs(10), async {
        let endpoint = ctx.client.endpoint();
        let page = super::files::plain_get(endpoint.host(), endpoint.port(), "/viewer/").await;
        let script = super::files::plain_get(endpoint.host(), endpoint.port(), "/viewer/viewer.js").await;
        match (page, script) {
            (Ok((200, html)), Ok((200, js))) if String::from_utf8_lossy(&html).contains("viewer.js") => Check::new(
                "viewer.page",
                Status::Pass,
                format!("GET /viewer/ and /viewer/viewer.js: HTTP 200 ({} + {} bytes, no credentials)", html.len(), js.len()),
            ),
            (page, script) => Check::new(
                "viewer.page",
                Status::Fail,
                format!(
                    "GET /viewer/: {:?}; /viewer/viewer.js: {:?}",
                    page.map(|(s, _)| s).map_err(|e| e.to_string()),
                    script.map(|(s, _)| s).map_err(|e| e.to_string())
                ),
            )
            .fix("this cua-spacesd build does not embed cua-spacesd-html5; rebuild it"),
        }
    })
    .await;

    rec.run(
        "viewer.ticket_scope",
        &["core"],
        Duration::from_secs(20),
        async {
            let root = format!("/tmp/cua-doctor-viewer-{}", ctx.nonce);
            let minted = match ctx
                .client
                .system()
                .create_viewer_ticket(pb::CreateViewerTicketRequest {
                    ttl: Some(pbjson_types::Duration {
                        seconds: 120,
                        nanos: 0,
                    }),
                    files_root: root.clone(),
                    ..Default::default()
                })
                .await
            {
                Ok(r) => r.into_inner(),
                Err(e) => {
                    return Check::new(
                        "viewer.ticket_scope",
                        Status::Fail,
                        format!("CreateViewerTicket: {e}"),
                    );
                }
            };
            let result = async {
                let mut options = ConnectOptions::parse(&ctx.client.endpoint().to_string())
                    .map_err(|e| e.to_string())?;
                options = options.probe(false).token(minted.ticket.clone());
                let viewer = SpacesdClient::connect(options)
                    .await
                    .map_err(|e| e.to_string())?;
                viewer
                    .filesystem()
                    .stat(pb::StatRequest {
                        path: minted.files_root.clone(),
                        no_follow_symlinks: false,
                    })
                    .await
                    .map_err(|e| format!("stat inside the root refused: {e}"))?;
                let outside = viewer
                    .filesystem()
                    .stat(pb::StatRequest {
                        path: "/etc".into(),
                        no_follow_symlinks: false,
                    })
                    .await;
                if !matches!(&outside, Err(e) if e.code() == tonic::Code::PermissionDenied) {
                    return Err(format!(
                        "stat /etc with a viewer ticket: {:?}",
                        outside.map(|_| "allowed")
                    ));
                }
                let process = viewer
                    .process()
                    .list_processes(pb::ListProcessesRequest::default())
                    .await;
                if !matches!(&process, Err(e) if e.code() == tonic::Code::PermissionDenied) {
                    return Err(format!(
                        "ListProcesses with a viewer ticket: {:?}",
                        process.map(|_| "allowed")
                    ));
                }
                Ok::<_, String>(())
            }
            .await;
            let _ = ctx.client.remove(&minted.files_root, true).await;
            match result {
                Ok(()) => Check::new(
                    "viewer.ticket_scope",
                    Status::Pass,
                    "a viewer ticket reads its files root and is refused /etc and ProcessService",
                ),
                Err(message) => Check::new("viewer.ticket_scope", Status::Fail, message)
                    .fix("viewer tickets must be confined; see cua-spacesd-server auth"),
            }
        },
    )
    .await;
}
