//! Keyed async gates: callers with the same key serialize, and callers with
//! different keys proceed independently.
//!
//! Browser mutation and reconnect leadership both key on process identity
//! plus one CDP resource. They differ only in the key, so they share this map.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

pub(crate) struct KeyedGates<K> {
    gates: Mutex<HashMap<K, Arc<AsyncMutex<()>>>>,
}

impl<K> Default for KeyedGates<K> {
    fn default() -> Self {
        Self {
            gates: Mutex::new(HashMap::new()),
        }
    }
}

impl<K: Eq + Hash> KeyedGates<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Wait for exclusive ownership of `key`. The gate is held until the
    /// returned guard drops.
    pub async fn lock(&self, key: K) -> OwnedMutexGuard<()> {
        let gate = self
            .gates
            .lock()
            .unwrap()
            .entry(key)
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone();
        gate.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::KeyedGates;
    use std::time::Duration;

    #[tokio::test]
    async fn same_key_serializes_and_different_keys_do_not_block() {
        let gates = KeyedGates::new();
        let first = gates.lock("a").await;

        assert!(
            tokio::time::timeout(Duration::from_millis(20), gates.lock("a"))
                .await
                .is_err(),
            "the same key must wait for the live holder"
        );
        tokio::time::timeout(Duration::from_millis(20), gates.lock("b"))
            .await
            .expect("an independent key must not block");

        drop(first);
        let _next = gates.lock("a").await;
    }
}
