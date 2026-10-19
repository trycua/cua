// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `stream`: every encoder backend compiled into this build encodes a
//! moving test pattern that an independent decoder reads back above a PSNR
//! floor; hardware backends run only with `CUA_CODEC_TEST_<BACKEND>=1` (GPU
//! runners), otherwise they skip with `hw_encoder_deferred`. Then a real
//! media session: `OpenMedia` on the primary display, the `/media`
//! WebSocket handshake with its ticket, and a decoded, non-blank keyframe.

use std::time::{Duration, Instant};

use cua_media_codec::{Backend, EncoderConfig, VideoCodec, VideoFrame};
use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::{pb, MediaOptions};
use futures_util::StreamExt as _;

use crate::{Ctx, Recorder};

/// Width and height of the synthetic test clip.
const CLIP: (u32, u32) = (640, 360);
/// Frames encoded per backend.
const FRAMES: u32 = 30;
/// Floor on the luma PSNR of the last decoded frame.
pub const MIN_PSNR_DB: f64 = 30.0;

/// Lowercase backend name.
pub fn backend_name(backend: Backend) -> String {
    format!("{backend:?}").to_lowercase()
}

/// Deterministic BGRA frame `n`: gradients plus a moving bar.
pub fn pattern(n: u32) -> Vec<u8> {
    let (w, h) = CLIP;
    let mut out = vec![0u8; (w * h * 4) as usize];
    let bar = (n * 12) % w;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let on_bar = x >= bar && x < bar + 40;
            out[i] = if on_bar { 250 } else { (x * 255 / w) as u8 };
            out[i + 1] = if on_bar { 250 } else { (y * 255 / h) as u8 };
            out[i + 2] = ((x + y + n * 3) % 256) as u8;
            out[i + 3] = 255;
        }
    }
    out
}

