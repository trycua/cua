// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Simulations of the replicated volume across machines: partitions, a
//! closed laptop, a takeover with the old owner waking up, jitter, big files
//! on a flapping link, a quorum of three. Time is scaled down (heartbeat
//! 50 ms) and the network is `SimNet`; the nodes are the real ones.

mod common;

use std::time::{Duration, Instant};

use common::*;
use cua_volume::backend::{Backend, Condition};
use cua_volume::replica::simnet::Impair;
use cua_volume::replica::{Config, Durability, PowerEvent};

#[tokio::test]
async fn two_machines_agree_on_an_owner_and_hold_two_copies() {
    let sim = Sim::new(&[("studio", 9000), ("laptop", 2000)]).await;
    // Both started alone with a volume of their own; the less reliable one
    // joins the other's and the more reliable owns it.
    group_ready(&sim, "studio").await;
    let studio = sim.get("studio");
    let (meta, d) = studio
        .backend
        .put_durable("agents/ada/notes.md", b"hello".to_vec(), Condition::None)
        .await
        .unwrap();
    assert_eq!(
        d,
        Durability::Replicated(2),
        "the write waits for the second copy"
    );
    let laptop = sim.get("laptop");
    // The second copy holds the same version and bytes.
    let h = laptop
        .store
        .head("agents/ada/notes.md")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(h.version, meta.version);
    assert_eq!(
        laptop
            .store
            .get("agents/ada/notes.md", None)
            .await
            .unwrap()
            .0,
        b"hello"
    );
    let s = laptop.node.status().await;
    assert!(!s.i_own && s.owner == "studio" && s.copies_now == 2 && s.copies_wanted == 2);
    sim.shutdown().await;
}

#[tokio::test]
async fn a_write_on_the_laptop_goes_through_the_owner_and_both_see_it() {
    let sim = Sim::new(&[("studio", 9000), ("laptop", 2000)]).await;
    group_ready(&sim, "studio").await;
    let laptop = sim.get("laptop");
    let (meta, d) = laptop
        .backend
        .put_durable("public/a.txt", b"from the laptop".to_vec(), Condition::None)
        .await
        .unwrap();
    assert_eq!(d, Durability::Replicated(2));
    // Read-your-writes on the laptop, and the owner has it.
    assert_eq!(
        laptop
            .backend
            .get("public/a.txt", None)
            .await
            .unwrap()
            .1
            .version,
        meta.version
    );
    assert_eq!(
        sim.get("studio")
            .store
            .head("public/a.txt")
            .await
            .unwrap()
            .unwrap()
            .version,
        meta.version
    );
    // A conditional write from the laptop is atomic across machines: a
    // second create-only loses.
    laptop
        .backend
        .put(
            "agents/ada/.cua-lease",
            b"laptop".to_vec(),
            Condition::IfNoneMatch,
        )
        .await
        .unwrap();
    let e = sim
        .get("studio")
        .backend
        .put(
            "agents/ada/.cua-lease",
            b"studio".to_vec(),
            Condition::IfNoneMatch,
        )
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "precondition_failed");
    sim.shutdown().await;
}

#[tokio::test]
async fn a_closed_laptop_never_blocks_the_studio_and_catches_up_when_it_returns() {
    let sim = Sim::new(&[("studio", 9000), ("laptop", 2000)]).await;
    group_ready(&sim, "studio").await;
    let studio = sim.get("studio");
    studio
        .backend
        .put("agents/ada/before.txt", b"1".to_vec(), Condition::None)
        .await
        .unwrap();
    // The lid closes: the laptop says goodbye and goes dark.
    sim.get("laptop")
        .node
        .notify_power(PowerEvent::WillSleep)
        .await;
    // The OS gives the machine a moment between "will sleep" and sleep.
    tokio::time::sleep(Duration::from_millis(50)).await;
    sim.get("laptop").node.freeze(true);
    sim.cut("studio", "laptop");
    let t0 = Instant::now();
    let mut last = Durability::Pending;
    for i in 0..20 {
        let (_, d) = studio
            .backend
            .put_durable(
                &format!("agents/ada/out-{i}.txt"),
                format!("line {i}").into_bytes(),
                Condition::None,
            )
            .await
            .unwrap();
        last = d;
    }
    // 20 writes while the second machine is away took 20 short waits at
    // most, and each says there is one copy.
    assert_eq!(last, Durability::Local(1));
    assert!(
        t0.elapsed() < Duration::from_secs(20 * 400 / 1000 + 4),
        "writes were held up: {:?}",
        t0.elapsed()
    );
    let s = studio.node.status().await;
    assert!(
        s.unreplicated > 0,
        "the status says files are on one machine"
    );
    let words = studio.node.summary(Some("studio")).await;
    assert_eq!(words.holders_up, vec!["studio".to_string()]);
    assert_eq!(words.holders_away, vec!["laptop".to_string()]);
    // The lid opens.
    sim.heal("studio", "laptop");
    sim.get("laptop").node.freeze(false);
    sim.get("laptop")
        .node
        .notify_power(PowerEvent::DidWake)
        .await;
    eventually("the laptop catches up", 15, || async {
        converged(&sim, &["studio", "laptop"]).await
            && sim.get("studio").node.status().await.unreplicated == 0
    })
    .await;
    assert_eq!(heads(sim.get("laptop")).await.len(), 21);
    sim.shutdown().await;
}

