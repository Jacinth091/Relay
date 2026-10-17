//! Phase 10: Delegation, Consultation, Budgets, Deadlock Prevention.
//!
//! Allows dynamic orchestration (a role requesting a child role instance)
//! without uncontrolled recursive spawning. Flow:
//!
//! ```text
//! provider requests role
//!   -> Relay validates request
//!   -> checks ApprovedIntent/ExecutionGrant (allowed_delegation_classes)
//!   -> checks role-definition-level delegation policy
//!   -> checks budget
//!   -> checks dependency cycle
//!   -> creates RoleInstance identity
//!   -> (a later phase resolves provider + spawns the RoleAttempt)
//! ```
//!
//! A provider can only ever *name a pre-registered `RoleDefinitionId`* --
//! never an executable or path. Launch construction remains entirely owned
//! by the Phase 08 `ProviderAdapter`/`ProviderResolver` seam, which this
//! module never touches.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ArtifactId, ExecutionBudget, ExecutionGrant, RoleAttemptId, RoleDefinition, RoleDefinitionId,
    RoleInstanceId, RoleWriteMode,
};

// ---------------------------------------------------------------------------
// Delegation & Consultation requests
// ---------------------------------------------------------------------------

/// Blocking vs non-blocking vs read-only delegation semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationMode {
    /// Parent waits for the child's result before proceeding.
    BlockingChild,
    /// Parent continues; the child's result is consumed asynchronously.
    NonblockingChild,
    /// Read-only advisory role. Never confers write authority, regardless
    /// of the target role definition's own configured write mode.
    Consultation,
}

impl DelegationMode {
    pub fn is_blocking(self) -> bool {
        matches!(self, Self::BlockingChild)
    }

    pub fn is_consultation(self) -> bool {
        matches!(self, Self::Consultation)
    }
}

/// Independence requested between a child attempt and its producer/parent context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndependenceRequirement {
    NoIndependenceRequirement,
    FreshSessionRequired,
    DifferentProviderPreferred,
    DifferentProviderRequired,
    ProducerContextExcluded,
}

/// A role requesting a child role instance.
///
/// Deliberately narrow: the only thing a caller can *name* is a
/// pre-registered `RoleDefinitionId`. There is no executable/path field --
/// `purpose` is free-form descriptive text that never influences launch
/// construction (that remains the Phase 08 resolver's job).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DelegationRequest {
    pub parent_role_instance_id: RoleInstanceId,
    pub parent_attempt_id: RoleAttemptId,
    pub requested_role: RoleDefinitionId,
    pub mode: DelegationMode,
    pub purpose: String,
    pub input_artifacts: Vec<ArtifactId>,
    pub requested_independence: IndependenceRequirement,
}

/// A read-only advisor/consultant role request. Always lowers to
/// `DelegationMode::Consultation`, which `DelegationManager::delegate` forces
/// to `RoleWriteMode::ReadOnly` regardless of the consulted role's own
/// declared write mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsultationRequest {
    pub parent_role_instance_id: RoleInstanceId,
    pub parent_attempt_id: RoleAttemptId,
    pub consultant_role: RoleDefinitionId,
    pub purpose: String,
    pub input_artifacts: Vec<ArtifactId>,
}

impl ConsultationRequest {
    pub fn into_delegation_request(self) -> DelegationRequest {
        DelegationRequest {
            parent_role_instance_id: self.parent_role_instance_id,
            parent_attempt_id: self.parent_attempt_id,
            requested_role: self.consultant_role,
            mode: DelegationMode::Consultation,
            purpose: self.purpose,
            input_artifacts: self.input_artifacts,
            requested_independence: IndependenceRequirement::NoIndependenceRequirement,
        }
    }
}

/// Outcome of a validated delegation: the identity and effective authority
/// the newly authorized child role instance may be constructed with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationOutcome {
    pub role_instance_id: RoleInstanceId,
    pub role_definition_id: RoleDefinitionId,
    pub parent_role_instance_id: RoleInstanceId,
    pub mode: DelegationMode,
    pub depth: u32,
    pub effective_write_mode: RoleWriteMode,
}

