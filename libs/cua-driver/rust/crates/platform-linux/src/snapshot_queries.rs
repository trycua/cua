//! Bounded, borrowed observation queries. No tasks outlive their snapshot.

use std::future::Future;

use futures_util::{stream, StreamExt};
use tokio::time::{timeout_at, Instant};

pub(crate) const SNAPSHOT_QUERY_CONCURRENCY: usize = 8;

/// Keep the existing pre-offset rule for unrealized or degenerate components.
pub(crate) fn plausible_raw_extents((x, y, w, h): (i32, i32, i32, i32)) -> bool {
    !(x == i32::MIN || y == i32::MIN || x < -16384 || y < -16384 || w <= 1 || h <= 1)
}

/// Keep original element indices while overlapping independent read-only calls.
/// The deadline covers the whole collection, including queued work. Dropping
/// this future drops every in-flight query; nothing is spawned or cached.
pub(crate) async fn collect_indexed<T, F>(
    queries: impl IntoIterator<Item = F>,
    deadline: Instant,
) -> Vec<(usize, T)>
where
    F: Future<Output = Option<(usize, T)>>,
{
    let pending = stream::iter(queries).buffer_unordered(SNAPSHOT_QUERY_CONCURRENCY);
    futures_util::pin_mut!(pending);
    let mut collected = Vec::new();
    while Instant::now() < deadline {
        match timeout_at(deadline, pending.next()).await {
            Ok(Some(Some(value))) => collected.push(value),
            Ok(Some(None)) => {}
            Ok(None) | Err(_) => break,
        }
    }
    collected.sort_unstable_by_key(|(index, _)| *index);
    collected
}

#[cfg(test)]
mod tests {
    use super::{collect_indexed, plausible_raw_extents, SNAPSHOT_QUERY_CONCURRENCY};
    use std::{cell::Cell, future::pending, rc::Rc, time::Duration};
    use tokio::time::{sleep, timeout, Instant};

    struct Active(Rc<Cell<usize>>);

    impl Drop for Active {
        fn drop(&mut self) {
            self.0.set(self.0.get() - 1);
        }
    }

    #[test]
    fn raw_extent_acceptance_is_unchanged_before_coordinate_offsets() {
        for raw in [
            (i32::MIN, 0, 10, 10),
            (0, i32::MIN, 10, 10),
            (-16385, 0, 10, 10),
            (0, -16385, 10, 10),
            (0, 0, 1, 10),
            (0, 0, 10, 1),
            (0, 0, -1, 10),
        ] {
            assert!(!plausible_raw_extents(raw), "{raw:?}");
        }
        for raw in [(0, 0, 2, 2), (-16384, 0, 2, 2), (0, -16384, 2, 2)] {
            assert!(plausible_raw_extents(raw), "{raw:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn preserves_original_indices_despite_out_of_order_completion_and_omissions() {
        let completed = Rc::new(Cell::new(0));
        let queries = [19, 3, 41, 7].into_iter().map(|index| {
            let completed = completed.clone();
            async move {
                sleep(Duration::from_millis(index as u64)).await;
                completed.set(completed.get() + 1);
                (index != 7).then_some((index, index * 2))
            }
        });
        let result = collect_indexed(queries, Instant::now() + Duration::from_secs(1)).await;
        assert_eq!(result, [(3, 6), (19, 38), (41, 82)]);
        assert_eq!(completed.get(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn concurrency_is_bounded_and_queued_work_is_not_started_after_deadline() {
        let active = Rc::new(Cell::new(0));
        let started = Rc::new(Cell::new(0));
        let queries = (0..100).map(|index| {
            let (active, started) = (active.clone(), started.clone());
            async move {
                started.set(started.get() + 1);
                active.set(active.get() + 1);
                assert!(active.get() <= SNAPSHOT_QUERY_CONCURRENCY);
                let _guard = Active(active);
                pending::<()>().await;
                Some((index, ()))
            }
        });
        let result = collect_indexed(queries, Instant::now() + Duration::from_secs(1)).await;
        assert!(result.is_empty());
        assert_eq!(started.get(), SNAPSHOT_QUERY_CONCURRENCY);
        assert_eq!(active.get(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn retains_completed_values_when_sibling_queries_stall() {
        let active = Rc::new(Cell::new(0));
        let queries = (0..20).map(|index| {
            let active = active.clone();
            async move {
                active.set(active.get() + 1);
                let _guard = Active(active);
                if index % 2 == 0 {
                    pending::<()>().await;
                }
                sleep(Duration::from_millis(1)).await;
                Some((index, index))
            }
        });
        let result = collect_indexed(queries, Instant::now() + Duration::from_secs(1)).await;
        assert_eq!(
            result,
            (1..14).step_by(2).map(|i| (i, i)).collect::<Vec<_>>()
        );
        assert_eq!(active.get(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn caller_cancellation_drops_every_borrowed_query() {
        let active = Rc::new(Cell::new(0));
        let queries = (0..20).map(|index| {
            let active = active.clone();
            async move {
                active.set(active.get() + 1);
                let _guard = Active(active);
                pending::<()>().await;
                Some((index, ()))
            }
        });
        let result = timeout(
            Duration::from_secs(1),
            collect_indexed(queries, Instant::now() + Duration::from_secs(20)),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(active.get(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn expired_deadline_does_not_poll_any_query() {
        let queries = std::iter::once(async {
            panic!("expired work must not start");
            #[allow(unreachable_code)]
            Some((0, ()))
        });
        assert!(collect_indexed(queries, Instant::now()).await.is_empty());
    }
}