/// Encodes [`FRAMES`] frames with `backend`, decodes them with the best
/// available decoder, and returns (psnr of the last frame, bytes, frames
/// decoded).
pub fn encode_decode(backend: Backend) -> Result<(f64, usize, usize), String> {
    let config = EncoderConfig::new(VideoCodec::H264, CLIP.0, CLIP.1, 30);
    let mut encoder = cua_media_codec::backends::open_encoder(backend, &config)
        .map_err(|e| format!("open: {e}"))?;
    let mut decoder = cua_media_codec::backends::open_best_decoder(VideoCodec::H264)
        .map_err(|e| format!("decoder: {e}"))?;
    let mut bytes = 0usize;
    let mut decoded = 0usize;
    let mut last = None;
    for n in 0..FRAMES {
        let bgra = pattern(n);
        let frame = VideoFrame::bgra(CLIP.0, CLIP.1, u64::from(n) * 33_333, &bgra);
        let units = encoder
            .encode(&frame)
            .map_err(|e| format!("encode frame {n}: {e}"))?;
        for unit in units {
            bytes += unit.data.len();
            if let Some(picture) = decoder
                .decode(&unit.data, unit.pts_us)
                .map_err(|e| format!("decode: {e}"))?
            {
                decoded += 1;
                last = Some((n, picture));
            }
        }
    }
    for unit in encoder.flush().map_err(|e| format!("flush: {e}"))? {
        bytes += unit.data.len();
        if let Some(picture) = decoder
            .decode(&unit.data, unit.pts_us)
            .map_err(|e| format!("decode: {e}"))?
        {
            decoded += 1;
            last = Some((FRAMES - 1, picture));
        }
    }
    let (n, picture) = last.ok_or("no frame decoded")?;
    // Compare the frame whose pts matches, on luma.
    let n = (picture.pts_us / 33_333).min(u64::from(n)) as u32;
    let source =
        cua_media_codec::convert::bgra_to_i420(CLIP.0, CLIP.1, &pattern(n), (CLIP.0 * 4) as usize);
    let psnr = cua_media_codec::convert::psnr(&source.y, &picture.y);
    Ok((psnr, bytes, decoded))
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("stream") {
        return;
    }
    let required: Vec<String> = ctx.manifest.manifest.codecs_required.clone();
    for backend in cua_media_codec::probe::compiled_backends() {
        let name = backend_name(backend);
        let id = format!("stream.encode.{name}");
        let codec_claim = format!("codec:{name}");
        let claims: Vec<&str> = if required.contains(&name) {
            vec!["manifest:codecs_required"]
        } else if backend.is_hardware() {
            vec!["feature:h264_hw"]
        } else {
            vec!["feature:h264_sw"]
        };
        let _ = codec_claim;
        if backend.is_hardware() {
            if let Some(var) = backend.hardware_test_env() {
                if std::env::var_os(var).is_none() {
                    rec.skip(
                        &id,
                        &claims,
                        "hw_encoder_deferred",
                        format!(
                            "{name} is a hardware encoder; set {var}=1 on a GPU runner to test it"
                        ),
                    )
                    .await;
                    continue;
                }
            }
        }
        rec.run(&id, &claims, Duration::from_secs(60), async {
            let result = tokio::task::spawn_blocking(move || encode_decode(backend)).await;
            match result {
                Ok(Ok((psnr, bytes, decoded))) => Check::new(
                    &id,
                    super::verdict(psnr >= MIN_PSNR_DB && decoded > 0),
                    format!("{name}: {FRAMES} frames {}x{} -> {bytes} bytes, {decoded} decoded, PSNR {psnr:.1} dB (floor {MIN_PSNR_DB})", CLIP.0, CLIP.1),
                )
                .fact("psnr_db", format!("{psnr:.1}"))
                .fact("hardware", backend.is_hardware()),
                Ok(Err(error)) => Check::new(&id, Status::Fail, format!("{name}: {error}")),
                Err(panic) => Check::new(&id, Status::Fail, format!("{name} panicked: {panic}")),
            }
        })
        .await;
    }

    // What the server's selection would pick (fidelity: encoder).
    let chosen = tokio::task::spawn_blocking(|| {
        let mut infos =
            cua_media_codec::probe::probe_cached(&cua_media_codec::probe::Isolation::InProcess);
        infos.retain(|i| i.is_usable());
        infos.sort_by_key(|i| i.backend.priority());
        infos.first().map(|i| backend_name(i.backend))
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| "none".into());
    ctx.fidelity.lock().await.encoder = chosen.clone();

    if !ctx.supports("desktop_stream") {
        rec.push(
            Check::new(
                "stream.media.desktop",
                Status::Fail,
                format!(
                    "desktop_stream unsupported: {}",
                    ctx.limitation("desktop_stream")
                ),
            ),
            &["feature:desktop_stream"],
        )
        .await;
    } else {
        rec.run(
            "stream.media.desktop",
            &["feature:desktop_stream"],
            Duration::from_secs(30),
            async { media_session(ctx, &chosen).await },
        )
        .await;
    }

    let port = ctx
        .caps
        .side_channels
        .as_ref()
        .map(|s| s.media_quic_port)
        .unwrap_or(0);
    rec.run(
        "stream.quic",
        &["feature:quic_media"],
        Duration::from_secs(5),
        async {
            if port == 0 {
                return Check::new(
                    "stream.quic",
                    Status::Fail,
                    format!("no QUIC media listener: {}", ctx.limitation("quic_media")),
                );
            }
            let bound = udp_bound(port as u16);
            Check::new(
                "stream.quic",
                match bound {
                    Some(true) | None => Status::Pass,
                    Some(false) => Status::Fail,
                },
                match bound {
                    Some(true) => format!("QUIC media listener bound on UDP {port}"),
                    Some(false) => {
                        format!("capabilities say UDP {port} but nothing is bound there")
                    }
                    None => {
                        format!("capabilities report UDP {port} (socket table not readable here)")
                    }
                },
            )
        },
    )
    .await;
}

/// Whether a UDP socket is bound to `port` (Linux `/proc/net/udp*`).
pub fn udp_bound(port: u16) -> Option<bool> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let hex = format!(":{port:04X} ");
    let mut readable = false;
    for file in ["/proc/net/udp", "/proc/net/udp6"] {
        if let Some(text) = crate::sys::read_capped(std::path::Path::new(file)) {
            readable = true;
            if text.lines().skip(1).any(|l| {
                l.split_whitespace()
                    .nth(1)
                    .is_some_and(|local| format!("{local} ").contains(&hex))
            }) {
                return Some(true);
            }
        }
    }
    readable.then_some(false)
}

