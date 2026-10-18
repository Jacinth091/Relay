//! Phase 11: Multi-Project / Provider Scheduler v2.
//!
//! Provides:
//! - Multi-project fair scheduling with deterministic aging to prevent starvation.
//! - Global and per-project concurrency slot limits.
//! - Checkout and worktree write serialization via monotonic Lease fencing tokens.
//! - Provider capacity accounting and slot allocation/release.
//! - Cancellation isolation across project queues.
//! - Privileged operation serialization where needed.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
};

use relay_store::SqliteStore;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    CheckoutId, ProjectId, RoleAttemptId, RoleInstanceId, TaskId, WorkflowRunId,
    candidate::{LeaseHolder, LeaseId, LeaseKind, LeaseResource},
    durable_state::{acquire_durable_lease, release_durable_lease},
    policy::Capability,
};

#[derive(Debug, Error)]
pub enum SchedulerV2Error {
    #[error("project queue not found: {0}")]
    ProjectQueueNotFound(String),
    #[error("task not in ready state: {0}")]
    TaskNotReady(String),
    #[error("resource contention: checkout {0} currently locked exclusively")]
    CheckoutLocked(String),
    #[error("provider {provider} at capacity ({active}/{limit})")]
    ProviderCapacityExceeded {
        provider: String,
        active: usize,
        limit: usize,
    },
    #[error("global capacity reached ({0}/{0})")]
    GlobalCapacityReached(usize),
    #[error("project {project} capacity reached ({active}/{limit})")]
    ProjectCapacityReached {
        project: String,
        active: usize,
        limit: usize,
    },
    #[error("invalid limit: {name}")]
    InvalidLimit { name: &'static str },
}

/// Dynamic scheduling priority for tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum SchedulePriority {
    Low = 0,
    #[default]
    Normal = 1,
    High = 2,
    Urgent = 3,
}

/// Work item queued for scheduling in a specific project queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedWorkItem {
    pub task_id: TaskId,
    pub project_id: ProjectId,
    pub checkout_id: CheckoutId,
    pub workflow_run_id: WorkflowRunId,
    pub role_instance_id: RoleInstanceId,
    pub role_attempt_id: RoleAttemptId,
    pub target_provider: String,
    pub priority: SchedulePriority,
    pub required_capabilities: HashSet<Capability>,
    pub requires_exclusive_checkout: bool,
    pub queued_at: u64,
    pub age_weight: u64,
}

/// Active execution slot allocation leased to a running attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveExecutionSlot {
    pub slot_id: String,
    pub work_item: QueuedWorkItem,
    pub lease_id: Option<LeaseId>,
    pub allocated_at: u64,
}

/// Limits and fairness configuration for the Multi-Project Scheduler v2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerV2Config {
    pub global_max_parallel: usize,
    pub per_project_max_parallel: usize,
    pub provider_capacity_limits: HashMap<String, usize>,
    pub aging_increment_per_tick: u64,
    pub max_aging_boost: u64,
}

impl Default for SchedulerV2Config {
    fn default() -> Self {
        let mut provider_limits = HashMap::new();
        provider_limits.insert("codex".to_string(), 4);
        provider_limits.insert("claude".to_string(), 4);
        provider_limits.insert("mock".to_string(), 10);
        provider_limits.insert("default".to_string(), 4);

        Self {
            global_max_parallel: 8,
            per_project_max_parallel: 2,
            provider_capacity_limits: provider_limits,
            aging_increment_per_tick: 5,
            max_aging_boost: 100,
        }
    }
}

/// State of an individual project's scheduling queue.
#[derive(Debug, Default)]
struct ProjectQueueState {
    queue: VecDeque<QueuedWorkItem>,
    active_count: usize,
}

/// Provider capacity counter.
#[derive(Debug, Default)]
struct ProviderCapacityTracker {
    active_count: usize,
}

/// The Relay Multi-Project / Provider Scheduler v2 engine.
pub struct SchedulerV2 {
    config: SchedulerV2Config,
    project_queues: Arc<Mutex<HashMap<ProjectId, ProjectQueueState>>>,
    provider_trackers: Arc<Mutex<HashMap<String, ProviderCapacityTracker>>>,
    active_slots: Arc<Mutex<HashMap<String, ActiveExecutionSlot>>>,
    current_tick: Arc<Mutex<u64>>,
}

impl SchedulerV2 {
    pub fn new(config: SchedulerV2Config) -> Self {
        Self {
            config,
            project_queues: Arc::new(Mutex::new(HashMap::new())),
            provider_trackers: Arc::new(Mutex::new(HashMap::new())),
            active_slots: Arc::new(Mutex::new(HashMap::new())),
            current_tick: Arc::new(Mutex::new(0)),
        }
    }

