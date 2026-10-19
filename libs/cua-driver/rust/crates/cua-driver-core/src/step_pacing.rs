//! How an action is paced when it is one step of a batch.
//!
//! A single tool call waits for two things that exist for the person watching
//! and for the next read: the agent cursor's glide to the target, and the
//! post-action window-change observation that reports a menu, dialog or new
//! window the action opened. Measured on one agent run, those two waits are
//! most of an action's latency: a click took about 1.4 s, of which the glide
//! floor and the observation were roughly 0.9 s.
//!
//! Inside `run_steps` and `run_script` the caller has already decided to chain
//! actions, and the next step starts right away. The batch executors mark each
//! step with [`StepPacing::Batched`], and the platform adapters then shorten
//! both waits (see [`BATCH_GLIDE_WAIT`] and [`BATCH_REPORT_WINDOW`]). The
//! cursor keeps gliding and the window watch keeps running detached, so only
//! the time the step is held up changes. A direct call keeps the full waits,
//! and so does the last step of a `run_steps` batch, which reports what the
//! batch left open.
//!
//! The marker is a task-local set by the batch executors. Callers cannot set it
//! through arguments, and a daemon operator can turn the shortening off with
//! [`BATCH_PACING_ENV`]. See `Skills/cua-driver/EMBEDDING.md`.

use std::future::Future;
use std::time::Duration;

/// Daemon environment switch. `full` keeps the full cursor glide and the full
/// window-change report for every step of a batch, as for a direct call. Any
/// other value, or none, applies the batch pacing.
pub const BATCH_PACING_ENV: &str = "CUA_DRIVER_BATCH_PACING";

/// Longest a batched step waits for the agent cursor to arrive. The glide
/// itself still plays to the end.
pub const BATCH_GLIDE_WAIT: Duration = Duration::from_millis(120);

/// How long a batched step waits for a window change to show before it returns.
/// The watch continues detached for the rest of its deadline, and a late change
/// is reported on the next action's result.
pub const BATCH_REPORT_WINDOW: Duration = Duration::from_millis(100);

/// The role of the action being dispatched within the current tool call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StepPacing {
    /// A direct call, or the last step of a batch: full waits.
    #[default]
    Single,
    /// A step that another step follows: shortened waits.
    Batched,
}

tokio::task_local! {
    static CURRENT: StepPacing;
}

/// Run `future` with `pacing` as the pacing of every action it dispatches.
pub async fn scope<F: Future>(pacing: StepPacing, future: F) -> F::Output {
    CURRENT.scope(pacing, future).await
}

/// Whether the action being dispatched is a step of a `run_steps` batch or a
/// `run_script` program, as opposed to a call the agent made directly.
pub fn in_batch() -> bool {
    CURRENT.try_with(|_| ()).is_ok()
}

/// The pacing of the action being dispatched. [`StepPacing::Single`] outside a
/// batch, and whenever the operator set [`BATCH_PACING_ENV`] to `full`.
pub fn current() -> StepPacing {
    let pacing = CURRENT.try_with(|pacing| *pacing).unwrap_or_default();
    resolve(pacing, std::env::var(BATCH_PACING_ENV).ok().as_deref())
}

/// Apply the operator switch to a step's pacing. Pure, so tests do not touch
/// the process environment.
fn resolve(pacing: StepPacing, env: Option<&str>) -> StepPacing {
    match env {
        Some(value) if value.trim().eq_ignore_ascii_case("full") => StepPacing::Single,
        _ => pacing,
    }
}

/// The longest a step waits for the cursor to arrive, given the arrival limit
/// of a direct call.
pub fn glide_wait_limit(direct: Duration) -> Duration {
    match current() {
        StepPacing::Single => direct,
        StepPacing::Batched => direct.min(BATCH_GLIDE_WAIT),
    }
}

/// How long the step's result waits for a window change, given the report
/// window of a direct call.
pub fn report_window(direct: Duration) -> Duration {
    match current() {
        StepPacing::Single => direct,
        StepPacing::Batched => direct.min(BATCH_REPORT_WINDOW),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_direct_call_keeps_the_full_waits() {
        assert_eq!(current(), StepPacing::Single);
        let direct = Duration::from_secs(8);
        assert_eq!(glide_wait_limit(direct), direct);
        assert_eq!(
            report_window(Duration::from_millis(300)),
            Duration::from_millis(300)
        );
    }

    #[tokio::test]
    async fn a_batched_step_caps_both_waits() {
        scope(StepPacing::Batched, async {
            assert_eq!(current(), StepPacing::Batched);
            assert_eq!(glide_wait_limit(Duration::from_secs(8)), BATCH_GLIDE_WAIT);
            assert_eq!(
                report_window(Duration::from_millis(300)),
                BATCH_REPORT_WINDOW
            );
        })
        .await;
    }

    #[tokio::test]
    async fn the_caps_never_lengthen_a_shorter_wait() {
        scope(StepPacing::Batched, async {
            let short = Duration::from_millis(40);
            assert_eq!(glide_wait_limit(short), short);
            assert_eq!(report_window(short), short);
        })
        .await;
    }

    #[tokio::test]
    async fn a_scope_marks_its_calls_as_inside_a_batch() {
        assert!(!in_batch());
        scope(StepPacing::Single, async {
            assert!(in_batch(), "the last step of a batch is still a batch step");
        })
        .await;
        scope(StepPacing::Batched, async {
            assert!(in_batch());
        })
        .await;
        assert!(!in_batch());
    }

    #[tokio::test]
    async fn a_nested_single_scope_restores_full_waits() {
        scope(StepPacing::Batched, async {
            scope(StepPacing::Single, async {
                assert_eq!(current(), StepPacing::Single);
            })
            .await;
            assert_eq!(current(), StepPacing::Batched);
        })
        .await;
    }

    #[test]
    fn the_operator_switch_turns_the_shortening_off() {
        assert_eq!(resolve(StepPacing::Batched, None), StepPacing::Batched);
        assert_eq!(resolve(StepPacing::Batched, Some("")), StepPacing::Batched);
        assert_eq!(
            resolve(StepPacing::Batched, Some("fast")),
            StepPacing::Batched
        );
        assert_eq!(
            resolve(StepPacing::Batched, Some(" Full ")),
            StepPacing::Single
        );
        assert_eq!(
            resolve(StepPacing::Single, Some("full")),
            StepPacing::Single
        );
    }
}