#[tokio::test]
async fn changes_made_on_a_laptop_with_no_owner_are_saved_and_sync_later() {
    let sim = Sim::new(&[("studio", 9000), ("laptop", 2000)]).await;
    group_ready(&sim, "studio").await;
    sim.cut("studio", "laptop");
    let laptop = sim.get("laptop");
    eventually("the laptop sees the studio go", 10, || async {
        laptop.node.status().await.peers[0].health != "up"
    })
    .await;
    let (_, d) = laptop
        .backend
        .put_durable(
            "agents/ada/draft.md",
            b"written on a plane".to_vec(),
            Condition::None,
        )
        .await
        .unwrap();
    assert_eq!(d, Durability::Pending);
    // The laptop shows its own write, and says what is waiting.
    assert_eq!(
        laptop
            .backend
            .get("agents/ada/draft.md", None)
            .await
            .unwrap()
            .0,
        b"written on a plane"
    );
    let s = laptop.node.status().await;
    assert_eq!(s.pending.len(), 1);
    let words = laptop.node.summary(Some("studio")).await;
    assert_eq!(words.pending_here, 1);
    // A conditional write cannot be ordered without the owner: it fails
    // plainly instead of pretending.
    let e = laptop
        .backend
        .put(
            "agents/ada/.cua-lease",
            b"x".to_vec(),
            Condition::IfNoneMatch,
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("cannot be reached"), "{e}");
    sim.heal("studio", "laptop");
    eventually("the studio takes the change", 15, || async {
        head_etag(sim.get("studio"), "agents/ada/draft.md")
            .await
            .is_some()
            && laptop.node.status().await.pending.is_empty()
    })
    .await;
    assert_eq!(
        sim.get("studio")
            .store
            .get("agents/ada/draft.md", None)
            .await
            .unwrap()
            .0,
        b"written on a plane"
    );
    eventually("both agree", 10, || async {
        converged(&sim, &["studio", "laptop"]).await
    })
    .await;
    sim.shutdown().await;
}

#[tokio::test]
async fn two_machines_changing_one_file_keep_both_versions() {
    let sim = Sim::new(&[("studio", 9000), ("laptop", 2000)]).await;
    group_ready(&sim, "studio").await;
    sim.get("studio")
        .backend
        .put("agents/ada/plan.md", b"v0".to_vec(), Condition::None)
        .await
        .unwrap();
    eventually("replicated", 10, || async {
        head_etag(sim.get("laptop"), "agents/ada/plan.md")
            .await
            .is_some()
    })
    .await;
    sim.cut("studio", "laptop");
    let laptop = sim.get("laptop");
    eventually("laptop notices", 10, || async {
        laptop.node.status().await.peers[0].health != "up"
    })
    .await;
    laptop
        .backend
        .put(
            "agents/ada/plan.md",
            b"laptop edit".to_vec(),
            Condition::None,
        )
        .await
        .unwrap();
    sim.get("studio")
        .backend
        .put(
            "agents/ada/plan.md",
            b"studio edit".to_vec(),
            Condition::None,
        )
        .await
        .unwrap();
    sim.heal("studio", "laptop");
    eventually("both converge", 20, || async {
        converged(&sim, &["studio", "laptop"]).await
            && laptop.node.status().await.pending.is_empty()
    })
    .await;
    // The owner's version stays at the key; the laptop's edit is a copy.
    let st = sim.get("studio");
    assert_eq!(
        st.store.get("agents/ada/plan.md", None).await.unwrap().0,
        b"studio edit"
    );
    let all = heads(st).await;
    let copies: Vec<_> = all
        .iter()
        .filter(|(k, _)| k.contains("(conflict from laptop"))
        .collect();
    assert_eq!(copies.len(), 1, "{all:?}");
    assert_eq!(
        st.store.get(&copies[0].0, None).await.unwrap().0,
        b"laptop edit"
    );
    sim.shutdown().await;
}

