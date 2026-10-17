//! Relay Architecture v2 - Wave A: daemon-owned pending effect worker.
//!
//! `PendingEffect` records were previously only ever inserted and read back
//! for snapshots; nothing executed or reconciled them, so an effect could sit
//! `pending` forever. This module closes that gap: it reconciles effects
//! stuck `in_flight` past a crash (nothing resumed them), and dispatches
//! `pending` effects to a registered handler.
//!
//! No production caller queues an effect yet in this wave (no corrected path
//! this wave cuts over produces one) - the registry starts empty and an
//! unregistered `effect_type` fails loudly rather than being silently
//! dropped, so this proves the mechanism and gives Wave B a slot to register
//! real handlers into.

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::sync::watch;

use relay_store::{DaemonFence, PendingEffectRecord, SqliteStore, StoreError};

pub enum EffectOutcome {
    Success(String),
    Failure(String),
}

pub trait EffectHandler: Send + Sync {
    fn handle(
        &self,
        effect: &PendingEffectRecord,
    ) -> impl std::future::Future<Output = EffectOutcome> + Send;
}

/// Maps `effect_type` to the handler responsible for executing it.
#[derive(Default)]
pub struct EffectWorkerRegistry {
    handlers: HashMap<String, Arc<dyn BoxedEffectHandler>>,
}

impl EffectWorkerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<H>(&mut self, effect_type: impl Into<String>, handler: H)
    where
        H: EffectHandler + 'static,
    {
        self.handlers
            .insert(effect_type.into(), Arc::new(HandlerAdapter(handler)));
    }

    async fn dispatch(&self, effect: &PendingEffectRecord) -> Result<EffectOutcome, String> {
        match self.handlers.get(&effect.effect_type) {
            Some(handler) => Ok(handler.handle_boxed(effect).await),
            None => Err(format!(
                "no effect handler registered for effect_type '{}'",
                effect.effect_type
            )),
        }
    }
}

// `EffectHandler::handle` returns an opaque `impl Future`, which can't be
// named in a trait object's method signature directly. This adapter boxes
// the future so handlers can be stored as `Arc<dyn ...>` in the registry.
trait BoxedEffectHandler: Send + Sync {
    fn handle_boxed<'a>(
        &'a self,
        effect: &'a PendingEffectRecord,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = EffectOutcome> + Send + 'a>>;
}

struct HandlerAdapter<H>(H);

impl<H: EffectHandler + Send + Sync> BoxedEffectHandler for HandlerAdapter<H> {
    fn handle_boxed<'a>(
        &'a self,
        effect: &'a PendingEffectRecord,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = EffectOutcome> + Send + 'a>> {
        Box::pin(self.0.handle(effect))
    }
}

