// When the Space list changed (`Spaces.watch_list`), so an app reads it
// again when something happened instead of on a timer.

/// Hears that the Space list changed ([`Spaces::watch_list`]). Called on an
/// SDK worker thread; return quickly (read the list on your own queue).
#[uniffi::export(with_foreign)]
pub trait SpacesListListener: Send + Sync {
    /// The list changed (and once at the start): read it again.
    /// `generation` only increases while one daemon runs.
    fn on_changed(&self, generation: u64);
}

/// A running [`Spaces::watch_list`]. Dropping it also stops it.
#[derive(uniffi::Object)]
pub struct SpacesListWatch {
    stop: tokio::sync::watch::Sender<bool>,
    task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[uniffi::export]
impl SpacesListWatch {
    /// Ends the watch. No call is made after this returns.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
        if let Some(t) = self.task.lock().expect("watch task").take() {
            t.abort();
        }
    }
}

impl Drop for SpacesListWatch {
    fn drop(&mut self) {
        self.stop();
    }
}

#[uniffi::export]
impl Spaces {
    /// Tells `listener` when the Space list changed: once at the start, then
    /// on every change. This computer's Spaces within a second; your other
    /// computers on the mesh within about a second of connecting or going
    /// away; the others within 10 seconds. Reopens by itself if the daemon
    /// restarts (and then calls `listener` again). Stop it with
    /// [`SpacesListWatch::stop`].
    pub fn watch_list(&self, listener: Arc<dyn SpacesListListener>) -> Arc<SpacesListWatch> {
        let (stop, rx) = tokio::sync::watch::channel(false);
        let host = self.host.clone();
        let task = super::runtime().spawn(async move {
            match host {
                Host::Embedded(s) => embedded_list_watch(s, rx, listener).await,
                Host::Daemon(d) => daemon_list_watch(d, rx, listener).await,
            }
        });
        Arc::new(SpacesListWatch {
            stop,
            task: std::sync::Mutex::new(Some(task)),
        })
    }
}

async fn embedded_list_watch(
    spaces: cua_spaces::Spaces,
    mut stop: tokio::sync::watch::Receiver<bool>,
    listener: Arc<dyn SpacesListListener>,
) {
    let mut watch = spaces.watch_list();
    listener.on_changed(watch.generation());
    loop {
        tokio::select! {
            g = watch.changed() => match g {
                Some(g) => listener.on_changed(g),
                None => return,
            },
            _ = stop.changed() => return,
        }
    }
}

async fn daemon_list_watch(
    daemon: cua_daemon::client::DaemonClient,
    mut stop: tokio::sync::watch::Receiver<bool>,
    listener: Arc<dyn SpacesListListener>,
) {
    loop {
        if *stop.borrow() {
            return;
        }
        let opened = daemon
            .spaces()
            .watch_spaces(dpb::WatchSpacesRequest {})
            .await;
        if let Ok(stream) = opened {
            let mut stream = stream.into_inner();
            loop {
                tokio::select! {
                    msg = stream.message() => match msg {
                        Ok(Some(m)) => listener.on_changed(m.generation),
                        // The daemon ended the stream or went away.
                        Ok(None) | Err(_) => break,
                    },
                    _ = stop.changed() => return,
                }
            }
        }
        // A daemon that is not up (or predates this): try again.
        tokio::select! {
            _ = tokio::time::sleep(REOPEN_AFTER) => {}
            _ = stop.changed() => return,
        }
    }
}