/// The laptop owns the volume, is cut off, and writes; the studio takes
/// over and writes; the laptop wakes. Nothing is lost, there is one owner,
/// the laptop's stale records are refused, and both end up equal.
#[tokio::test]
async fn the_old_owner_waking_after_a_takeover_is_fenced_and_loses_nothing() {
    // The laptop is the more reliable one here, so it owns at the start and
    // gets the volume back at the end.
    let sim = Sim::new(&[("laptop", 9000), ("studio", 2000)]).await;
    group_ready(&sim, "laptop").await;
    let laptop = sim.get("laptop");
    let studio = sim.get("studio");
    laptop
        .backend
        .put("agents/ada/base.txt", b"base".to_vec(), Condition::None)
        .await
        .unwrap();
    laptop
        .backend
        .put("agents/ada/shared.txt", b"s0".to_vec(), Condition::None)
        .await
        .unwrap();
    eventually("studio holds the base", 10, || async {
        head_etag(studio, "agents/ada/shared.txt").await.is_some()
    })
    .await;
    // A partition: the laptop keeps writing as owner; nobody hears it.
    sim.cut("laptop", "studio");
    laptop
        .backend
        .put("agents/ada/lonely-1.txt", b"L1".to_vec(), Condition::None)
        .await
        .unwrap();
    laptop
        .backend
        .put(
            "agents/ada/shared.txt",
            b"laptop shared".to_vec(),
            Condition::None,
        )
        .await
        .unwrap();
    // The studio needs to order a write (an agent's fence): it takes over.
    needs_authority(studio, "spaces/dev/sessions/s1/fence-x").await;
    assert!(studio.node.status().await.i_own);
    studio
        .backend
        .put("agents/ada/studio-1.txt", b"S1".to_vec(), Condition::None)
        .await
        .unwrap();
    studio
        .backend
        .put(
            "agents/ada/shared.txt",
            b"studio shared".to_vec(),
            Condition::None,
        )
        .await
        .unwrap();
    assert!(!laptop.node.status().await.peers.is_empty());
    // The laptop still thinks it owns the volume and keeps writing.
    assert!(laptop.node.status().await.i_own);
    laptop
        .backend
        .put("agents/ada/lonely-2.txt", b"L2".to_vec(), Condition::None)
        .await
        .unwrap();
    sim.heal("laptop", "studio");
    eventually("everything converges", 30, || async {
        converged(&sim, &["laptop", "studio"]).await
            && laptop.node.status().await.pending.is_empty()
            && head_etag(studio, "agents/ada/lonely-2.txt").await.is_some()
    })
    .await;
    // The studio's era stood; nothing from the laptop's divergent tail was
    // applied as is.
    let st = studio.node.status().await;
    assert!(st.epoch >= 2, "{st:?}");
    // Nothing is lost: every written value is a head or a conflict copy.
    let mut values: Vec<Vec<u8>> = vec![];
    for (k, _) in heads(studio).await {
        values.push(studio.store.get(&k, None).await.unwrap().0);
    }
    for want in [
        b"base".to_vec(),
        b"L1".to_vec(),
        b"L2".to_vec(),
        b"S1".to_vec(),
        b"studio shared".to_vec(),
        b"laptop shared".to_vec(),
    ] {
        assert!(
            values.contains(&want),
            "lost {:?}: {:?}",
            String::from_utf8_lossy(&want),
            values
                .iter()
                .map(|v| String::from_utf8_lossy(v).to_string())
                .collect::<Vec<_>>()
        );
    }
    // The key both sides changed has the owner's value and a copy of the other.
    let owner_now = if studio.node.status().await.i_own {
        studio
    } else {
        laptop
    };
    let v = owner_now
        .store
        .get("agents/ada/shared.txt", None)
        .await
        .unwrap()
        .0;
    assert!(v == b"studio shared" || v == b"laptop shared");
    // The more reliable laptop gets the volume back, without anyone losing it.
    eventually("the volume goes back to the laptop", 20, || async {
        laptop.node.status().await.i_own && studio.node.status().await.owner == "laptop"
    })
    .await;
    let s = studio.node.status().await;
    assert!(s.handbacks >= 1 || laptop.node.status().await.handbacks >= 1);
    sim.shutdown().await;
}

