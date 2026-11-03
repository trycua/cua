// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `presence`: a participant can join, gets its own `joined` event with a
//! roster, and leaves cleanly.

use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::pb;
use futures_util::StreamExt as _;

use crate::{Ctx, Recorder};

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("presence") {
        return;
    }
    let claims: &[&str] = &["feature:presence"];
    if !ctx.supports("presence") {
        rec.push(
            Check::new(
                "presence.join",
                Status::Fail,
                format!("presence unsupported: {}", ctx.limitation("presence")),
            ),
            claims,
        )
        .await;
        return;
    }
    rec.run("presence.join", claims, Duration::from_secs(20), async {
        let principal = pb::Principal {
            id: format!("cua-doctor-{}", ctx.nonce),
            display_name: "cua doctor".into(),
            color: String::new(),
            kind: pb::PrincipalKind::Agent as i32,
        };
        let mut stream = match ctx
            .client
            .presence()
            .join(pb::JoinRequest {
                principal: Some(principal.clone()),
                keepalive_interval: Some(pbjson_types::Duration {
                    seconds: 5,
                    nanos: 0,
                }),
                ..pb::JoinRequest::default()
            })
            .await
        {
            Ok(r) => r.into_inner(),
            Err(status) => {
                return Check::new(
                    "presence.join",
                    Status::Fail,
                    format!("Join: {}", status.message()),
                )
            }
        };
        let mut joined = None;
        for _ in 0..50 {
            match tokio::time::timeout(Duration::from_secs(8), stream.next()).await {
                Ok(Some(Ok(event))) => {
                    if let Some(pb::join_response::Event::Joined(j)) = event.event {
                        joined = Some(j);
                        break;
                    }
                }
                _ => break,
            }
        }
        let Some(joined) = joined else {
            return Check::new(
                "presence.join",
                Status::Fail,
                "no `joined` event within 8 s",
            );
        };
        let participant = joined.participant.unwrap_or_default();
        let ours = participant
            .principal
            .as_ref()
            .is_some_and(|p| p.id == principal.id);
        let left = ctx
            .client
            .presence()
            .leave(pb::LeaveRequest {
                participant_id: participant.participant_id.clone(),
            })
            .await
            .is_ok();
        Check::new(
            "presence.join",
            super::verdict(ours && left),
            format!(
                "joined as {} (roster {}), leave {}",
                participant.participant_id,
                joined.roster.len(),
                if left { "ok" } else { "failed" }
            ),
        )
    })
    .await;
}