async fn media_session(ctx: &Ctx, encoder: &str) -> Check {
    let session = match ctx
        .client
        .open_media(MediaOptions {
            codecs: vec![pb::MediaCodec::H264],
            max_fps: 15,
            max_dimension: 1280,
            ..Default::default()
        })
        .await
    {
        Ok(s) => s,
        Err(error) => {
            return Check::new(
                "stream.media.desktop",
                Status::Fail,
                format!("OpenMedia: {error}"),
            )
        }
    };
    let result = read_keyframe(&session.ws_url).await;
    let _ = ctx
        .client
        .stream()
        .close_media(pb::CloseMediaRequest {
            media_session_id: session.session_id.clone(),
        })
        .await;
    match result {
        Ok((width, height, luma_spread, waited)) => Check::new(
            "stream.media.desktop",
            super::verdict(luma_spread > 8),
            format!(
                "/media delivered a {width}x{height} H.264 keyframe in {} ms (encoder {encoder}), luma spread {luma_spread}",
                waited.as_millis()
            ),
        )
        .fact("encoder", encoder),
        Err(error) => Check::new("stream.media.desktop", Status::Fail, error),
    }
}

/// Connects to the media socket, decodes the first video access unit that
/// yields a picture. Bounded: 20 s, 2000 messages.
async fn read_keyframe(url: &str) -> Result<(u32, u32, u8, Duration), String> {
    let started = Instant::now();
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(url),
    )
    .await
    .map_err(|_| "media WebSocket connect timed out".to_owned())?
    .map_err(|e| format!("media WebSocket: {e}"))?;
    let mut decoder = cua_media_codec::backends::open_best_decoder(VideoCodec::H264)
        .map_err(|e| e.to_string())?;
    let deadline = started + Duration::from_secs(20);
    for _ in 0..2000 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let message = match tokio::time::timeout(remaining, socket.next()).await {
            Ok(Some(Ok(m))) => m,
            Ok(Some(Err(e))) => return Err(format!("media socket: {e}")),
            Ok(None) => return Err("media socket closed before a frame".into()),
            Err(_) => return Err("no decodable frame within 20 s".into()),
        };
        let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
            continue;
        };
        if bytes.starts_with(&cua_proto::AUDIO_PACKET_MAGIC) {
            continue;
        }
        let Ok((header, payload)) = cua_media_transport::decode_packet(&bytes) else {
            continue;
        };
        if !matches!(header, cua_media_protocol::WireHeader::Video(_)) {
            continue;
        }
        if let Ok(Some(picture)) = decoder.decode(&payload, 0) {
            let (min, max) = picture
                .y
                .iter()
                .fold((255u8, 0u8), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
            let _ = socket.close(None).await;
            return Ok((
                picture.width,
                picture.height,
                max.saturating_sub(min),
                started.elapsed(),
            ));
        }
    }
    Err("2000 media messages without a decodable frame".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openh264_round_trips_above_the_floor() {
        let (psnr, bytes, decoded) = encode_decode(Backend::OpenH264).unwrap();
        assert!(psnr >= MIN_PSNR_DB, "psnr {psnr}");
        assert!(bytes > 0 && decoded > 0);
    }

    #[test]
    fn pattern_moves() {
        assert_ne!(pattern(0), pattern(1));
        assert_eq!(pattern(3).len(), (CLIP.0 * CLIP.1 * 4) as usize);
    }
}