#[tokio::test]
async fn a_goodbye_hands_the_volume_over_at_once() {
    let sim = Sim::new(&[("laptop", 9000), ("studio", 2000)]).await;
    group_ready(&sim, "laptop").await;
    sim.get("laptop")
        .backend
        .put("agents/ada/a.txt", b"a".to_vec(), Condition::None)
        .await
        .unwrap();
    eventually("replicated", 10, || async {
        head_etag(sim.get("studio"), "agents/ada/a.txt")
            .await
            .is_some()
    })
    .await;
    let t0 = Instant::now();
    sim.get("laptop")
        .node
        .notify_power(PowerEvent::WillSleep)
        .await;
    eventually("the studio owns the volume", 10, || async {
        sim.get("studio").node.status().await.i_own
    })
    .await;
    let took = t0.elapsed();
    // Silence alone needs the floor (250 ms) plus confirmation (400 ms).
    assert!(took < Duration::from_millis(600), "took {took:?}");
    sim.shutdown().await;
}

#[tokio::test]
async fn jitter_and_a_skewed_clock_cause_no_takeover() {
    let mut cfg = Config::fast();
    cfg.wall_skew_ms = 3_600_000; // the studio's clock is an hour ahead
    let sim = Sim::with(&[("laptop", 9000), ("studio", 2000)], cfg).await;
    group_ready(&sim, "laptop").await;
    sim.net.set_both(
        "laptop",
        "studio",
        Impair {
            latency: Duration::from_millis(30),
            jitter: Duration::from_millis(140),
            ..Default::default()
        },
    );
    let laptop = sim.get("laptop");
    for i in 0..40 {
        laptop
            .backend
            .put(
                &format!("agents/ada/j-{i}.txt"),
                vec![b'x'; 2000],
                Condition::None,
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    eventually("replicated under jitter", 20, || async {
        converged(&sim, &["laptop", "studio"]).await
    })
    .await;
    for id in ["laptop", "studio"] {
        let s = sim.get(id).node.status().await;
        assert_eq!(s.takeovers, 0, "{id} took over on jitter");
        assert_eq!(s.epoch, 1, "{id}");
    }
    sim.shutdown().await;
}

fn sample(n: usize, seed: u8) -> Vec<u8> {
    (0..n)
        .map(|i| ((i as u64 * 2654435761 + seed as u64) >> 7) as u8)
        .collect()
}

#[tokio::test]
async fn a_big_file_survives_a_flapping_link_without_being_sent_again() {
    let mut cfg = Config::fast();
    cfg.block = 256 * 1024;
    cfg.data_timeout = Duration::from_secs(3);
    let sim = Sim::with(&[("studio", 9000), ("laptop", 2000)], cfg).await;
    group_ready(&sim, "studio").await;
    let size = 6 * 1024 * 1024;
    let body = sample(size, 3);
    // 100 kB per second each way: the file takes a minute without help, so
    // flap it.
    sim.net.set_both(
        "studio",
        "laptop",
        Impair {
            bandwidth: 2 * 1024 * 1024,
            latency: Duration::from_millis(5),
            ..Default::default()
        },
    );
    let before = sim.net.bytes("studio", "laptop");
    let studio = sim.get("studio");
    let w = {
        let b = studio.backend.clone();
        let body = body.clone();
        tokio::spawn(async move { b.put("agents/ada/big.bin", body, Condition::None).await })
    };
    // Cut the link a few times while it moves.
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(400)).await;
        sim.cut("studio", "laptop");
        tokio::time::sleep(Duration::from_millis(300)).await;
        sim.heal("studio", "laptop");
    }
    w.await.unwrap().unwrap();
    let t = Instant::now();
    eventually("the big file arrives intact", 25, || async {
        if t.elapsed().as_secs() % 5 == 4 {
            eprintln!(
                "laptop {:?}\nstudio {:?}",
                sim.get("laptop").node.status().await,
                sim.get("studio").node.status().await
            );
        }
        sim.get("laptop")
            .store
            .head("agents/ada/big.bin")
            .await
            .unwrap()
            .is_some()
    })
    .await;
    let got = sim
        .get("laptop")
        .store
        .get("agents/ada/big.bin", None)
        .await
        .unwrap()
        .0;
    assert_eq!(got.len(), size);
    assert_eq!(
        cua_volume::sha256_hex(&got),
        cua_volume::sha256_hex(&body),
        "bytes differ"
    );
    let moved = sim.net.bytes("studio", "laptop") - before;
    assert!(
        moved < (size as u64) * 2,
        "{moved} bytes crossed for a {size} byte file: it was sent again"
    );
    // A new version that only grows at the end costs its end, not the file.
    let mut longer = body.clone();
    longer.extend(sample(100 * 1024, 9));
    let b2 = sim.net.bytes("studio", "laptop");
    studio
        .backend
        .put("agents/ada/big.bin", longer.clone(), Condition::None)
        .await
        .unwrap();
    eventually("the longer version arrives", 30, || async {
        sim.get("laptop")
            .store
            .head("agents/ada/big.bin")
            .await
            .unwrap()
            .is_some_and(|h| h.size == longer.len() as u64)
    })
    .await;
    let moved2 = sim.net.bytes("studio", "laptop") - b2;
    assert!(
        moved2 < 1024 * 1024,
        "{moved2} bytes crossed for an append of 100 KiB"
    );
    sim.shutdown().await;
}

