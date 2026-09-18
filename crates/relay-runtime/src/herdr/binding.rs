//! RuntimeBinding storage and tracking for Herdr runtime sessions.
//!
//! Maps Relay domain attempt identity to physical runtime handles,
//! persisting state across restarts and reconciliation.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use crate::backplane::{EffectKey, RuntimeBinding, RuntimeBindingId, RuntimeBindingState};

#[derive(Debug, Clone, Default)]
pub struct RuntimeBindingStore {
    inner: Arc<Mutex<BindingStoreInner>>,
}

#[derive(Debug, Default)]
struct BindingStoreInner {
    bindings: HashMap<RuntimeBindingId, RuntimeBinding>,
    by_attempt: HashMap<String, RuntimeBindingId>,
    by_effect: HashMap<EffectKey, RuntimeBindingId>,
    by_handle: HashMap<String, RuntimeBindingId>,
}

impl RuntimeBindingStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, binding: RuntimeBinding) {
        let mut inner = self.inner.lock().unwrap();
        let id = binding.id.clone();
        inner
            .by_attempt
            .insert(binding.role_attempt_id.clone(), id.clone());
        if let Some(meta) = &binding.metadata {
            inner.by_effect.insert(meta.effect_key.clone(), id.clone());
        }
        inner
            .by_handle
            .insert(binding.runtime_handle.handle_id.clone(), id.clone());
        inner.bindings.insert(id, binding);
    }

    pub fn get(&self, id: &RuntimeBindingId) -> Option<RuntimeBinding> {
        self.inner.lock().unwrap().bindings.get(id).cloned()
    }

    pub fn get_by_attempt(&self, role_attempt_id: &str) -> Option<RuntimeBinding> {
        let inner = self.inner.lock().unwrap();
        let id = inner.by_attempt.get(role_attempt_id)?;
        inner.bindings.get(id).cloned()
    }

    pub fn get_by_effect(&self, effect_key: &EffectKey) -> Option<RuntimeBinding> {
        let inner = self.inner.lock().unwrap();
        let id = inner.by_effect.get(effect_key)?;
        inner.bindings.get(id).cloned()
    }

    pub fn get_by_handle(&self, handle_id: &str) -> Option<RuntimeBinding> {
        let inner = self.inner.lock().unwrap();
        let id = inner.by_handle.get(handle_id)?;
        inner.bindings.get(id).cloned()
    }

    pub fn update_state(&self, id: &RuntimeBindingId, new_state: RuntimeBindingState) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(binding) = inner.bindings.get_mut(id) {
            binding.state = new_state;
        }
    }

    pub fn update_last_reconciled(
        &self,
        id: &RuntimeBindingId,
        timestamp: impl Into<String>,
        new_state: Option<RuntimeBindingState>,
    ) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(binding) = inner.bindings.get_mut(id) {
            binding.last_reconciled_at = timestamp.into();
            if let Some(state) = new_state {
                binding.state = state;
            }
        }
    }

    pub fn list_all(&self) -> Vec<RuntimeBinding> {
        self.inner
            .lock()
            .unwrap()
            .bindings
            .values()
            .cloned()
            .collect()
    }

    pub fn list_active(&self) -> Vec<RuntimeBinding> {
        self.inner
            .lock()
            .unwrap()
            .bindings
            .values()
            .filter(|b| {
                matches!(
                    b.state,
                    RuntimeBindingState::Attached | RuntimeBindingState::Running
                )
            })
            .cloned()
            .collect()
    }

    /// Simulates saving and reloading persistent bindings across a Relay daemon restart.
    pub fn snapshot(&self) -> Vec<RuntimeBinding> {
        self.list_all()
    }

    pub fn restore_from_snapshot(&self, list: Vec<RuntimeBinding>) {
        for b in list {
            self.insert(b);
        }
    }
}