    /// Submits a work item into its project's scheduling queue.
    pub fn enqueue(&self, item: QueuedWorkItem) {
        let mut queues = self.project_queues.lock().unwrap();
        let project_state = queues.entry(item.project_id.clone()).or_default();
        project_state.queue.push_back(item);
    }

    /// Removes a queued (not yet admitted) work item, e.g. when its attempt is
    /// abandoned, so it can never be admitted later for a dead attempt.
    /// Returns whether anything was removed.
    pub fn withdraw_queued(&self, role_attempt_id: &RoleAttemptId) -> bool {
        let mut queues = self.project_queues.lock().unwrap();
        let mut removed = false;
        for state in queues.values_mut() {
            let before = state.queue.len();
            state
                .queue
                .retain(|item| &item.role_attempt_id != role_attempt_id);
            removed |= state.queue.len() != before;
        }
        removed
    }

    /// Advances the deterministic scheduler tick and schedules eligible tasks.
    /// Uses an aging algorithm so older tasks in busy projects don't starve.
    pub async fn schedule_next(&self, store: &SqliteStore) -> Vec<ActiveExecutionSlot> {
        let current_tick_val = {
            let mut tick = self.current_tick.lock().unwrap();
            *tick += 1;
            *tick
        };

        let mut newly_allocated = Vec::new();

        // Try scheduling across projects with fair round-robin + aging prioritization
        'scheduling_loop: loop {
            let (active_len, mut project_keys) = {
                let active = self.active_slots.lock().unwrap();
                if active.len() >= self.config.global_max_parallel {
                    break 'scheduling_loop;
                }
                let queues = self.project_queues.lock().unwrap();
                let keys: Vec<ProjectId> = queues.keys().cloned().collect();
                (active.len(), keys)
            };

            if active_len >= self.config.global_max_parallel {
                break 'scheduling_loop;
            }

            project_keys.sort();

            // Age queued items and collect candidates
            let mut candidates: Vec<(ProjectId, usize, u64)> = Vec::new();
            {
                let mut queues = self.project_queues.lock().unwrap();
                let providers = self.provider_trackers.lock().unwrap();

                for proj_id in &project_keys {
                    if let Some(state) = queues.get_mut(proj_id) {
                        for item in state.queue.iter_mut() {
                            item.age_weight = (item.age_weight
                                + self.config.aging_increment_per_tick)
                                .min(self.config.max_aging_boost);
                        }

                        // Check per-project capacity limit
                        if state.active_count >= self.config.per_project_max_parallel {
                            continue;
                        }

                        for (idx, item) in state.queue.iter().enumerate() {
                            let prov_limit = self
                                .config
                                .provider_capacity_limits
                                .get(&item.target_provider)
                                .cloned()
                                .unwrap_or(4);
                            let prov_active = providers
                                .get(&item.target_provider)
                                .map(|p| p.active_count)
                                .unwrap_or(0);
                            if prov_active >= prov_limit {
                                continue;
                            }

                            let effective_score = (item.priority as u64 * 100) + item.age_weight;
                            candidates.push((proj_id.clone(), idx, effective_score));
                        }
                    }
                }
            }

            // Sort candidates by effective_score descending
            candidates.sort_by_key(|b| std::cmp::Reverse(b.2));

            let mut scheduled_any = false;

            for (proj_id, idx, _) in candidates {
                let item = {
                    let queues = self.project_queues.lock().unwrap();
                    let state = match queues.get(&proj_id) {
                        Some(s) => s,
                        None => continue,
                    };
                    if state.active_count >= self.config.per_project_max_parallel {
                        continue;
                    }
                    match state.queue.get(idx) {
                        Some(it) => it.clone(),
                        None => continue,
                    }
                };

                let mut allocated_lease_id = None;
                // Each work item is its own lease holder: the store's "same
                // holder renewing" path would otherwise treat two different
                // items competing for the same checkout as one holder
                // extending its own lease, silently defeating exclusivity.
                let holder_id = item.task_id.to_string();
                let holder = LeaseHolder {
                    holder_id: holder_id.clone(),
                    pid: std::process::id(),
                    host: "localhost".to_string(),
                };

                if item.requires_exclusive_checkout {
                    match acquire_durable_lease(
                        store,
                        &LeaseId::generate(),
                        &LeaseResource::Checkout(item.checkout_id.clone()),
                        &LeaseKind::ExclusiveWrite,
                        &holder,
                        300,
                    )
                    .await
                    {
                        Ok(lease) => {
                            allocated_lease_id = Some(lease.id);
                        }
                        Err(_) => {
                            // Resource contention or locked by another active lease, skip this candidate
                            continue;
                        }
                    }
                }

                // If acquired or not required, remove from queue and allocate slot
                let removed_item = {
                    let mut queues = self.project_queues.lock().unwrap();
                    let mut providers = self.provider_trackers.lock().unwrap();
                    let mut active_slots = self.active_slots.lock().unwrap();

                    if active_slots.len() >= self.config.global_max_parallel {
                        None
                    } else if let Some(state) = queues.get_mut(&proj_id) {
                        if state.active_count >= self.config.per_project_max_parallel {
                            None
                        } else {
                            let item_pos =
                                state.queue.iter().position(|it| it.task_id == item.task_id);
                            if let Some(pos) = item_pos {
                                let item = state.queue.remove(pos).unwrap();
                                state.active_count += 1;

                                let prov_tracker =
                                    providers.entry(item.target_provider.clone()).or_default();
                                prov_tracker.active_count += 1;

                                let slot_id = format!(
                                    "slot-{}-{}",
                                    item.task_id,
                                    uuid::Uuid::new_v4().simple()
                                );
                                let slot = ActiveExecutionSlot {
                                    slot_id: slot_id.clone(),
                                    work_item: item.clone(),
                                    lease_id: allocated_lease_id.clone(),
                                    allocated_at: current_tick_val,
                                };
                                active_slots.insert(slot_id, slot.clone());
                                Some(slot)
                            } else {
                                None
                            }
                        }
                    } else {
                        None
                    }
                };

                match removed_item {
                    Some(slot) => {
                        newly_allocated.push(slot);
                        scheduled_any = true;
                        break;
                    }
                    None => {
                        if let Some(ref lid) = allocated_lease_id {
                            let _ = release_durable_lease(store, lid, &holder_id).await;
                        }
                        continue;
                    }
                }
            }

            if !scheduled_any {
                // No more candidates can be scheduled in this cycle
                break 'scheduling_loop;
            }
        }