// ---------------------------------------------------------------------------
// Role dependency graph
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleDependencyState {
    Active,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDependencyNode {
    pub role_instance_id: RoleInstanceId,
    pub role_definition_id: RoleDefinitionId,
    /// `None` for the workflow's root role instance.
    pub parent: Option<RoleInstanceId>,
    /// `None` for the root; the delegation mode that created this node otherwise.
    pub mode: Option<DelegationMode>,
    pub depth: u32,
    pub state: RoleDependencyState,
}

/// DAG tracking parent-child role instance relationships, depth, and
/// execution state. Instance identity is always fresh (retrying an attempt
/// never creates a new node), so this graph is structurally a tree; deadlock
/// risk is instead detected at the role-*definition* level (see
/// `would_create_blocking_cycle`).
#[derive(Debug, Default)]
pub struct RoleDependencyGraph {
    nodes: HashMap<RoleInstanceId, RoleDependencyNode>,
    children: HashMap<RoleInstanceId, Vec<RoleInstanceId>>,
}

impl RoleDependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_root(
        &mut self,
        role_instance_id: RoleInstanceId,
        role_definition_id: RoleDefinitionId,
    ) {
        self.nodes.insert(
            role_instance_id.clone(),
            RoleDependencyNode {
                role_instance_id,
                role_definition_id,
                parent: None,
                mode: None,
                depth: 0,
                state: RoleDependencyState::Active,
            },
        );
    }

    pub fn get(&self, id: &RoleInstanceId) -> Option<&RoleDependencyNode> {
        self.nodes.get(id)
    }

    pub fn depth_of(&self, id: &RoleInstanceId) -> Option<u32> {
        self.nodes.get(id).map(|n| n.depth)
    }

    /// True if a BLOCKING delegation from `parent` to `child_role` would
    /// create an unbounded blocked-wait chain: walking up the parent's
    /// unbroken chain of BLOCKING ancestors, the same role definition
    /// already appears. That ancestor is (transitively) waiting on this new
    /// child, which would in turn wait on a fresh instance of a role that is
    /// itself blocked further up the same chain -- a deadlock shape, rejected
    /// before any process is ever launched.
    pub fn would_create_blocking_cycle(
        &self,
        parent: &RoleInstanceId,
        child_role: &RoleDefinitionId,
    ) -> bool {
        let mut cursor = Some(parent.clone());
        while let Some(id) = cursor {
            let Some(node) = self.nodes.get(&id) else {
                break;
            };
            if &node.role_definition_id == child_role {
                return true;
            }
            cursor = match node.mode {
                Some(DelegationMode::BlockingChild) => node.parent.clone(),
                // A non-blocking ancestor, a consultation, or the root
                // breaks the blocking chain -- no deadlock risk beyond it.
                None | Some(_) => None,
            };
        }
        false
    }

    /// Inserts a new child node. Returns its depth, or `None` if `parent`
    /// is not present in the graph.
    pub fn insert_child(
        &mut self,
        parent: &RoleInstanceId,
        child_instance_id: RoleInstanceId,
        child_role_definition_id: RoleDefinitionId,
        mode: DelegationMode,
    ) -> Option<u32> {
        let parent_depth = self.nodes.get(parent)?.depth;
        let depth = parent_depth + 1;
        self.nodes.insert(
            child_instance_id.clone(),
            RoleDependencyNode {
                role_instance_id: child_instance_id.clone(),
                role_definition_id: child_role_definition_id,
                parent: Some(parent.clone()),
                mode: Some(mode),
                depth,
                state: RoleDependencyState::Active,
            },
        );
        self.children
            .entry(parent.clone())
            .or_default()
            .push(child_instance_id);
        Some(depth)
    }

    pub fn mark_state(&mut self, id: &RoleInstanceId, state: RoleDependencyState) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.state = state;
        }
    }

    /// All instance ids in the subtree rooted at `id`, including `id` itself.
    pub fn subtree(&self, id: &RoleInstanceId) -> Vec<RoleInstanceId> {
        let mut out = Vec::new();
        let mut stack = vec![id.clone()];
        while let Some(current) = stack.pop() {
            if let Some(kids) = self.children.get(&current) {
                stack.extend(kids.iter().cloned());
            }
            out.push(current);
        }
        out
    }

    /// Marks `id` and every still-Active descendant Cancelled. Already
    /// terminal nodes (Completed/Failed) are left untouched -- cancellation
    /// never overwrites a truthful terminal outcome. Returns the ids that
    /// actually transitioned.
    pub fn cancel_subtree(&mut self, id: &RoleInstanceId) -> Vec<RoleInstanceId> {
        let mut cancelled = Vec::new();
        for instance_id in self.subtree(id) {
            if let Some(node) = self.nodes.get_mut(&instance_id) {
                if node.state == RoleDependencyState::Active {
                    node.state = RoleDependencyState::Cancelled;
                    cancelled.push(instance_id);
                }
            }
        }
        cancelled
    }

    pub fn active_count(&self) -> usize {
        self.nodes
            .values()
            .filter(|n| n.state == RoleDependencyState::Active)
            .count()
    }
}

