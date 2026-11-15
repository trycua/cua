// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The process's shared HTTPS clients for cua.ai (sign-in, the relay).
//!
//! Building a `reqwest::Client` reads the operating system's root store,
//! which is expensive on macOS (`SecTrustSettingsCopyCertificates` parses
//! the trust settings of every root), and each client has its own
//! connection pool. A client built per call (a relay client per call, an
//! OIDC client per session read) made an idle daemon with the app open
//! read the roots and open a new TLS connection for every relay call: a
//! quarter of a core on macOS (F53). [`shared`] builds one client per
//! shape and hands out clones (one pool); a client older than
//! [`REFRESH_AFTER`] is replaced in the background, so a CA the user adds
//! or removes applies within minutes, and a change of `SSL_CERT_FILE` or
//! `SSL_CERT_DIR` applies to the next call.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// How long one shared client (and its read of the roots) is used.
pub const REFRESH_AFTER: Duration = Duration::from_secs(10 * 60);

/// What a shared client is built with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shape {
    /// TCP and TLS connect timeout.
    pub connect_timeout: Duration,
    /// Whole-request timeout.
    pub timeout: Duration,
    /// `User-Agent`, when the client sets one.
    pub user_agent: Option<&'static str>,
}

type NativeInput = (Option<OsString>, Option<OsString>);

fn native_input() -> NativeInput {
    (
        std::env::var_os("SSL_CERT_FILE"),
        std::env::var_os("SSL_CERT_DIR"),
    )
}

struct Entry {
    at: Instant,
    input: NativeInput,
    client: reqwest::Client,
}

#[derive(Default)]
struct Clients {
    entries: HashMap<Shape, Entry>,
    refreshing: HashSet<Shape>,
}

static CLIENTS: Mutex<Option<Clients>> = Mutex::new(None);
static BUILDS: AtomicUsize = AtomicUsize::new(0);

/// How many shared clients this process built (each one read the roots).
pub fn builds() -> usize {
    BUILDS.load(Ordering::Relaxed)
}

fn build(shape: Shape) -> reqwest::Result<reqwest::Client> {
    crate::ensure_crypto_provider();
    BUILDS.fetch_add(1, Ordering::Relaxed);
    let mut b = reqwest::Client::builder()
        .connect_timeout(shape.connect_timeout)
        .timeout(shape.timeout);
    if let Some(ua) = shape.user_agent {
        b = b.user_agent(ua);
    }
    b.build()
}

fn lock() -> std::sync::MutexGuard<'static, Option<Clients>> {
    CLIENTS.lock().unwrap_or_else(|e| e.into_inner())
}

fn refresh_in_background(shape: Shape) {
    {
        let mut guard = lock();
        let clients = guard.get_or_insert_with(Clients::default);
        if !clients.refreshing.insert(shape) {
            return;
        }
    }
    let spawned = std::thread::Builder::new()
        .name("cua-http-roots".into())
        .spawn(move || {
            let input = native_input();
            let next = build(shape);
            let mut guard = lock();
            let clients = guard.get_or_insert_with(Clients::default);
            clients.refreshing.remove(&shape);
            match next {
                Ok(client) => {
                    clients.entries.insert(
                        shape,
                        Entry {
                            at: Instant::now(),
                            input,
                            client,
                        },
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, "HTTPS client not rebuilt; keeping the current one");
                    if let Some(entry) = clients.entries.get_mut(&shape) {
                        entry.at = Instant::now();
                    }
                }
            }
        });
    if spawned.is_err() {
        lock()
            .get_or_insert_with(Clients::default)
            .refreshing
            .remove(&shape);
    }
}

/// The shared client of `shape` (a clone: same pool, same roots).
pub fn shared(shape: Shape) -> reqwest::Result<reqwest::Client> {
    let input = native_input();
    {
        let guard = lock();
        if let Some(entry) = guard.as_ref().and_then(|c| c.entries.get(&shape))
            && entry.input == input
        {
            let client = entry.client.clone();
            let stale = entry.at.elapsed() >= REFRESH_AFTER;
            drop(guard);
            if stale {
                refresh_in_background(shape);
            }
            return Ok(client);
        }
    }
    // First use, or the native input changed: build now (outside the lock;
    // a concurrent first use may build twice, and the last one stays).
    let client = build(shape)?;
    lock().get_or_insert_with(Clients::default).entries.insert(
        shape,
        Entry {
            at: Instant::now(),
            input,
            client: client.clone(),
        },
    );
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPE: Shape = Shape {
        connect_timeout: Duration::from_secs(7),
        timeout: Duration::from_secs(11),
        user_agent: Some("cua-auth-test"),
    };

    /// F53: many relay calls, one client (one read of the roots, one pool).
    #[test]
    fn one_shape_is_built_once() {
        shared(SHAPE).unwrap();
        let before = builds();
        for _ in 0..200 {
            shared(SHAPE).unwrap();
        }
        assert_eq!(builds(), before);
    }

    /// A stale client is replaced off the caller's path: the call returns
    /// the current one, and a background build follows.
    #[test]
    fn a_stale_client_is_rebuilt_in_the_background() {
        let shape = Shape {
            user_agent: Some("cua-auth-test-stale"),
            ..SHAPE
        };
        shared(shape).unwrap();
        {
            let mut guard = lock();
            let entry = guard.as_mut().unwrap().entries.get_mut(&shape).unwrap();
            entry.at = Instant::now() - REFRESH_AFTER - Duration::from_secs(1);
        }
        shared(shape).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let fresh = lock()
                .as_ref()
                .unwrap()
                .entries
                .get(&shape)
                .is_some_and(|e| e.at.elapsed() < REFRESH_AFTER);
            if fresh {
                break;
            }
            assert!(Instant::now() < deadline, "no background rebuild");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