/// Polls for effect work: reconciles effects stuck `in_flight` past a
/// staleness threshold (crash recovery), then dispatches a batch of `pending`
/// effects to their registered handler.
///
/// Every write this worker makes is fenced on the daemon lease it was started
/// with and conditional on the effect state it observed, because the worker
/// mutates the same canonical effect ledger as the rest of the daemon: a
/// successor that settles an effect must not be overwritten, and must not have a
/// second, contradictory result row added beside its own. Losing the lease stops
/// the worker outright rather than letting it keep polling without authority.
pub async fn run_effect_worker(
    store: SqliteStore,
    fence: DaemonFence,
    registry: EffectWorkerRegistry,
    poll_interval: Duration,
    max_attempts: i64,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(poll_interval);
    // A crash-stuck `in_flight` effect is one whose staleness exceeds this
    // multiple of the poll interval - generous enough that a normal in-flight
    // handler invocation is never mistaken for a crash.
    let stale_after = poll_interval * 6;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if !reconcile_stale_in_flight(&store, &fence, stale_after, max_attempts).await {
                    return;
                }
                if !process_pending_batch(&store, &fence, &registry).await {
                    return;
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

/// Reports whether the worker may keep running: `false` once the lease has been
/// taken over, in which case nothing it writes can commit anyway.
fn may_continue(err: &StoreError) -> bool {
    !matches!(err, StoreError::DaemonAuthorityLost { .. })
}

async fn reconcile_stale_in_flight(
    store: &SqliteStore,
    fence: &DaemonFence,
    stale_after: Duration,
    max_attempts: i64,
) -> bool {
    let stale = match store
        .list_stale_in_flight_effects(stale_after.as_secs() as i64)
        .await
    {
        Ok(effects) => effects,
        Err(err) => {
            tracing::error!(error = %err, "effect worker: failed to list stale in-flight effects");
            return true;
        }
    };

    for effect in stale {
        let (state, last_error) = if effect.attempts >= max_attempts {
            (
                "failed",
                Some("exceeded max retry attempts after crash recovery"),
            )
        } else {
            ("pending", None)
        };

        // Conditional on the effect still being in flight: an effect that a
        // successor already settled must not be re-opened by this instance.
        match store
            .update_effect_state_fenced(&effect.id, state, last_error, "in_flight", fence)
            .await
        {
            Ok(_) => {}
            Err(err) if !may_continue(&err) => {
                tracing::error!(
                    effect_id = %effect.id,
                    error = %err,
                    "effect worker: lost the daemon lease; stopping instead of writing stale effect state"
                );
                return false;
            }
            Err(err) => {
                tracing::error!(effect_id = %effect.id, error = %err, "effect worker: failed to reconcile stale in-flight effect");
            }
        }
    }

    true
}

async fn process_pending_batch(
    store: &SqliteStore,
    fence: &DaemonFence,
    registry: &EffectWorkerRegistry,
) -> bool {
    const BATCH_LIMIT: usize = 20;

    let pending = match store.list_pending_effects(Some("pending")).await {
        Ok(effects) => effects,
        Err(err) => {
            tracing::error!(error = %err, "effect worker: failed to list pending effects");
            return true;
        }
    };

    for effect in pending.into_iter().take(BATCH_LIMIT) {
        // Claim the effect. A successor that already claimed or settled it
        // makes this write match nothing, and the worker leaves it alone.
        match store
            .update_effect_state_fenced(&effect.id, "in_flight", None, "pending", fence)
            .await
        {
            Ok(true) => {}
            Ok(false) => continue,
            Err(err) if !may_continue(&err) => {
                tracing::error!(
                    effect_id = %effect.id,
                    error = %err,
                    "effect worker: lost the daemon lease; stopping instead of claiming effect work"
                );
                return false;
            }
            Err(err) => {
                tracing::error!(effect_id = %effect.id, error = %err, "effect worker: failed to mark effect in-flight");
                continue;
            }
        }

        let outcome = registry.dispatch(&effect).await;
        let (result_outcome, result_json) = match outcome {
            Ok(EffectOutcome::Success(json)) => ("success", json),
            Ok(EffectOutcome::Failure(message)) => ("failure", message),
            Err(message) => ("failure", message),
        };

        let result_id = format!("{}-worker-result", effect.id);
        // Records the result and settles the effect in one fenced, claim-guarded
        // transaction: if the claim was overtaken while the handler ran, neither
        // the state move nor the result row lands.
        match store
            .settle_effect_from_worker_fenced(
                &result_id,
                &effect.id,
                result_outcome,
                &result_json,
                fence,
            )
            .await
        {
            Ok(_) => {}
            Err(err) if !may_continue(&err) => {
                tracing::error!(
                    effect_id = %effect.id,
                    error = %err,
                    "effect worker: lost the daemon lease; discarding a result it no longer owns"
                );
                return false;
            }
            Err(err) => {
                tracing::error!(effect_id = %effect.id, error = %err, "effect worker: failed to record effect result");
            }
        }
    }

    true
}
