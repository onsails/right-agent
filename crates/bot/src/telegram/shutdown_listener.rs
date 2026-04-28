//! `ShutdownAware<L>` — wraps a teloxide `UpdateListener` so its update stream
//! ends as soon as a `CancellationToken` is cancelled.
//!
//! Why: teloxide 0.17's `axum_no_setup` listener feeds updates from an
//! `UnboundedReceiverStream`. The listener's `StopToken::stop()` only flips
//! a flag — it does not close the underlying `mpsc::UnboundedSender`. The
//! sender is closed lazily by the next incoming HTTP request that observes
//! `flag.is_stopped() == true`. During shutdown the cloudflared tunnel is
//! gone (or about to be) and Telegram has no way to deliver more updates,
//! so the channel never closes and `Dispatcher::dispatch_with_listener`
//! hangs in `stream.next()` until process-compose SIGKILLs us at
//! `shutdown.timeout_seconds`.
//!
//! The wrapper races each `inner.next()` against a `CancellationToken`;
//! once the token is cancelled the wrapped stream yields `None`, the
//! dispatcher loop hits `Either::Left(None) => break`, drains in-flight
//! workers (mpsc-fed worker tasks exit naturally when their `Worker.tx` is
//! dropped), and `dispatch_with_listener` returns cleanly.
//!
//! The inner `StopToken` is still exposed via `UpdateListener::stop_token`
//! so teloxide can also flip the `WebhookState.flag` to short-circuit any
//! late-arriving HTTP requests with `503 Service Unavailable`.

use futures::stream::TakeUntil;
use teloxide::stop::StopToken;
use teloxide::types::AllowedUpdate;
use teloxide::update_listeners::{AsUpdateStream, UpdateListener};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

/// Wraps an `UpdateListener` so its update stream terminates when `stop` is
/// cancelled, regardless of whether the inner transport closes its channel.
pub(crate) struct ShutdownAware<L> {
    inner: L,
    stop: CancellationToken,
}

impl<L> ShutdownAware<L> {
    pub(crate) fn new(inner: L, stop: CancellationToken) -> Self {
        Self { inner, stop }
    }
}

impl<'a, L> AsUpdateStream<'a> for ShutdownAware<L>
where
    L: AsUpdateStream<'a>,
{
    type StreamErr = L::StreamErr;
    type Stream = TakeUntil<L::Stream, WaitForCancellationFutureOwned>;

    fn as_stream(&'a mut self) -> Self::Stream {
        use futures::StreamExt;
        self.inner
            .as_stream()
            .take_until(self.stop.clone().cancelled_owned())
    }
}

impl<L> UpdateListener for ShutdownAware<L>
where
    L: UpdateListener,
{
    type Err = L::Err;

    fn stop_token(&mut self) -> StopToken {
        self.inner.stop_token()
    }

    fn hint_allowed_updates(&mut self, hint: &mut dyn Iterator<Item = AllowedUpdate>) {
        self.inner.hint_allowed_updates(hint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use futures::stream::Pending;
    use std::convert::Infallible;
    use std::time::Duration;
    use teloxide::types::Update;
    use teloxide::update_listeners::StatefulListener;

    type TestState = (Pending<Result<Update, Infallible>>, StopToken);

    /// HRTB-friendly stream selector — closures are not always inferred as
    /// `for<'a> FnMut(&'a mut St) -> ...`, but a `fn` item always is.
    fn select_stream(state: &mut TestState) -> &mut Pending<Result<Update, Infallible>> {
        &mut state.0
    }

    fn select_stop(state: &mut TestState) -> StopToken {
        state.1.clone()
    }

    /// Build a `StatefulListener` whose update stream never yields — this
    /// matches the production failure mode where teloxide's webhook stream
    /// keeps awaiting `recv()` on an `UnboundedReceiverStream` that no one
    /// closes.
    fn pending_listener() -> impl UpdateListener<Err = Infallible> {
        let (stop_token, _stop_flag) = teloxide::stop::mk_stop_token();
        let stream: Pending<Result<Update, Infallible>> = futures::stream::pending();
        StatefulListener::new((stream, stop_token), select_stream, select_stop)
    }

    /// The wrapped stream MUST yield `None` after the token is cancelled, even
    /// when the inner stream is permanently pending — this is the production
    /// failure mode where no incoming webhooks arrive after SIGTERM and the
    /// underlying mpsc channel never closes.
    #[tokio::test]
    async fn stream_ends_when_token_is_cancelled_on_pending_inner() {
        let listener = pending_listener();
        let stop = CancellationToken::new();
        let mut wrapped = ShutdownAware::new(listener, stop.clone());
        let stream = wrapped.as_stream();
        tokio::pin!(stream);

        stop.cancel();

        let next = tokio::time::timeout(Duration::from_millis(500), stream.next())
            .await
            .expect("wrapped stream did not yield within timeout");
        assert!(
            next.is_none(),
            "wrapped stream must yield None after cancellation, got Some(_)"
        );
    }

    /// Sanity check: without cancellation the wrapped stream stays pending —
    /// it must not spuriously terminate just because we layered the wrapper.
    #[tokio::test]
    async fn stream_stays_pending_without_cancellation() {
        let listener = pending_listener();
        let stop = CancellationToken::new();
        let mut wrapped = ShutdownAware::new(listener, stop);
        let stream = wrapped.as_stream();
        tokio::pin!(stream);

        let result = tokio::time::timeout(Duration::from_millis(100), stream.next()).await;
        assert!(
            result.is_err(),
            "wrapped stream yielded {:?} without cancellation",
            result.ok()
        );
    }
}