// ---------------------------------------------------------------------------
// Budget accounting
// ---------------------------------------------------------------------------

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BudgetError {
    #[error("role activation budget exhausted (max {max})")]
    RoleActivationsExhausted { max: u32 },
    #[error("delegation budget exhausted (max {max})")]
    DelegationsExhausted { max: u32 },
    #[error("delegation depth {requested} exceeds max {max}")]
    DelegationDepthExceeded { requested: u32, max: u32 },
    #[error("concurrent attempt budget exhausted (max {max})")]
    ConcurrentAttemptsExhausted { max: u32 },
}

/// Tracks and deducts a `WorkflowRun`'s `ExecutionBudget` as role instances
/// are activated and delegated. Wall-clock (`max_wall_clock_secs`) remains a
/// declarative ceiling enforced by the scheduler (later phase); this module
/// deliberately does not fabricate time/telemetry-based enforcement here.
#[derive(Debug, Clone)]
pub struct BudgetAccounting {
    budget: ExecutionBudget,
    role_activations_used: u32,
    delegations_used: u32,
    concurrent_attempts_active: u32,
}

impl BudgetAccounting {
    pub fn new(budget: ExecutionBudget) -> Self {
        Self {
            budget,
            role_activations_used: 0,
            delegations_used: 0,
            concurrent_attempts_active: 0,
        }
    }

    pub fn budget(&self) -> &ExecutionBudget {
        &self.budget
    }

    pub fn role_activations_used(&self) -> u32 {
        self.role_activations_used
    }

    pub fn delegations_used(&self) -> u32 {
        self.delegations_used
    }

    pub fn concurrent_attempts_active(&self) -> u32 {
        self.concurrent_attempts_active
    }

    /// Accounts for activating the workflow's root role instance (not a delegation).
    pub fn try_activate_role(&mut self) -> Result<(), BudgetError> {
        if self.role_activations_used >= self.budget.max_role_activations {
            return Err(BudgetError::RoleActivationsExhausted {
                max: self.budget.max_role_activations,
            });
        }
        self.role_activations_used += 1;
        Ok(())
    }

    /// Accounts for one delegated child (blocking, non-blocking, or
    /// consultation) at the given depth. Checks depth, delegation count, and
    /// role activation count together and commits all three atomically --
    /// a rejected request never partially consumes budget.
    pub fn try_delegate_and_activate(&mut self, depth: u32) -> Result<(), BudgetError> {
        if depth > self.budget.max_delegation_depth {
            return Err(BudgetError::DelegationDepthExceeded {
                requested: depth,
                max: self.budget.max_delegation_depth,
            });
        }
        if self.delegations_used >= self.budget.max_delegations {
            return Err(BudgetError::DelegationsExhausted {
                max: self.budget.max_delegations,
            });
        }
        if self.role_activations_used >= self.budget.max_role_activations {
            return Err(BudgetError::RoleActivationsExhausted {
                max: self.budget.max_role_activations,
            });
        }
        self.delegations_used += 1;
        self.role_activations_used += 1;
        Ok(())
    }

    /// Accounts for a `RoleAttempt` starting to run (concurrency slot).
    pub fn begin_attempt(&mut self) -> Result<(), BudgetError> {
        if self.concurrent_attempts_active >= self.budget.max_concurrent_attempts {
            return Err(BudgetError::ConcurrentAttemptsExhausted {
                max: self.budget.max_concurrent_attempts,
            });
        }
        self.concurrent_attempts_active += 1;
        Ok(())
    }

    /// Releases a concurrency slot (attempt completed, failed, or cancelled).
    pub fn release_attempt(&mut self) {
        self.concurrent_attempts_active = self.concurrent_attempts_active.saturating_sub(1);
    }
}

