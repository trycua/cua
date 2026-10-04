// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The HTML5 viewer surface: `/viewer` is served without auth, and a viewer
//! ticket (`SystemService.CreateViewerTicket`) reaches only the viewer's
//! methods, only its `files_root`, and cannot mint more tickets.

mod common;

use common::*;
use cua_proto::env::v1::*;
use cua_spacesd_client::TransportPreference;

fn code(e: &tonic::Status) -> tonic::Code {
    e.code()
}

#[tokio::test]
async fn viewer_page_and_ticket_scope() {
    let t = target().await;
    let root_client = t.client(TransportPreference::Native).await;

    // The page is public and carries the security headers.
    let http = http_client();
    let page = http
        .get(t.endpoint().http_url("/viewer/").parse().unwrap())
        .await
        .unwrap();
    assert_eq!(page.status(), http::StatusCode::OK);
    assert!(page.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("frame-ancestors 'none'"));
    let html = String::from_utf8(body_bytes(page, 1 << 20).await).unwrap();
    assert!(html.contains("viewer.js"), "{html}");

    // Mint a ticket confined to the scratch dir, no clipboard.
    let share = format!("{}/viewer-root", t.scratch);
    let minted = root_client
        .system()
        .create_viewer_ticket(CreateViewerTicketRequest {
            ttl: Some(cua_proto::wkt::Duration {
                seconds: 120,
                nanos: 0,
            }),
            policy: SessionPolicy::BackgroundOnly as i32,
            clipboard: false,
            files_root: share.clone(),
            audio_uplink: false,
            principal: Some(Principal {
                id: "ada".into(),
                display_name: "Ada".into(),
                ..Default::default()
            }),
        })
        .await
        .expect("mint")
        .into_inner();
    assert!(
        minted.viewer_path.starts_with("/viewer/#ticket="),
        "{}",
        minted.viewer_path
    );
    assert!(
        minted.viewer_path.contains("&files="),
        "{}",
        minted.viewer_path
    );
    assert!(
        minted.viewer_path.ends_with("&clipboard=0"),
        "{}",
        minted.viewer_path
    );
    let real_root = std::fs::canonicalize(&share)
        .map(|p| p.display().to_string())
        .unwrap_or(share.clone());
    assert_eq!(minted.files_root, real_root);

    for transport in TRANSPORTS {
        let viewer = t
            .client_with_token(transport, Some(minted.ticket.clone()))
            .await;
        // Allowed: capabilities, and files under the root.
        viewer
            .system()
            .get_capabilities(GetCapabilitiesRequest {})
            .await
            .unwrap_or_else(|e| panic!("{transport:?} capabilities: {e}"));
        viewer
            .filesystem()
            .make_dir(MakeDirRequest {
                path: format!("{real_root}/sub"),
                parents: true,
                mode: 0,
            })
            .await
            .unwrap_or_else(|e| panic!("{transport:?} mkdir in root: {e}"));
        viewer
            .filesystem()
            .list_dir(ListDirRequest {
                path: real_root.clone(),
                depth: 4,
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("{transport:?} list root: {e}"));

        // Refused: outside the root, `..` escapes, other services, minting.
        for path in [
            "/etc/hostname".to_string(),
            format!("{real_root}/../outside"),
        ] {
            let e = viewer
                .filesystem()
                .stat(StatRequest {
                    path: path.clone(),
                    no_follow_symlinks: false,
                })
                .await
                .expect_err("outside the root");
            assert_eq!(
                code(&e),
                tonic::Code::PermissionDenied,
                "{transport:?} {path}: {e}"
            );
        }
        let e = viewer
            .process()
            .list_processes(ListProcessesRequest::default())
            .await
            .expect_err("processes are not a viewer method");
        assert_eq!(
            code(&e),
            tonic::Code::PermissionDenied,
            "{transport:?}: {e}"
        );
        let e = viewer
            .system()
            .create_viewer_ticket(CreateViewerTicketRequest::default())
            .await
            .expect_err("a viewer cannot mint");
        assert_eq!(
            code(&e),
            tonic::Code::PermissionDenied,
            "{transport:?}: {e}"
        );
    }

    // A symlink inside the root cannot lead out of it.
    #[cfg(unix)]
    if t.local.is_some() {
        std::os::unix::fs::symlink("/etc", format!("{real_root}/etc-link")).unwrap();
        let viewer = t
            .client_with_token(TransportPreference::Native, Some(minted.ticket.clone()))
            .await;
        let e = viewer
            .filesystem()
            .stat(StatRequest {
                path: format!("{real_root}/etc-link/hostname"),
                no_follow_symlinks: false,
            })
            .await
            .expect_err("symlink escape");
        assert_eq!(code(&e), tonic::Code::PermissionDenied, "{e}");
    }

    // A tampered ticket is refused outright.
    let mut forged = minted.ticket.clone();
    forged.push('x');
    let bad = t
        .client_with_token(TransportPreference::Native, Some(forged))
        .await;
    let e = bad
        .system()
        .get_capabilities(GetCapabilitiesRequest {})
        .await
        .expect_err("forged");
    assert_eq!(code(&e), tonic::Code::Unauthenticated, "{e}");
    t.cleanup().await;
}

#[tokio::test]
async fn viewer_without_files_grant_has_no_filesystem() {
    let t = target().await;
    let minted = t
        .client(TransportPreference::GrpcWeb)
        .await
        .system()
        .create_viewer_ticket(CreateViewerTicketRequest {
            clipboard: true,
            ..Default::default()
        })
        .await
        .expect("mint")
        .into_inner();
    assert!(minted.files_root.is_empty());
    assert!(!minted.viewer_path.contains("files="));
    let viewer = t
        .client_with_token(TransportPreference::GrpcWeb, Some(minted.ticket))
        .await;
    let e = viewer
        .filesystem()
        .stat(StatRequest {
            path: t.scratch.clone(),
            no_follow_symlinks: false,
        })
        .await
        .expect_err("no files grant");
    assert_eq!(e.code(), tonic::Code::PermissionDenied, "{e}");
    t.cleanup().await;
}