#[tokio::test]
async fn with_three_machines_a_lone_minority_does_not_take_over() {
    let sim = Sim::new(&[("studio", 9000), ("mini", 7000), ("laptop", 2000)]).await;
    group_ready(&sim, "studio").await;
    let mini = sim.get("mini");
    // The laptop keeps no copy (two copies: studio and mini).
    let s = sim.get("laptop").node.status().await;
    assert_eq!(s.role, cua_volume::replica::Role::Client);
    // The mini is cut off from both: it is a minority of one.
    sim.cut("mini", "studio");
    sim.cut("mini", "laptop");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let s = mini.node.status().await;
    assert_eq!(s.takeovers, 0, "a minority must not take over");
    assert_ne!(s.owner, "mini");
    // The studio goes away; the mini, back with the laptop, now has a
    // majority (itself and the laptop) and takes over.
    sim.heal("mini", "laptop");
    sim.heal("mini", "studio");
    sim.cut("studio", "mini");
    sim.cut("studio", "laptop");
    needs_authority(mini, "agents/ada/.cua-lease").await;
    assert!(mini.node.status().await.i_own);
    sim.shutdown().await;
}

#[tokio::test]
async fn a_machine_that_keeps_no_copy_reads_through_its_peers() {
    let sim = Sim::new(&[("studio", 9000), ("mini", 7000), ("laptop", 2000)]).await;
    group_ready(&sim, "studio").await;
    let laptop = sim.get("laptop");
    assert!(
        laptop.backend.remote(),
        "a client sits behind the block cache"
    );
    sim.get("studio")
        .backend
        .put("agents/ada/r.txt", b"read me".to_vec(), Condition::None)
        .await
        .unwrap();
    let (bytes, meta) = laptop.backend.get("agents/ada/r.txt", None).await.unwrap();
    assert_eq!(bytes, b"read me");
    assert_eq!(
        laptop
            .backend
            .get_range("agents/ada/r.txt", &meta.version, 5, 2)
            .await
            .unwrap(),
        b"me"
    );
    assert_eq!(laptop.backend.list("agents/").await.unwrap().len(), 1);
    // The owner goes away: the other copy serves.
    sim.cut("studio", "laptop");
    sim.cut("studio", "mini");
    eventually("the laptop reads from the mini", 15, || async {
        laptop.backend.get("agents/ada/r.txt", None).await.is_ok()
    })
    .await;
    // Someone needs an ordered write: the mini takes over (with the laptop's
    // vote), and a write from the client goes to it.
    needs_authority(sim.get("mini"), "agents/ada/.cua-lease").await;
    let (_, d) = laptop
        .backend
        .put_durable("agents/ada/w.txt", b"w".to_vec(), Condition::None)
        .await
        .unwrap();
    assert!(matches!(
        d,
        Durability::Local(_) | Durability::Replicated(_)
    ));
    sim.shutdown().await;
}

#[tokio::test]
async fn a_restart_keeps_the_log_and_the_outbox() {
    let mut sim = Sim::new(&[("studio", 9000), ("laptop", 2000)]).await;
    group_ready(&sim, "studio").await;
    sim.cut("studio", "laptop");
    let laptop_id = "laptop";
    eventually("laptop sees the studio go", 10, || async {
        sim.get(laptop_id).node.status().await.peers[0].health != "up"
    })
    .await;
    sim.get("laptop")
        .backend
        .put(
            "agents/ada/kept.txt",
            b"survives a restart".to_vec(),
            Condition::None,
        )
        .await
        .unwrap();
    sim.restart("laptop").await;
    assert_eq!(sim.get("laptop").node.status().await.pending.len(), 1);
    sim.heal("studio", "laptop");
    eventually("the studio takes it after the restart", 20, || async {
        head_etag(sim.get("studio"), "agents/ada/kept.txt")
            .await
            .is_some()
            && sim.get("laptop").node.status().await.pending.is_empty()
    })
    .await;
    sim.shutdown().await;
}

