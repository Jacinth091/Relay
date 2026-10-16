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

use relay_store::{PendingEffectRecord, SqliteStore};

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
pub async fn run_effect_worker(
    store: SqliteStore,
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
                reconcile_stale_in_flight(&store, stale_after, max_attempts).await;
                process_pending_batch(&store, &registry).await;
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

async fn reconcile_stale_in_flight(store: &SqliteStore, stale_after: Duration, max_attempts: i64) {
    let stale = match store
        .list_stale_in_flight_effects(stale_after.as_secs() as i64)
        .await
    {
        Ok(effects) => effects,
        Err(err) => {
            tracing::error!(error = %err, "effect worker: failed to list stale in-flight effects");
            return;
        }
    };

    for effect in stale {
        let outcome = if effect.attempts >= max_attempts {
            store
                .update_effect_state(
                    &effect.id,
                    "failed",
                    Some("exceeded max retry attempts after crash recovery"),
                )
                .await
        } else {
            store.update_effect_state(&effect.id, "pending", None).await
        };

        if let Err(err) = outcome {
            tracing::error!(effect_id = %effect.id, error = %err, "effect worker: failed to reconcile stale in-flight effect");
        }
    }
}

async fn process_pending_batch(store: &SqliteStore, registry: &EffectWorkerRegistry) {
    const BATCH_LIMIT: usize = 20;

    let pending = match store.list_pending_effects(Some("pending")).await {
        Ok(effects) => effects,
        Err(err) => {
            tracing::error!(error = %err, "effect worker: failed to list pending effects");
            return;
        }
    };

    for effect in pending.into_iter().take(BATCH_LIMIT) {
        if let Err(err) = store
            .update_effect_state(&effect.id, "in_flight", None)
            .await
        {
            tracing::error!(effect_id = %effect.id, error = %err, "effect worker: failed to mark effect in-flight");
            continue;
        }

        let outcome = registry.dispatch(&effect).await;
        let (result_outcome, result_json) = match outcome {
            Ok(EffectOutcome::Success(json)) => ("success", json),
            Ok(EffectOutcome::Failure(message)) => ("failure", message),
            Err(message) => ("failure", message),
        };

        let result_id = format!("{}-worker-result", effect.id);
        if let Err(err) = store
            .record_effect_result(&result_id, &effect.id, result_outcome, &result_json)
            .await
        {
            tracing::error!(effect_id = %effect.id, error = %err, "effect worker: failed to record effect result");
        }
    }
}
