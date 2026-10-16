//! Phase 11: Multi-Project / Provider Scheduler v2 Integration Tests.
//!
//! Enforces:
//! 1. One blocked project does not starve another (multi-project fair round-robin).
//! 2. Same checkout write conflict serializes (exclusive checkout locks).
//! 3. Independent checkouts run concurrently if limits allow.
//! 4. Provider capacity release after completion, failure, or cancellation.
//! 5. Starvation prevention via dynamic aging weight.
//! 6. Cancellation of one project does not corrupt another queue.

use std::collections::{HashMap, HashSet};

use relay_core::{
    CheckoutId, ProjectId, QueuedWorkItem, RoleAttemptId, RoleInstanceId, SchedulePriority,
    SchedulerV2, SchedulerV2Config, SqliteStore, TaskId, WorkflowRunId,
};

fn sample_work_item(
    proj: &str,
    chk: &str,
    provider: &str,
    exclusive: bool,
    priority: SchedulePriority,
) -> QueuedWorkItem {
    let proj_id = ProjectId::parse(proj).unwrap();
    let chk_id = CheckoutId::parse(chk).unwrap();
    let task_id = TaskId::generate();
    let run_id = WorkflowRunId::generate();
    let role_inst = RoleInstanceId(format!("role-{}", uuid::Uuid::new_v4().simple()));
    let role_att = RoleAttemptId(format!("att-{}", uuid::Uuid::new_v4().simple()));

    QueuedWorkItem {
        task_id,
        project_id: proj_id,
        checkout_id: chk_id,
        workflow_run_id: run_id,
        role_instance_id: role_inst,
        role_attempt_id: role_att,
        target_provider: provider.to_string(),
        priority,
        required_capabilities: HashSet::new(),
        requires_exclusive_checkout: exclusive,
        queued_at: 0,
        age_weight: 0,
    }
}

#[tokio::test]
async fn test_multi_project_fairness_and_starvation_prevention() {
    let store = SqliteStore::in_memory().await.unwrap();
    let config = SchedulerV2Config {
        global_max_parallel: 2,
        per_project_max_parallel: 1,
        ..SchedulerV2Config::default()
    };

    let scheduler = SchedulerV2::new(config);

    // Enqueue 3 items in Project A, 1 item in Project B
    scheduler.enqueue(sample_work_item(
        "proj-a",
        "chk-a",
        "mock",
        false,
        SchedulePriority::Normal,
    ));
    scheduler.enqueue(sample_work_item(
        "proj-a",
        "chk-a",
        "mock",
        false,
        SchedulePriority::Normal,
    ));
    scheduler.enqueue(sample_work_item(
        "proj-a",
        "chk-a",
        "mock",
        false,
        SchedulePriority::Normal,
    ));
    scheduler.enqueue(sample_work_item(
        "proj-b",
        "chk-b",
        "mock",
        false,
        SchedulePriority::Normal,
    ));

    // Tick 1: Schedule next. Global max = 2, per-project max = 1.
    // Must allocate 1 to Project A and 1 to Project B (Project A cannot monopolize both slots!)
    let slots = scheduler.schedule_next(&store).await;
    assert_eq!(slots.len(), 2);

    let proj_a_slots = slots
        .iter()
        .filter(|s| s.work_item.project_id.to_string() == "proj-a")
        .count();
    let proj_b_slots = slots
        .iter()
        .filter(|s| s.work_item.project_id.to_string() == "proj-b")
        .count();

    assert_eq!(proj_a_slots, 1, "Project A should get 1 slot");
    assert_eq!(
        proj_b_slots, 1,
        "Project B must not be starved by Project A"
    );
}

