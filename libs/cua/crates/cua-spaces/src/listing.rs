//! The Space list as known now ([`Spaces::list_known`]).
//!
//! This machine's own Spaces are in the registry, a local file: they are
//! listed at once. The account's other computers come from the relay
//! directory, a network read that can take long (the relay or the mesh
//! still connecting right after a daemon restart). A list that waits for
//! it shows nothing at all meanwhile, not even this machine's Spaces, and
//! an app reading that as "no Spaces" draws its first-run page.
//!
//! So the read of the account's computers runs on its own (one at a time,
//! [`Spaces::list_known`] waits for it only a moment), the directory it
//! last answered is kept in `relay-directory.json` across restarts, and
//! every listing says how far it got ([`ListProgress`]): whether some
//! computers have not answered yet (the list is incomplete, and an app
//! must not call it empty), how many, and whether the rows of the other
//! computers are the last known ones rather than this process's own read.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::{Result, SpaceInfo, Spaces};

/// The relay directory as last read, so a restarted process lists the
/// account's computers before the relay answers again.
pub(crate) const RELAY_DIRECTORY_FILE: &str = "relay-directory.json";

/// How long a list read waits for the account's computers before it
/// answers with what it knows. The relay normally answers well within it.
pub const LIST_WAIT: Duration = Duration::from_millis(1500);

/// How far a listing got.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListProgress {
    /// Some of the account's computers have not answered yet: the list may
    /// grow, and an empty one is not "no Spaces".
    pub incomplete: bool,
    /// How many computers are still being reached (0 when unknown, or when
    /// the list is complete).
    pub reaching: u32,
    /// The other computers' rows are the last known ones (from before this
    /// process, or from before a failed read), not confirmed by the relay.
    pub stale: bool,
    /// Why the account's computers could not be read, when they could not
    /// (the list is complete as far as it goes; this machine's Spaces are
    /// in it).
    pub notice: String,
}

/// The Spaces known now, and how far the read got.
#[derive(Clone, Debug, Default)]
pub struct SpaceListing {
    pub spaces: Vec<SpaceInfo>,
    pub progress: ListProgress,
    /// Spaces your other computers are creating now
    /// ([`Spaces::creating_elsewhere`]).
    pub creating: Vec<crate::resources::creates::CreatingElsewhere>,
}

/// How a read of the account's computers ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RoundEnd {
    /// The relay answered (or there is no account to ask).
    Answered,
    /// It could not be read: why.
    Failed(String),
}

/// The read of the account's computers in flight, and the last that ended.
#[derive(Default)]
pub(crate) struct AccountRounds {
    in_flight: std::sync::Mutex<Option<watch::Receiver<Option<RoundEnd>>>>,
    last: std::sync::Mutex<Option<(Instant, RoundEnd)>>,
}

impl AccountRounds {
    /// Another account: its first read is still ahead.
    pub(crate) fn reset(&self) {
        *self.in_flight.lock().expect("rounds") = None;
        *self.last.lock().expect("rounds") = None;
    }
}

/// What `relay-directory.json` holds.
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedDirectory {
    /// The relay it was read from (a directory of another relay is not
    /// listed).
    relay: String,
    machines: Vec<crate::relay::RelayMachine>,
}