/// A laptop on bad wifi comes and goes. It must never take the volume away
/// from the studio that runs the agent (its changes wait in the outbox), and
/// the studio's ordered writes (leases, fences) never wait for it.
#[tokio::test]
async fn a_flaky_laptop_never_takes_the_volume_from_the_studio() {
    let sim = Sim::new(&[("studio", 9000), ("laptop", 2000)]).await;
    group_ready(&sim, "studio").await;
    let studio = sim.get("studio");
    let laptop = sim.get("laptop");
    for round in 0..6 {
        sim.cut("studio", "laptop");
        tokio::time::sleep(Duration::from_millis(900)).await; // longer than detection
        laptop
            .backend
            .put(
                &format!("agents/ada/plane-{round}.txt"),
                vec![b'p'; 100],
                Condition::None,
            )
            .await
            .unwrap();
        // The studio's ordered writes are immediate.
        let t = Instant::now();
        studio
            .backend
            .put(
                &format!("agents/ada/.fence-{round}"),
                b"1".to_vec(),
                Condition::IfNoneMatch,
            )
            .await
            .unwrap();
        assert!(
            t.elapsed() < Duration::from_millis(1200),
            "the studio waited for the laptop"
        );
        sim.heal("studio", "laptop");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    eventually("everything lands", 30, || async {
        converged(&sim, &["studio", "laptop"]).await
            && laptop.node.status().await.pending.is_empty()
    })
    .await;
    let s = laptop.node.status().await;
    assert_eq!(s.takeovers, 0, "the laptop took the volume");
    assert_eq!(studio.node.status().await.epoch, 1);
    assert!(studio.node.status().await.i_own);
    assert_eq!(heads(studio).await.len(), 12);
    sim.shutdown().await;
}

/// The usual way a second machine arrives: the laptop already has a volume
/// with files in it, and the studio is new. The studio, the more reliable
/// one, ends up owning it; the laptop's files are not lost and are not
/// duplicated; writes from either side stay quick afterwards.
#[tokio::test]
async fn a_new_studio_joins_a_laptop_that_already_has_files() {
    let mut sim = Sim::new(&[("laptop", 2000)]).await;
    let laptop_files = [
        ("agents/ada/old-1.txt", "one"),
        ("agents/ada/old-2.txt", "two"),
        ("public/readme.md", "hello"),
    ];
    for (k, v) in laptop_files {
        sim.get("laptop")
            .backend
            .put(k, v.as_bytes().to_vec(), Condition::None)
            .await
            .unwrap();
    }
    // The studio signs in: the laptop's machines list gains it.
    sim.members.push(member("studio", 9000));
    let studio_member = member("studio", 9000);
    sim.get("laptop")
        .node
        .set_members(vec![studio_member])
        .await;
    let dir = tempfile::tempdir().unwrap();
    sim.boot("studio", dir).await;
    group_ready(&sim, "studio").await;
    eventually("the laptop's files are on the studio, once", 20, || async {
        heads(sim.get("studio")).await.len() == 3
            && converged(&sim, &["studio", "laptop"]).await
            && sim.get("laptop").node.status().await.pending.is_empty()
    })
    .await;
    for (k, v) in laptop_files {
        assert_eq!(
            sim.get("studio").store.get(k, None).await.unwrap().0,
            v.as_bytes()
        );
    }
    // A write from the laptop is not held up by the arrival.
    let t = Instant::now();
    let (_, d) = sim
        .get("laptop")
        .backend
        .put_durable("agents/ada/new.txt", b"n".to_vec(), Condition::None)
        .await
        .unwrap();
    assert_eq!(d, Durability::Replicated(2));
    assert!(
        t.elapsed() < Duration::from_millis(600),
        "slow after joining: {:?}",
        t.elapsed()
    );
    // And in the other direction.
    let t = Instant::now();
    sim.get("studio")
        .backend
        .put("agents/ada/new2.txt", b"n".to_vec(), Condition::None)
        .await
        .unwrap();
    assert!(t.elapsed() < Duration::from_millis(600));
    sim.shutdown().await;
}

/// A laptop that vanishes without a goodbye (a dead battery, a dropped
/// link) is waited for at most twice: after two writes whose second copy
/// never came, the studio stops waiting until the laptop answers again.
#[tokio::test]
async fn a_laptop_that_vanishes_without_a_goodbye_costs_the_studio_two_waits() {
    let sim = Sim::new(&[("studio", 9000), ("laptop", 2000)]).await;
    group_ready(&sim, "studio").await;
    let studio = sim.get("studio");
    studio
        .backend
        .put("agents/ada/warm.txt", b"w".to_vec(), Condition::None)
        .await
        .unwrap();
    sim.get("laptop").node.freeze(true);
    // A stopped process behind an open connection: nothing arrives, nothing
    // is reset (the link stays open until the detector gives up on it).
    sim.net.set_both(
        "studio",
        "laptop",
        Impair {
            hold: true,
            ..Default::default()
        },
    );
    let t = Instant::now();
    let mut durs = vec![];
    for i in 0..12 {
        let t1 = Instant::now();
        studio
            .backend
            .put(
                &format!("agents/ada/v-{i}.txt"),
                vec![b'v'; 100],
                Condition::None,
            )
            .await
            .unwrap();
        durs.push(t1.elapsed());
    }
    let ack_wait = Config::fast().ack_wait;
    assert!(
        t.elapsed() < ack_wait * 2 + Duration::from_millis(1500),
        "12 writes took {:?} ({durs:?})",
        t.elapsed()
    );
    assert!(
        durs[4..].iter().all(|d| *d < Duration::from_millis(200)),
        "{durs:?}"
    );
    sim.shutdown().await;
}

/// A machine whose process is gone answers dials with a refusal. That is
/// far better evidence than silence, so the volume does not wait out the
/// silence window before a machine that needs authority takes over.
#[tokio::test]
async fn a_machine_that_refuses_connections_is_replaced_faster_than_a_silent_one() {
    // Silence would need 1.5 s of quiet and 3 s of confirmation.
    let mut cfg = Config::fast();
    cfg.timing.floor = Duration::from_millis(1500);
    cfg.timing.confirm = Duration::from_millis(3000);
    let sim = Sim::with(&[("laptop", 9000), ("studio", 2000)], cfg.clone()).await;
    group_ready(&sim, "laptop").await;
    // The owner's process dies: its connections break, nothing answers.
    sim.get("laptop").node.freeze(true);
    sim.net.leave("laptop");
    let t = Instant::now();
    needs_authority(sim.get("studio"), "agents/ada/.cua-lease").await;
    let took = t.elapsed();
    assert!(
        took < cfg.timing.floor + cfg.timing.confirm - Duration::from_millis(1500),
        "took {took:?}, as long as silence would"
    );
    assert!(sim.get("studio").node.status().await.i_own);
    sim.shutdown().await;
}

/// An older Cua Spaces build on the same home writes to the store directly
/// (it does not know about replication). When the new build starts again
/// those changes are found and reach the other machine; nothing is lost and
/// nothing diverges silently.
#[tokio::test]
async fn changes_an_older_build_made_behind_replications_back_still_arrive() {
    let mut sim = Sim::new(&[("studio", 9000), ("laptop", 2000)]).await;
    group_ready(&sim, "studio").await;
    sim.get("studio")
        .backend
        .put("agents/ada/a.txt", b"a".to_vec(), Condition::None)
        .await
        .unwrap();
    sim.get("studio")
        .backend
        .put("agents/ada/b.txt", b"b".to_vec(), Condition::None)
        .await
        .unwrap();
    eventually("replicated", 10, || async {
        converged(&sim, &["studio", "laptop"]).await && heads(sim.get("laptop")).await.len() == 2
    })
    .await;
    // Both machines stop cleanly (the app is quit); an older build then
    // writes on each home.
    let (studio_store, laptop_store) = (
        sim.get("studio").store.clone(),
        sim.get("laptop").store.clone(),
    );
    sim.get("studio").node.shutdown().await;
    sim.get("laptop").node.shutdown().await;
    studio_store
        .put(
            "agents/ada/a.txt",
            b"a, edited by the old build".to_vec(),
            Condition::None,
        )
        .await
        .unwrap();
    studio_store
        .put(
            "agents/ada/from-old-build.txt",
            b"new".to_vec(),
            Condition::None,
        )
        .await
        .unwrap();
    laptop_store
        .delete("agents/ada/b.txt", Condition::None)
        .await
        .unwrap();
    // The new build starts again on both.
    sim.restart("studio").await;
    sim.restart("laptop").await;
    group_ready(&sim, "studio").await;
    eventually("the changes reach both machines", 30, || async {
        converged(&sim, &["studio", "laptop"]).await
            && sim.get("laptop").node.status().await.pending.is_empty()
            && head_etag(sim.get("laptop"), "agents/ada/from-old-build.txt")
                .await
                .is_some()
    })
    .await;
    assert_eq!(
        sim.get("laptop")
            .store
            .get("agents/ada/a.txt", None)
            .await
            .unwrap()
            .0,
        b"a, edited by the old build"
    );
    assert!(
        head_etag(sim.get("studio"), "agents/ada/b.txt")
            .await
            .is_none(),
        "the delete arrived"
    );
    sim.shutdown().await;
}

/// The first time replication touches a Volume that has files, a copy of its
/// heads is taken; a directory prepared by a newer format is refused.
#[tokio::test]
async fn turning_replication_on_backs_up_the_heads_and_refuses_a_newer_format() {
    let dir = tempfile::tempdir().unwrap();
    let store = cua_volume::fs::FsBackend::new(dir.path().join("data"));
    for i in 0..5 {
        store
            .put(
                &format!("agents/ada/f{i}.txt"),
                vec![b'x'; 10 + i],
                Condition::None,
            )
            .await
            .unwrap();
    }
    let net = cua_volume::replica::simnet::SimNet::new();
    let (tr, _rx) = net.join("m");
    let node = cua_volume::replica::ReplicaNode::open(
        Config::fast(),
        "m",
        "m",
        dir.path(),
        store.clone(),
        std::sync::Arc::new(tr),
        vec![common::member("m", 5000)],
    )
    .await
    .unwrap();
    let backups: Vec<_> = std::fs::read_dir(dir.path().join("replica/backup"))
        .unwrap()
        .collect();
    assert_eq!(backups.len(), 1, "one backup of the heads");
    // The Volume's own files are untouched.
    assert_eq!(store.list("").await.unwrap().len(), 5);
    node.shutdown().await;
    drop(node);
    // A newer format is refused with a plain message.
    std::fs::write(
        dir.path().join("replica/format.json"),
        br#"{"format":99,"created_ms":1}"#,
    )
    .unwrap();
    let (tr2, _rx2) = net.join("m2");
    let e = cua_volume::replica::ReplicaNode::open(
        Config::fast(),
        "m",
        "m",
        dir.path(),
        store,
        std::sync::Arc::new(tr2),
        vec![common::member("m", 5000)],
    )
    .await
    .unwrap_err();
    assert!(e.to_string().contains("newer Cua Spaces"), "{e}");
}

/// A machine that wrote while the owner was away and then became the owner
/// itself must put those writes in the log so the other machine gets them
/// (they were visible on the writing machine as heads of their own).
#[tokio::test]
async fn writes_made_while_apart_by_the_machine_that_takes_over_reach_the_other() {
    let sim = Sim::new(&[("studio", 9000), ("laptop", 2000)]).await;
    group_ready(&sim, "studio").await;
    sim.cut("studio", "laptop");
    let laptop = sim.get("laptop");
    eventually("the laptop sees the studio go", 10, || async {
        laptop.node.status().await.peers[0].health != "up"
    })
    .await;
    // Written while the owner is away: saved, visible here, not in any log.
    laptop
        .backend
        .put(
            "agents/laptop-w/p1",
            b"typed on a plane".to_vec(),
            Condition::None,
        )
        .await
        .unwrap();
    // The laptop then needs an ordered write and becomes the owner.
    needs_authority(laptop, "agents/ada/.cua-lease").await;
    assert!(laptop.node.status().await.i_own);
    sim.heal("studio", "laptop");
    eventually("the studio has it too", 30, || async {
        converged(&sim, &["studio", "laptop"]).await
            && head_etag(sim.get("studio"), "agents/laptop-w/p1")
                .await
                .is_some()
    })
    .await;
    assert_eq!(
        sim.get("studio")
            .store
            .get("agents/laptop-w/p1", None)
            .await
            .unwrap()
            .0,
        b"typed on a plane"
    );
    sim.shutdown().await;
}

/// A folder copy-in is one batch at the owner (one disk barrier, one run of
/// log records) and reaches the other machine whole.
#[tokio::test]
async fn a_batch_of_files_is_one_write_at_the_owner_and_reaches_the_other_machine() {
    let sim = Sim::new(&[("studio", 9000), ("laptop", 2000)]).await;
    group_ready(&sim, "studio").await;
    let items: Vec<(String, Vec<u8>)> = (0..300)
        .map(|i| {
            (
                format!("agents/ada/batch/f{i}"),
                format!("file {i}").into_bytes(),
            )
        })
        .collect();
    let metas = sim
        .get("studio")
        .backend
        .put_many(items.clone())
        .await
        .unwrap();
    assert_eq!(metas.len(), 300);
    eventually("all of it is on the laptop", 20, || async {
        converged(&sim, &["studio", "laptop"]).await && heads(sim.get("laptop")).await.len() == 300
    })
    .await;
    for (k, v) in items.iter().step_by(37) {
        assert_eq!(&sim.get("laptop").store.get(k, None).await.unwrap().0, v);
    }
    sim.shutdown().await;
}