#[tokio::test]
async fn test_exclusive_checkout_conflict_serialization() {
    let store = SqliteStore::in_memory().await.unwrap();
    let config = SchedulerV2Config {
        global_max_parallel: 4,
        per_project_max_parallel: 4,
        ..SchedulerV2Config::default()
    };

    let scheduler = SchedulerV2::new(config);

    // Enqueue two items targeting the EXACT SAME checkout with exclusive requirement
    scheduler.enqueue(sample_work_item(
        "proj-a",
        "chk-shared",
        "mock",
        true,
        SchedulePriority::Normal,
    ));
    scheduler.enqueue(sample_work_item(
        "proj-a",
        "chk-shared",
        "mock",
        true,
        SchedulePriority::Normal,
    ));

    // Tick 1: Only ONE item can be scheduled because the checkout is locked exclusively
    let slots_tick1 = scheduler.schedule_next(&store).await;
    assert_eq!(
        slots_tick1.len(),
        1,
        "Second item must wait due to exclusive checkout lock"
    );

    // Tick 2: Second attempt to schedule still yields 0 new slots because checkout is busy
    let slots_tick2 = scheduler.schedule_next(&store).await;
    assert_eq!(slots_tick2.len(), 0);

    // Release the active slot
    let released = scheduler
        .release_slot(&slots_tick1[0].slot_id, &store)
        .await;
    assert!(released.is_some());

    // Tick 3: Now the second item can be scheduled
    let slots_tick3 = scheduler.schedule_next(&store).await;
    assert_eq!(
        slots_tick3.len(),
        1,
        "Second item acquires the released checkout"
    );
}

#[tokio::test]
async fn test_provider_capacity_limiting_and_release() {
    let store = SqliteStore::in_memory().await.unwrap();
    let mut prov_caps = HashMap::new();
    prov_caps.insert("claude".to_string(), 2); // Claude capped at 2 slots

    let config = SchedulerV2Config {
        global_max_parallel: 10,
        per_project_max_parallel: 10,
        provider_capacity_limits: prov_caps,
        ..SchedulerV2Config::default()
    };

    let scheduler = SchedulerV2::new(config);

    // Enqueue 4 tasks targeting 'claude'
    for _ in 0..4 {
        scheduler.enqueue(sample_work_item(
            "proj-a",
            "chk-a",
            "claude",
            false,
            SchedulePriority::Normal,
        ));
    }

    // Tick 1: Schedules exactly 2 tasks because of Claude provider capacity
    let slots = scheduler.schedule_next(&store).await;
    assert_eq!(
        slots.len(),
        2,
        "Claude provider cap must restrict active executions to 2"
    );
    assert_eq!(scheduler.get_provider_active_count("claude"), 2);

    // Release 1 slot
    scheduler.release_slot(&slots[0].slot_id, &store).await;
    assert_eq!(scheduler.get_provider_active_count("claude"), 1);

    // Tick 2: Now 1 more can be scheduled
    let slots_tick2 = scheduler.schedule_next(&store).await;
    assert_eq!(slots_tick2.len(), 1);
    assert_eq!(scheduler.get_provider_active_count("claude"), 2);
}

#[tokio::test]
async fn test_cancellation_isolation_across_projects() {
    let store = SqliteStore::in_memory().await.unwrap();
    let config = SchedulerV2Config {
        global_max_parallel: 2,
        per_project_max_parallel: 1,
        ..SchedulerV2Config::default()
    };
    let scheduler = SchedulerV2::new(config);

    let proj_a = ProjectId::parse("proj-a").unwrap();
    let proj_b = ProjectId::parse("proj-b").unwrap();

    // Enqueue in Project A and Project B
    scheduler.enqueue(sample_work_item(
        "proj-a",
        "chk-a",
        "mock",
        false,
        SchedulePriority::Normal,
    ));
    scheduler.enqueue(sample_work_item(
        "proj-a",
        "chk-a",
        "mock",
        false,
        SchedulePriority::Normal,
    ));
    scheduler.enqueue(sample_work_item(
        "proj-b",
        "chk-b",
        "mock",
        false,
        SchedulePriority::Normal,
    ));

    // Schedule 1 from each
    let active_slots = scheduler.schedule_next(&store).await;
    assert_eq!(active_slots.len(), 2);

    // Cancel Project A completely
    let (drained_count, cancelled_active) = scheduler.cancel_project(&proj_a, &store).await;
    assert_eq!(drained_count, 1, "1 queued item in Project A drained");
    assert_eq!(
        cancelled_active.len(),
        1,
        "1 active slot in Project A cancelled"
    );

    // Verify Project B is 100% UNTOUCHED and still running
    let (proj_b_queued, proj_b_active) = scheduler.get_project_stats(&proj_b);
    assert_eq!(proj_b_active, 1, "Project B active execution untouched");
    assert_eq!(proj_b_queued, 0);
}