// ---------------------------------------------------------------------------
// Delegation manager
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum DelegationError {
    #[error("parent role instance '{0}' not found in dependency graph")]
    ParentNotFound(RoleInstanceId),
    #[error("execution grant is not active")]
    GrantInactive,
    #[error("role '{0}' is not an approved delegation target for this execution grant")]
    RoleNotAllowedByGrant(RoleDefinitionId),
    #[error("role '{0}' is not declared as an allowed delegation of parent role '{1}'")]
    RoleNotAllowedByParent(RoleDefinitionId, RoleDefinitionId),
    #[error("unknown role definition '{0}'")]
    UnknownRoleDefinition(RoleDefinitionId),
    #[error("cyclic blocking delegation to role '{0}' rejected")]
    CyclicBlockingDelegation(RoleDefinitionId),
    #[error("execution budget exhausted: {0}")]
    BudgetExhausted(#[from] BudgetError),
}

/// Entirely Relay-mediated delegation authority: validates every child role
/// request against the `ExecutionGrant`, role-definition delegation policy,
/// the dependency graph (cycle/deadlock check), and the execution budget
/// before a child role instance identity is ever minted. It never grants
/// write authority beyond what `RoleDefinition::write_mode` and
/// `DelegationMode` allow, and it never touches `ApprovedIntent`.
pub struct DelegationManager {
    execution_grant: ExecutionGrant,
    role_definitions: HashMap<RoleDefinitionId, RoleDefinition>,
    graph: RoleDependencyGraph,
    budget: BudgetAccounting,
}

impl DelegationManager {
    pub fn new(execution_grant: ExecutionGrant, role_definitions: Vec<RoleDefinition>) -> Self {
        let budget = BudgetAccounting::new(execution_grant.execution_budget.clone());
        let role_definitions = role_definitions
            .into_iter()
            .map(|role| (role.id.clone(), role))
            .collect();
        Self {
            execution_grant,
            role_definitions,
            graph: RoleDependencyGraph::new(),
            budget,
        }
    }

    pub fn graph(&self) -> &RoleDependencyGraph {
        &self.graph
    }

    pub fn budget(&self) -> &BudgetAccounting {
        &self.budget
    }

    fn role_definition(&self, id: &RoleDefinitionId) -> Result<&RoleDefinition, DelegationError> {
        self.role_definitions
            .get(id)
            .ok_or_else(|| DelegationError::UnknownRoleDefinition(id.clone()))
    }

    /// Registers the workflow's top-level role instance, created directly
    /// from the `WorkflowDefinition` rather than via delegation.
    pub fn register_root(
        &mut self,
        role_instance_id: RoleInstanceId,
        role_definition_id: RoleDefinitionId,
    ) -> Result<(), DelegationError> {
        self.budget.try_activate_role()?;
        self.graph.insert_root(role_instance_id, role_definition_id);
        Ok(())
    }

    /// Validates and executes a delegation request.
    pub fn delegate(
        &mut self,
        req: DelegationRequest,
    ) -> Result<DelegationOutcome, DelegationError> {
        if !self.execution_grant.is_active() {
            return Err(DelegationError::GrantInactive);
        }

        let parent = self
            .graph
            .get(&req.parent_role_instance_id)
            .cloned()
            .ok_or_else(|| DelegationError::ParentNotFound(req.parent_role_instance_id.clone()))?;

        // Invariant: a child cannot broaden approved intent -- the target
        // role must already be within the set the human approved when the
        // ExecutionGrant was created.
        if !self
            .execution_grant
            .allowed_delegation_classes
            .contains(&req.requested_role)
        {
            return Err(DelegationError::RoleNotAllowedByGrant(req.requested_role));
        }

        if !self
            .role_definition(&parent.role_definition_id)?
            .allowed_delegations
            .contains(&req.requested_role)
        {
            return Err(DelegationError::RoleNotAllowedByParent(
                req.requested_role,
                parent.role_definition_id.clone(),
            ));
        }

        let target_write_mode = self.role_definition(&req.requested_role)?.write_mode;

        if req.mode.is_blocking()
            && self
                .graph
                .would_create_blocking_cycle(&req.parent_role_instance_id, &req.requested_role)
        {
            return Err(DelegationError::CyclicBlockingDelegation(
                req.requested_role,
            ));
        }

        let depth = parent.depth + 1;
        self.budget.try_delegate_and_activate(depth)?;

        let child_instance_id = RoleInstanceId::generate(&req.requested_role);
        self.graph.insert_child(
            &req.parent_role_instance_id,
            child_instance_id.clone(),
            req.requested_role.clone(),
            req.mode,
        );

        // Consultation is read-only advisory input; it can NEVER confer
        // write authority, regardless of the consulted role's own declared
        // write mode.
        let effective_write_mode = if req.mode.is_consultation() {
            RoleWriteMode::ReadOnly
        } else {
            target_write_mode
        };

        Ok(DelegationOutcome {
            role_instance_id: child_instance_id,
            role_definition_id: req.requested_role,
            parent_role_instance_id: req.parent_role_instance_id,
            mode: req.mode,
            depth,
            effective_write_mode,
        })
    }

    /// Convenience wrapper: lowers a `ConsultationRequest` to `delegate`.
    pub fn consult(
        &mut self,
        req: ConsultationRequest,
    ) -> Result<DelegationOutcome, DelegationError> {
        self.delegate(req.into_delegation_request())
    }

    pub fn complete(&mut self, id: &RoleInstanceId) {
        self.graph.mark_state(id, RoleDependencyState::Completed);
    }

    pub fn fail(&mut self, id: &RoleInstanceId) {
        self.graph.mark_state(id, RoleDependencyState::Failed);
    }

    /// Cancels a role instance and propagates cancellation through every
    /// outstanding (still-Active) descendant. Returns the ids that actually
    /// transitioned to Cancelled.
    pub fn cancel(&mut self, id: &RoleInstanceId) -> Vec<RoleInstanceId> {
        self.graph.cancel_subtree(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApprovedIntent, ConfigSnapshotId, ExecutionGrantId, TaskId, WorkflowDefinitionId};

    fn grant_with(
        allowed: Vec<RoleDefinitionId>,
        budget: ExecutionBudget,
    ) -> (ExecutionGrant, ApprovedIntent) {
        let intent = ApprovedIntent::new(
            TaskId::generate(),
            1,
            "Ship the feature".to_owned(),
            vec![],
            vec![],
            vec![],
            "operator".to_owned(),
        );
        let grant = ExecutionGrant {
            id: ExecutionGrantId::generate(),
            task_id: intent.task_id,
            approved_intent_id: intent.id.clone(),
            approved_intent_generation: intent.generation,
            workflow_definition_id: WorkflowDefinitionId::new("default"),
            config_snapshot_id: ConfigSnapshotId::generate(),
            execution_budget: budget,
            allowed_delegation_classes: allowed,
            granted_by: "operator".to_owned(),
            granted_at: "0".to_owned(),
            revoked_at: None,
        };
        (grant, intent)
    }

    fn role(id: &str, write_mode: RoleWriteMode, allowed_delegations: Vec<&str>) -> RoleDefinition {
        RoleDefinition {
            id: RoleDefinitionId::new(id),
            purpose: id.to_owned(),
            write_mode,
            allowed_delegations: allowed_delegations
                .into_iter()
                .map(RoleDefinitionId::new)
                .collect(),
        }
    }

    #[test]
    fn depth_and_cycle_tracking_on_simple_chain() {
        let (grant, _intent) = grant_with(
            vec![RoleDefinitionId::new("reviewer")],
            ExecutionBudget::default(),
        );
        let implementer = role(
            "implementer",
            RoleWriteMode::CandidateProducer,
            vec!["reviewer"],
        );
        let reviewer = role("reviewer", RoleWriteMode::ReadOnly, vec![]);
        let mut mgr = DelegationManager::new(grant, vec![implementer, reviewer]);

        let root_id = RoleInstanceId::generate(&RoleDefinitionId::new("implementer"));
        mgr.register_root(root_id.clone(), RoleDefinitionId::new("implementer"))
            .unwrap();

        let outcome = mgr
            .delegate(DelegationRequest {
                parent_role_instance_id: root_id.clone(),
                parent_attempt_id: RoleAttemptId::generate(&root_id, 1),
                requested_role: RoleDefinitionId::new("reviewer"),
                mode: DelegationMode::BlockingChild,
                purpose: "review the diff".to_owned(),
                input_artifacts: vec![],
                requested_independence: IndependenceRequirement::NoIndependenceRequirement,
            })
            .unwrap();

        assert_eq!(outcome.depth, 1);
        assert_eq!(mgr.graph().depth_of(&outcome.role_instance_id), Some(1));
        assert_ne!(outcome.role_instance_id, root_id);
    }
}