/// The directory saved for `relay`, when there is one.
pub(crate) fn load_directory(
    home: &std::path::Path,
    relay: &str,
) -> Vec<crate::relay::RelayMachine> {
    std::fs::read(home.join(RELAY_DIRECTORY_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<SavedDirectory>(&b).ok())
        .filter(|d| d.relay == relay)
        .map(|d| d.machines)
        .unwrap_or_default()
}

/// Keeps the directory `relay` answered. Best effort.
pub(crate) fn save_directory(
    home: &std::path::Path,
    relay: &str,
    machines: &[crate::relay::RelayMachine],
) {
    let path = home.join(RELAY_DIRECTORY_FILE);
    let write = || -> Result<()> {
        cua_home::guard_write(&path)?;
        let saved = SavedDirectory {
            relay: relay.to_string(),
            machines: machines.to_vec(),
        };
        cua_home::write_private(&path, &serde_json::to_vec_pretty(&saved)?)?;
        Ok(())
    };
    if let Err(e) = write() {
        tracing::debug!(error = %e, "{RELAY_DIRECTORY_FILE} not written");
    }
}

/// Forgets the saved directory (signed out: the account's computers are not
/// listed any more).
pub(crate) fn forget_directory(home: &std::path::Path) {
    let path = home.join(RELAY_DIRECTORY_FILE);
    if path.exists() && cua_home::guard_write(&path).is_ok() {
        let _ = std::fs::remove_file(path);
    }
}

impl Spaces {
    /// The Spaces known now: this machine's at once, and the account's other
    /// computers as far as they answered within `wait`.
    ///
    /// Unlike [`Spaces::list_all`], a slow or unreachable relay never holds
    /// up this machine's own Spaces, and never fails the list: the other
    /// computers' rows are the last known ones meanwhile (also across a
    /// restart, from `relay-directory.json`), and [`ListProgress`] says so.
    /// The read it starts goes on after `wait`; the next call picks up its
    /// answer. Once one read has ended, later calls answer with the last
    /// read's rows while the next one runs, so a relay slower than `wait`
    /// does not make every list incomplete.
    pub async fn list_known(&self, wait: Duration) -> Result<SpaceListing> {
        let mut round = self.account_round();
        let ended = tokio::time::timeout(wait, round.wait_for(Option::is_some))
            .await
            .ok()
            .and_then(|r| r.ok().and_then(|v| v.clone()));
        let end = ended.or_else(|| {
            self.inner
                .rounds
                .last
                .lock()
                .expect("rounds")
                .as_ref()
                .map(|(_, e)| e.clone())
        });
        let spaces = self.list()?;
        let confirmed = self
            .inner
            .relay_confirmed
            .load(std::sync::atomic::Ordering::Acquire);
        let others = !self
            .inner
            .relay_cache
            .lock()
            .expect("relay cache")
            .is_empty();
        let progress = match end {
            None => ListProgress {
                incomplete: true,
                reaching: self.computers_to_reach(),
                stale: others && !confirmed,
                notice: String::new(),
            },
            Some(RoundEnd::Answered) => ListProgress {
                stale: others && !confirmed,
                ..ListProgress::default()
            },
            Some(RoundEnd::Failed(why)) => ListProgress {
                incomplete: false,
                reaching: 0,
                stale: others,
                notice: why,
            },
        };
        Ok(SpaceListing {
            spaces,
            progress,
            creating: self.creating_elsewhere(),
        })
    }

    /// The account's computers the directory last listed, other than this
    /// one and the Spaces a computer provides (they answer with it).
    fn computers_to_reach(&self) -> u32 {
        let this = self.this_relay_machine_id();
        let cache = self.inner.relay_cache.lock().expect("relay cache");
        let n = cache
            .iter()
            .filter(|m| m.host.as_deref().is_none_or(str::is_empty))
            .filter(|m| this.as_deref() != Some(m.id.as_str()))
            .count();
        u32::try_from(n).unwrap_or(u32::MAX)
    }

    /// The read of the account's computers in flight, or a new one.
    fn account_round(&self) -> watch::Receiver<Option<RoundEnd>> {
        let mut slot = self.inner.rounds.in_flight.lock().expect("rounds");
        if let Some(rx) = slot.as_ref()
            && rx.borrow().is_none()
        {
            return rx.clone();
        }
        let (tx, rx) = watch::channel(None);
        *slot = Some(rx.clone());
        drop(slot);
        let this = self.clone();
        let tx = Arc::new(tx);
        tokio::spawn(async move {
            let end = this.read_account_computers().await;
            *this.inner.rounds.last.lock().expect("rounds") = Some((Instant::now(), end.clone()));
            let _ = tx.send(Some(end));
        });
        rx
    }

    /// Durable Spaces and the relay directory, read again.
    async fn read_account_computers(&self) -> RoundEnd {
        if let Err(e) = self.durable_spaces().await {
            tracing::debug!(error = %e, "durable Spaces not listed");
        }
        let Some(account) = self.relay_account() else {
            return RoundEnd::Answered;
        };
        match self.relay_machines().await {
            Ok(_) => RoundEnd::Answered,
            Err(e) if e.tag() == "unauthenticated" && !account.tokens.signed_in().await => {
                tracing::debug!("relay directory skipped: not signed in");
                self.forget_account_computers();
                RoundEnd::Answered
            }
            Err(e) => RoundEnd::Failed(e.to_string()),
        }
    }

    /// Signed out: the account's computers leave the list, here and on disk.
    pub(crate) fn forget_account_computers(&self) {
        self.inner.relay_cache.lock().expect("relay cache").clear();
        self.inner
            .relay_confirmed
            .store(false, std::sync::atomic::Ordering::Release);
        forget_directory(self.home_dir());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::{RelayAccount, StaticToken};
    use cua_proto::daemon::v1 as dpb;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A relay whose directory answers `machines` only once `open` is sent
    /// (`false` keeps every read hanging).
    async fn slow_relay(
        machines: serde_json::Value,
    ) -> (
        String,
        watch::Sender<bool>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (open, gate) = watch::channel(false);
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = reads.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let mut gate = gate.clone();
                let body = serde_json::json!({ "machines": machines }).to_string();
                let counted = counted.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let _ = sock.read(&mut buf).await;
                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if gate.wait_for(|o| *o).await.is_err() {
                        return;
                    }
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(reply.as_bytes()).await;
                });
            }
        });
        (url, open, reads)
    }

    fn local_space(id: &str) -> dpb::Space {
        dpb::Space {
            id: id.into(),
            name: id.trim_start_matches("local:").into(),
            os: "macos".into(),
            ..Default::default()
        }
    }

    fn two_computers() -> serde_json::Value {
        serde_json::json!([
            {"id": "aaaaaaaaaaaaaaaa", "name": "Studio", "online": true},
            {"id": "bbbbbbbbbbbbbbbb", "name": "Builder", "online": false},
        ])
    }

    fn spaces_in(home: &std::path::Path) -> Spaces {
        let spaces = Spaces::builder().home(home.to_path_buf()).build();
        spaces
            .registry()
            .upsert(local_space("local:space-mac1"), Default::default())
            .unwrap();
        spaces
    }

    fn ids(l: &SpaceListing) -> Vec<&str> {
        l.spaces.iter().map(|s| s.id.as_str()).collect()
    }

    /// This machine's Spaces are listed while the relay has not answered,
    /// and the listing says it is incomplete (not empty).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn this_machines_spaces_are_listed_before_the_relay_answers() {
        let home = tempfile::tempdir().unwrap();
        let (url, open, _) = slow_relay(two_computers()).await;
        let spaces = spaces_in(home.path());
        spaces.set_relay(Some(RelayAccount::new(
            &url,
            Arc::new(StaticToken("t".into())),
        )));

        let started = Instant::now();
        let first = spaces.list_known(Duration::from_millis(200)).await.unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(ids(&first), ["local:space-mac1"]);
        assert!(first.progress.incomplete, "{:?}", first.progress);
        // Nothing known of the account's computers yet.
        assert_eq!(first.progress.reaching, 0);
        assert!(!first.progress.stale);

        // The relay answers: the same read completes the list.
        open.send(true).unwrap();
        let done = spaces.list_known(Duration::from_secs(5)).await.unwrap();
        assert!(!done.progress.incomplete, "{:?}", done.progress);
        assert!(!done.progress.stale);
        assert_eq!(
            ids(&done),
            [
                "local:space-mac1",
                "relay:aaaaaaaaaaaaaaaa",
                "relay:bbbbbbbbbbbbbbbb"
            ]
        );
    }

    /// A restart lists the last known computers at once (stale, and
    /// incomplete while the relay is being reached, with how many), and
    /// the relay's answer confirms them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_last_known_computers_survive_a_restart() {
        let home = tempfile::tempdir().unwrap();
        let (url, open, _) = slow_relay(two_computers()).await;
        open.send(true).unwrap();
        let account = || RelayAccount::new(&url, Arc::new(StaticToken("t".into())));
        {
            let before = spaces_in(home.path());
            before.set_relay(Some(account()));
            let l = before.list_known(Duration::from_secs(5)).await.unwrap();
            assert_eq!(l.spaces.len(), 3);
        }
        assert!(home.path().join(RELAY_DIRECTORY_FILE).exists());

        // The new process: the relay does not answer yet.
        open.send(false).unwrap();
        let after = Spaces::builder().home(home.path().to_path_buf()).build();
        after.set_relay(Some(account()));
        let l = after.list_known(Duration::from_millis(200)).await.unwrap();
        assert_eq!(
            ids(&l),
            [
                "local:space-mac1",
                "relay:aaaaaaaaaaaaaaaa",
                "relay:bbbbbbbbbbbbbbbb"
            ]
        );
        assert_eq!(
            l.progress,
            ListProgress {
                incomplete: true,
                reaching: 2,
                stale: true,
                notice: String::new(),
            }
        );
        // An offline computer keeps its row, offline.
        let builder = l.spaces.iter().find(|s| s.id.ends_with("bbbb")).unwrap();
        assert_eq!(builder.status, "offline", "{builder:?}");

        open.send(true).unwrap();
        let l = after.list_known(Duration::from_secs(5)).await.unwrap();
        assert_eq!(l.progress, ListProgress::default());
        assert_eq!(l.spaces.len(), 3);
    }

    /// Another relay's saved directory is not listed.
    #[test]
    fn a_directory_of_another_relay_is_not_listed() {
        let home = tempfile::tempdir().unwrap();
        let m: crate::relay::RelayMachine =
            serde_json::from_value(serde_json::json!({"id": "aaaaaaaaaaaaaaaa"})).unwrap();
        save_directory(home.path(), "https://relay.one", &[m]);
        assert_eq!(load_directory(home.path(), "https://relay.one").len(), 1);
        assert!(load_directory(home.path(), "https://relay.two").is_empty());
    }

    /// A relay that fails: the list is complete as far as it goes (this
    /// machine's Spaces, the last known computers), stale, and says why.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_relay_read_lists_what_is_known_and_says_why() {
        let home = tempfile::tempdir().unwrap();
        let m: crate::relay::RelayMachine = serde_json::from_value(
            serde_json::json!({"id": "aaaaaaaaaaaaaaaa", "name": "Studio", "online": true}),
        )
        .unwrap();
        // Nothing listens there.
        let dead = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        save_directory(home.path(), &dead, &[m]);
        let spaces = spaces_in(home.path());
        spaces.set_relay(Some(RelayAccount::new(
            &dead,
            Arc::new(StaticToken("t".into())),
        )));
        let l = spaces.list_known(Duration::from_secs(10)).await.unwrap();
        assert_eq!(ids(&l), ["local:space-mac1", "relay:aaaaaaaaaaaaaaaa"]);
        assert!(!l.progress.incomplete, "{:?}", l.progress);
        assert!(l.progress.stale);
        assert!(!l.progress.notice.is_empty());
    }

    /// Once a read has ended, a slow relay does not make later lists
    /// incomplete: they answer with the last read while the next one runs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_slow_relay_after_the_first_answer_keeps_the_list_complete() {
        let home = tempfile::tempdir().unwrap();
        let (url, open, reads) = slow_relay(two_computers()).await;
        open.send(true).unwrap();
        let spaces = spaces_in(home.path());
        spaces.set_relay(Some(RelayAccount::new(
            &url,
            Arc::new(StaticToken("t".into())),
        )));
        assert!(
            !spaces
                .list_known(Duration::from_secs(5))
                .await
                .unwrap()
                .progress
                .incomplete
        );
        open.send(false).unwrap();
        for _ in 0..3 {
            let l = spaces.list_known(Duration::from_millis(100)).await.unwrap();
            assert_eq!(l.progress, ListProgress::default());
            assert_eq!(l.spaces.len(), 3);
        }
        // One read at a time: the hanging one is joined, not repeated.
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    /// Signed out: the account's computers leave the list and the disk.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn signing_out_forgets_the_saved_computers() {
        let home = tempfile::tempdir().unwrap();
        let (url, open, _) = slow_relay(two_computers()).await;
        open.send(true).unwrap();
        let spaces = spaces_in(home.path());
        spaces.set_relay(Some(RelayAccount::new(
            &url,
            Arc::new(StaticToken("t".into())),
        )));
        assert_eq!(
            spaces
                .list_known(Duration::from_secs(5))
                .await
                .unwrap()
                .spaces
                .len(),
            3
        );
        spaces.set_relay(Some(RelayAccount::new(
            &url,
            Arc::new(StaticToken(String::new())),
        )));
        let l = spaces.list_known(Duration::from_secs(5)).await.unwrap();
        assert_eq!(ids(&l), ["local:space-mac1"]);
        assert_eq!(l.progress, ListProgress::default());
        assert!(!home.path().join(RELAY_DIRECTORY_FILE).exists());
    }
}