        newly_allocated
    }

    /// Releases an active execution slot upon completion, failure, or cancellation.
    /// Releases durable leases and frees provider/project capacity immediately.
    pub async fn release_slot(
        &self,
        slot_id: &str,
        store: &SqliteStore,
    ) -> Option<ActiveExecutionSlot> {
        let slot = {
            let mut active_slots = self.active_slots.lock().unwrap();
            active_slots.remove(slot_id)?
        };

        // 1. Release project capacity
        {
            let mut queues = self.project_queues.lock().unwrap();
            if let Some(state) = queues.get_mut(&slot.work_item.project_id) {
                state.active_count = state.active_count.saturating_sub(1);
            }
        }

        // 2. Release provider capacity
        {
            let mut providers = self.provider_trackers.lock().unwrap();
            if let Some(tracker) = providers.get_mut(&slot.work_item.target_provider) {
                tracker.active_count = tracker.active_count.saturating_sub(1);
            }
        }

        // 3. Release durable lease if present. Must match the holder id used
        // to acquire it in schedule_next (the work item's task id).
        if let Some(ref lease_id) = slot.lease_id {
            let holder_id = slot.work_item.task_id.to_string();
            let _ = release_durable_lease(store, lease_id, &holder_id).await;
        }

        Some(slot)
    }

    /// Cancels all queued work items and active slots for a specific project.
    /// Guarantees that cancellation of one project does NOT affect or corrupt any other project queue.
    pub async fn cancel_project(
        &self,
        project_id: &ProjectId,
        store: &SqliteStore,
    ) -> (usize, Vec<ActiveExecutionSlot>) {
        // 1. Drain queued items in this project
        let drained_queued_count = {
            let mut queues = self.project_queues.lock().unwrap();
            if let Some(state) = queues.get_mut(project_id) {
                let count = state.queue.len();
                state.queue.clear();
                count
            } else {
                0
            }
        };

        // 2. Find all active slots belonging to this project
        let slots_to_release = {
            let active = self.active_slots.lock().unwrap();
            let mut to_release = Vec::new();
            for (slot_id, slot) in active.iter() {
                if &slot.work_item.project_id == project_id {
                    to_release.push(slot_id.clone());
                }
            }
            to_release
        };

        let mut released_slots = Vec::new();
        for slot_id in slots_to_release {
            if let Some(slot) = self.release_slot(&slot_id, store).await {
                released_slots.push(slot);
            }
        }

        (drained_queued_count, released_slots)
    }

    /// Returns queue depth and active count for a project.
    pub fn get_project_stats(&self, project_id: &ProjectId) -> (usize, usize) {
        let queues = self.project_queues.lock().unwrap();
        queues
            .get(project_id)
            .map(|s| (s.queue.len(), s.active_count))
            .unwrap_or((0, 0))
    }

    /// Returns active count for a specific provider.
    pub fn get_provider_active_count(&self, provider: &str) -> usize {
        let providers = self.provider_trackers.lock().unwrap();
        providers.get(provider).map(|p| p.active_count).unwrap_or(0)
    }

    /// Returns total number of active slots globally.
    pub fn get_global_active_count(&self) -> usize {
        self.active_slots.lock().unwrap().len()
    }
}
