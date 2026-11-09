//! Big futures off the awaiting function's stack frame.
//!
//! `fut.await` and `Box::pin(fut).await` both build `fut` in the awaiting
//! function's own `poll` frame before moving it into its state. An
//! unoptimized build gives every such future its own slot there, so an
//! async function that awaits several big futures (one per branch, say)
//! gets a `poll` frame as large as all of them together, and it is on the
//! stack on every poll, under everything it awaits. A create of a Space
//! went 2 MiB deep that way and overflowed a test thread.
//!
//! [`boxed`] builds the future inside its own short call and hands back the
//! heap pointer: the caller's frame keeps a pointer, not the future.

use std::future::Future;
use std::pin::Pin;

/// Builds the future `make` returns on the heap, outside the caller's
/// stack frame: `boxed(|| self.create(spec)).await`.
#[inline(never)]
///
/// The result is a `dyn Future + Send`, not `Pin<Box<Fut>>`: erasing the
/// type keeps the compiler from walking the whole nested async state
/// machine for every auto-trait check (E0275 through uniffi's `Send`
/// requirement on exported async fns).
pub fn boxed<'a, F, Fut>(make: F) -> Pin<Box<dyn Future<Output = Fut::Output> + Send + 'a>>
where
    F: FnOnce() -> Fut,
    Fut: Future + Send + 'a,
{
    Box::pin(make())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn the_future_runs_and_its_output_comes_back() {
        let n = 41;
        assert_eq!(super::boxed(|| async move { n + 1 }).await, 42);
    }

    /// The point of [`super::boxed`]: an async function that awaits a big
    /// future through it keeps a small state (a pointer), where awaiting
    /// the future directly embeds all of it.
    #[test]
    fn the_caller_holds_a_pointer_not_the_future() {
        async fn big() -> u8 {
            let buf = [1u8; 64 * 1024];
            tokio::task::yield_now().await;
            buf[100]
        }
        async fn direct() -> u8 {
            big().await
        }
        async fn through_boxed() -> u8 {
            super::boxed(big).await
        }
        assert!(std::mem::size_of_val(&direct()) >= 64 * 1024);
        assert!(std::mem::size_of_val(&through_boxed()) < 1024);
    }
}
