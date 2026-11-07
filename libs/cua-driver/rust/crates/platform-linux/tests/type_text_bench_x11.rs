// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Micro-benchmark for `type_text` key delivery on X11. It calls only API
//! that exists before and after the `delay_ms` change, so the same file
//! measures both. Run under an X server with XTEST:
//!
//! `xvfb-run -a cargo test -p platform-linux --test type_text_bench_x11 -- --ignored --nocapture`

#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use platform_linux::input::{send_type_text, send_type_text_xtest};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;

fn drain_keys(conn: &RustConnection, expected: usize) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = 0;
    while seen < expected {
        if Instant::now() >= deadline {
            bail!("received {seen} of {expected} key events");
        }
        match conn.poll_for_event()? {
            Some(Event::KeyPress(_)) | Some(Event::KeyRelease(_)) => seen += 1,
            Some(Event::Error(error)) => bail!("X11 error: {error:?}"),
            _ => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    Ok(())
}

fn p50(mut ms: Vec<f64>) -> (f64, f64, f64) {
    ms.sort_by(|a, b| a.total_cmp(b));
    (ms[ms.len() / 2], ms[0], ms[ms.len() - 1])
}

#[test]
#[ignore = "benchmark; requires an isolated X11 display with XTEST"]
fn bench_type_text_posting() -> Result<()> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let window = conn.generate_id()?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        200,
        100,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .override_redirect(1)
            .event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE),
    )?
    .check()?;
    conn.map_window(window)?.check()?;
    conn.set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)?;
    conn.get_input_focus()?.reply()?;

    // 80 characters, no characters that need a borrowed keycode.
    let text = "The quick brown fox jumps over the lazy dog, then types 1234567890 in a field.";
    let n = text.chars().count();
    let mut xtest = Vec::new();
    let mut synthetic = Vec::new();
    for _ in 0..9 {
        let started = Instant::now();
        send_type_text_xtest(text)?;
        xtest.push(started.elapsed().as_secs_f64() * 1000.0);
        drain_keys(&conn, n * 2)?;
        let started = Instant::now();
        send_type_text(u64::from(window), text)?;
        synthetic.push(started.elapsed().as_secs_f64() * 1000.0);
        drain_keys(&conn, n * 2)?;
    }
    let (m, lo, hi) = p50(xtest);
    eprintln!("BENCH xtest chars={n} p50_ms={m:.1} min_ms={lo:.1} max_ms={hi:.1}");
    let (m, lo, hi) = p50(synthetic);
    eprintln!("BENCH xsendevent chars={n} p50_ms={m:.1} min_ms={lo:.1} max_ms={hi:.1}");
    Ok(())
}
