use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use relay_core::{
    CoreExecutionEvent, ExecutionLimits, ExecutionReport, ExecutionScheduler,
    ExecutionSchedulerError, GitHubRepositoryMetadata, ProjectKey, RegisterProject, Relay,
    RepositoryInspection, SchedulerSnapshot, SchedulerTaskStatus, SqliteStore, StoredTask, TaskId,
    TaskImportCandidate, TaskState, TrustedProject, inspect_trusted_project_repository,
    list_stored_tasks,
};
#[cfg(not(test))]
use relay_runtime::RuntimeIntegration;
use relay_runtime::{
    CancellationToken, Runtime, RuntimeActivity, RuntimeCapabilities, RuntimeIdentity,
    RuntimeOutputStream, RuntimePermissionRequest, RuntimeRequest, RuntimeRouter,
    RuntimeStartFuture,
};
use relay_runtime_antigravity::{AntigravityRuntime, AntigravityRuntimeHealth};
use relay_runtime_codex::{CodexRuntime, CodexRuntimeHealth};
use relay_runtime_fake::FakeRuntime;
use relay_store::ExecutionRecord;
use tokio::process::Command as TokioCommand;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::command_completion;
use crate::events;
use crate::markdown;
use crate::project_discovery::{DiscoveredProject, discover_project};
use crate::runtime_settings;
use crate::util::{app_execution_limits, repository_preflight_status};

async fn probe_version(executable: &str, args: &[&str]) -> Result<String, String> {
    let output = timeout(Duration::from_secs(3), {
        let mut command = TokioCommand::new(executable);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        command.args(args).kill_on_drop(true).output()
    })
    .await
    .map_err(|_| format!("{executable} probe timed out"))?
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("{executable} exited with {}", output.status));
    }
    let text = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    Ok(if text.is_empty() {
        "available".to_owned()
    } else {
        text.chars().take(160).collect()
    })
}

async fn collect_health(
    active_provider: String,
    full_probe: bool,
    provider_filter: Option<String>,
) -> HealthSnapshot {
    let git_health = match probe_version("git", &["--version"]).await {
        Ok(version) => ToolHealth::Available(version),
        Err(error) => ToolHealth::Unavailable(error),
    };
    let runtime_health = if provider_filter.as_deref().is_none_or(|p| p == "codex")
        && (full_probe || active_provider == "codex")
    {
        CodexRuntime::default().discover().await
    } else {
        CodexRuntimeHealth::Checking
    };
    let antigravity_health = if provider_filter
        .as_deref()
        .is_none_or(|p| p == "antigravity")
        && (full_probe || active_provider == "antigravity")
    {
        AntigravityRuntime::default().discover().await
    } else {
        AntigravityRuntimeHealth::Checking
    };
    let github_cli_health = match probe_version("gh", &["--version"]).await {
        Ok(version) => {
            let auth = timeout(Duration::from_secs(3), {
                let mut command = TokioCommand::new("gh");
                #[cfg(windows)]
                command.creation_flags(0x08000000);
                command
                    .args(["auth", "status", "--active", "--hostname", "github.com"])
                    .kill_on_drop(true)
                    .output()
            })
            .await;
            match auth {
                Ok(Ok(output)) if output.status.success() => {
                    format!("available and authenticated ({version})")
                }
                Ok(Ok(_)) => format!("available but not authenticated ({version})"),
                Ok(Err(error)) => format!("available but auth check failed ({error})"),
                Err(_) => format!("available but auth check timed out ({version})"),
            }
        }
        Err(_) => "not installed (optional integration)".to_owned(),
    };
    HealthSnapshot {
        git_health,
        runtime_health,
        antigravity_health,
        github_cli_health,
    }
}

/// TUI-local production/test selection. Core remains generic over `Runtime`.
#[allow(dead_code)]
#[derive(Debug, Clone)]
enum AppRuntime {
    Codex(CodexRuntime),
    Antigravity(AntigravityRuntime),
    Fake(FakeRuntime),
}

/// Build the single provider router owned by the production scheduler. The
/// router is intentionally long-lived so a queued execution is dispatched by
/// its frozen profile rather than by the mutable UI default.
pub(crate) fn build_runtime_router() -> RuntimeRouter {
    let mut router = RuntimeRouter::new();
    #[cfg(test)]
    {
        let runtime = Arc::new(FakeRuntime::default());
        let runtime_dyn: Arc<dyn Runtime> = runtime;
        router.register(
            "codex",
            runtime_dyn.clone(),
            Arc::new(CodexRuntime::default()),
        );
        router.register(
            "antigravity",
            runtime_dyn,
            Arc::new(AntigravityRuntime::default()),
        );
    }
    #[cfg(not(test))]
    {
        let codex = Arc::new(CodexRuntime::default());
        let codex_runtime: Arc<dyn Runtime> = codex.clone();
        let codex_integration: Arc<dyn RuntimeIntegration> = codex;
        router.register("codex", codex_runtime, codex_integration);
        let antigravity = Arc::new(AntigravityRuntime::default());
        let agy_runtime: Arc<dyn Runtime> = antigravity.clone();
        let agy_integration: Arc<dyn RuntimeIntegration> = antigravity;
        router.register("antigravity", agy_runtime, agy_integration);
    }
    router
}

impl Default for AppRuntime {
    fn default() -> Self {
        #[cfg(test)]
        {
            Self::Fake(FakeRuntime::default())
        }
        #[cfg(not(test))]
        {
            Self::Codex(CodexRuntime::default())
        }
    }
}

impl Runtime for AppRuntime {
    fn identity(&self) -> RuntimeIdentity {
        match self {
            Self::Codex(runtime) => runtime.identity(),
            Self::Antigravity(runtime) => runtime.identity(),
            Self::Fake(runtime) => runtime.identity(),
        }
    }
    fn capabilities(&self) -> RuntimeCapabilities {
        match self {
            Self::Codex(runtime) => runtime.capabilities(),
            Self::Antigravity(runtime) => runtime.capabilities(),
            Self::Fake(runtime) => runtime.capabilities(),
        }
    }
    fn start(
        &self,
        request: RuntimeRequest,
        cancellation: CancellationToken,
    ) -> RuntimeStartFuture {
        match self {
            Self::Codex(runtime) => runtime.start(request, cancellation),
            Self::Antigravity(runtime) => runtime.start(request, cancellation),
            Self::Fake(runtime) => runtime.start(request, cancellation),
        }
    }
}

pub(crate) struct AppState {
    pub(crate) relay: Relay,
    pub(crate) config_path: PathBuf,
    pub(crate) registry_writable: bool,
    pub(crate) database_path: PathBuf,
    pub(crate) store: Option<SqliteStore>,
    pub(crate) durable_tasks: Vec<StoredTask>,
    pub(crate) selected_task: Option<TaskId>,
    pub(crate) scheduler: Option<Arc<ExecutionScheduler<RuntimeRouter>>>,
    pub(crate) runtime_health: CodexRuntimeHealth,
    pub(crate) git_health: ToolHealth,
    // GitHub CLI is an optional integration; GitHub ingress is descoped from
    // v0.1 (ADR-0016), so nothing currently renders this.
    #[allow(dead_code)]
    pub(crate) github_cli_health: String,
    pub(crate) antigravity_health: AntigravityRuntimeHealth,
    pub(crate) active_runtime_provider: String,
    pub(crate) active_profile_id: String,
    pub(crate) active_model: relay_core::RuntimeModelSelection,
    pub(crate) active_reasoning_effort: relay_core::RuntimeReasoningSelection,
    pub(crate) one_run_model_override: Option<String>,
    pub(crate) one_run_reasoning_override: Option<String>,
    pub(crate) runtime_settings: runtime_settings::RuntimeSettings,
    pub(crate) runtime_detail_provider: Option<String>,
    pub(crate) diff_output: Option<String>,
    pub(crate) diff_scroll: u16,
    pub(crate) logs_scroll: u16,
    pub(crate) execution_scroll: u16,
    pub(crate) execution_follow_tail: bool,
    pub(crate) result_scroll: u16,
    pub(crate) health_refresh: Option<JoinHandle<HealthSnapshot>>,
    pub(crate) command_input: String,
    pub(crate) command_mode: bool,
    pub(crate) command_selected_index: usize,
    #[allow(dead_code)]
    pub(crate) command_scroll_offset: usize,
    pub(crate) scheduler_events: Option<broadcast::Receiver<CoreExecutionEvent>>,
    pub(crate) scheduler_snapshot: SchedulerSnapshot,
    pub(crate) scheduler_driver:
        Option<JoinHandle<Result<Vec<ExecutionReport>, relay_core::ExecutionSchedulerError>>>,
    pub(crate) execution_views: HashMap<TaskId, ExecutionViewState>,
    pub(crate) last_reports: HashMap<TaskId, ExecutionReport>,
    pub(crate) execution_records: HashMap<TaskId, ExecutionRecord>,
    pub(crate) preflight_retry_pending: HashSet<TaskId>,
    pub(crate) scheduler_rebind_pending: bool,
    pub(crate) cancel_confirmation: Option<TaskId>,
    pub(crate) screen: Screen,
    pub(crate) add_project: AddProjectForm,
    pub(crate) discovered_project: Option<DiscoveredProject>,
    pub(crate) add_field: AddProjectField,
    pub(crate) import_file_path: String,
    pub(crate) paste_text: String,
    pub(crate) task_candidate: Option<TaskImportCandidate>,
    pub(crate) new_task_option: usize,
    pub(crate) repository_inspections: BTreeMap<ProjectKey, RepositoryInspection>,
    pub(crate) inspection_refresh: Option<JoinHandle<BTreeMap<ProjectKey, RepositoryInspection>>>,
    pub(crate) preflight_cache: HashMap<TaskId, (String, Instant)>,
    pub(crate) preflight_refreshes: HashMap<TaskId, JoinHandle<String>>,
    pub(crate) feedback: String,
    pub(crate) pending_permissions: Vec<(TaskId, RuntimePermissionRequest)>,
    pub(crate) cached_model_catalog: Option<relay_runtime::RuntimeModelCatalog>,
    pub(crate) runtime_detail_menu_index: usize,
    pub(crate) runtime_detail_submenu: Option<RuntimeDetailSubmenu>,
    /// Selected row on the Runtimes screen (index into `RUNTIME_PROVIDERS`).
    pub(crate) runtimes_list_index: usize,
    /// Selected row on the Doctor screen (index into `DOCTOR_CHECKS`).
    pub(crate) doctor_selected_index: usize,
    pub(crate) task_review_menu_index: usize,
    pub(crate) task_review_submenu: Option<TaskReviewSubmenu>,
    pub(crate) cached_codex_account: Option<String>,
    /// Path the runtime-settings TOML is saved to; defaults to
    /// `RuntimeSettings::default_path()` and is overridden by tests.
    pub(crate) runtime_config_path: PathBuf,
    /// Execution limits currently shown/applied by this process.
    pub(crate) execution_limits: ExecutionLimits,
    pub(crate) settings_form: SettingsForm,
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeDetailSubmenu {
    ModelSelector,
    ReasoningSelector,
    ProfileSelector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskReviewSubmenu {
    ModelSelector,
    ReasoningSelector,
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionViewState {
    pub(crate) frozen_profile: Option<String>,
    pub(crate) frozen_model: Option<String>,
    pub(crate) frozen_reasoning: Option<String>,
    pub(crate) scheduler_status: Option<SchedulerTaskStatus>,
    pub(crate) durable_state: Option<TaskState>,
    pub(crate) repair_attempt: u32,
    pub(crate) output: VecDeque<String>,
    pub(crate) output_bytes: usize,
    pub(crate) persistence_error: Option<String>,
    pub(crate) cancellation_requested: bool,
    pub(crate) started_at: Option<Instant>,
}

#[derive(Debug)]
pub(crate) struct HealthSnapshot {
    git_health: ToolHealth,
    runtime_health: CodexRuntimeHealth,
    antigravity_health: AntigravityRuntimeHealth,
    // GitHub CLI remains an optional integration only; GitHub ingress is
    // descoped from v0.1 (ADR-0016), so nothing currently renders this.
    #[allow(dead_code)]
    github_cli_health: String,
}

const UI_OUTPUT_BYTE_LIMIT: usize = 16 * 1024;
const PREFLIGHT_CACHE_TTL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolHealth {
    Checking,
    Available(String),
    Unavailable(String),
}

/// Outcome of a single Doctor diagnostic row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DoctorCheckStatus {
    Pending,
    Ok(String),
    Failed(String),
}

/// Doctor checks in selectable display order. `codex` and `antigravity` have
/// targeted re-probes (`begin_provider_health_refresh`); the others are probed
/// together with the full diagnostics refresh.
pub(crate) const DOCTOR_CHECKS: [&str; 5] = ["git", "config", "sqlite", "codex", "antigravity"];

/// Fix-it guidance shown when the user presses Enter on a local-state Doctor
/// row (git / config / sqlite). Provider rows navigate instead of using this.
fn doctor_remediation_hint(check: &str, state: &AppState) -> String {
    match check {
        "git" => match &state.git_health {
            ToolHealth::Available(version) => {
                format!("Git is healthy ({version}). Nothing to fix.")
            }
            ToolHealth::Unavailable(error) => format!(
                "Git is unavailable: {error}. Install Git and make sure 'git' is on PATH, \
then press r to re-check."
            ),
            ToolHealth::Checking => {
                "Git probe still running; press r to re-check in a moment.".to_owned()
            }
        },
        "config" => {
            let path = state.config_path.display().to_string();
            if state.registry_writable {
                format!(
                    "Config loaded from {path}. Edit it with a text editor; restart Relay to apply."
                )
            } else {
                format!(
                    "Config at {path} could not be loaded or written. Check the file for parse \
errors or permission problems, fix it externally, then press r to re-check."
                )
            }
        }
        "sqlite" => {
            let path = state.database_path.display().to_string();
            if state.store.is_some() {
                format!("Local state database opened from {path}. Nothing to fix.")
            } else {
                format!(
                    "Local state database at {path} could not be opened. Check disk space and \
file permissions, then press r to re-check. Relay fails closed without durable state."
                )
            }
        }
        _ => "Select a row and press Enter for guidance; provider rows open their runtime screen."
            .to_owned(),
    }
}

pub(crate) fn reasoning_effort_label(effort: relay_core::ReasoningEffort) -> &'static str {
    match effort {
        relay_core::ReasoningEffort::Default => "default",
        relay_core::ReasoningEffort::Low => "low",
        relay_core::ReasoningEffort::Medium => "medium",
        relay_core::ReasoningEffort::High => "high",
        relay_core::ReasoningEffort::XHigh => "xhigh",
    }
}

impl AppState {
    pub(crate) fn begin_health_refresh(&mut self, full_probe: bool) {
        if let Some(task) = self.health_refresh.take() {
            task.abort();
        }
        if full_probe || self.active_runtime_provider == "codex" {
            self.runtime_health = CodexRuntimeHealth::Checking;
        }
        if full_probe || self.active_runtime_provider == "antigravity" {
            self.antigravity_health = AntigravityRuntimeHealth::Checking;
        }
        self.git_health = ToolHealth::Checking;
        self.github_cli_health = "checking (optional integration)".to_owned();
        let provider = self.active_runtime_provider.clone();
        self.health_refresh = Some(tokio::spawn(collect_health(provider, full_probe, None)));
    }

    pub(crate) fn begin_provider_health_refresh(&mut self, provider: String) {
        if let Some(task) = self.health_refresh.take() {
            task.abort();
        }
        self.health_refresh = Some(tokio::spawn(collect_health(
            provider.clone(),
            true,
            Some(provider),
        )));
    }

    /// Status of one Doctor check row, derived from already-probed state.
    pub(crate) fn doctor_check_status(&self, check: &str) -> DoctorCheckStatus {
        match check {
            "git" => match &self.git_health {
                ToolHealth::Checking => DoctorCheckStatus::Pending,
                ToolHealth::Available(version) => DoctorCheckStatus::Ok(version.clone()),
                ToolHealth::Unavailable(error) => DoctorCheckStatus::Failed(error.clone()),
            },
            "config" => {
                let path = self.config_path.display().to_string();
                if self.registry_writable {
                    DoctorCheckStatus::Ok(path)
                } else {
                    DoctorCheckStatus::Failed(path)
                }
            }
            "sqlite" => {
                let path = self.database_path.display().to_string();
                if self.store.is_some() {
                    DoctorCheckStatus::Ok(path)
                } else {
                    DoctorCheckStatus::Failed(path)
                }
            }
            "codex" => match &self.runtime_health {
                CodexRuntimeHealth::Checking => DoctorCheckStatus::Pending,
                CodexRuntimeHealth::Available { version, .. } => {
                    DoctorCheckStatus::Ok(version.clone())
                }
                CodexRuntimeHealth::NotAuthenticated { .. } => {
                    DoctorCheckStatus::Failed("authentication unavailable".to_owned())
                }
                CodexRuntimeHealth::Incompatible { reason, .. } => {
                    DoctorCheckStatus::Failed(reason.clone())
                }
                CodexRuntimeHealth::NotInstalled { error }
                | CodexRuntimeHealth::DiscoveryFailed { error } => {
                    DoctorCheckStatus::Failed(error.clone())
                }
            },
            "antigravity" => {
                if matches!(self.antigravity_health, AntigravityRuntimeHealth::Checking) {
                    DoctorCheckStatus::Pending
                } else if self.antigravity_health.is_ready() {
                    DoctorCheckStatus::Ok(self.antigravity_health.display_status().to_owned())
                } else {
                    DoctorCheckStatus::Failed(self.antigravity_health.display_status().to_owned())
                }
            }
            _ => DoctorCheckStatus::Pending,
        }
    }

    pub(crate) fn select_next_doctor_check(&mut self) {
        self.doctor_selected_index = (self.doctor_selected_index + 1) % DOCTOR_CHECKS.len();
    }

    pub(crate) fn select_previous_doctor_check(&mut self) {
        self.doctor_selected_index =
            (self.doctor_selected_index + DOCTOR_CHECKS.len() - 1) % DOCTOR_CHECKS.len();
    }

    /// Re-runs the selected check. Provider rows use their targeted
    /// re-probe; git/config/sqlite are probed together at startup, so those
    /// rows re-run the full diagnostics refresh.
    pub(crate) fn recheck_doctor_selection(&mut self) {
        let check = DOCTOR_CHECKS
            .get(self.doctor_selected_index)
            .copied()
            .unwrap_or("git");
        match check {
            "codex" | "antigravity" => {
                if check == "codex" {
                    self.runtime_health = CodexRuntimeHealth::Checking;
                } else {
                    self.antigravity_health = AntigravityRuntimeHealth::Checking;
                }
                self.begin_provider_health_refresh(check.to_owned());
            }
            _ => self.begin_health_refresh(true),
        }
    }

    /// Runs the primary fix-it action for the selected Doctor row. Provider
    /// rows jump straight to that runtime's detail screen, where profile,
    /// model, and executable overrides live; local-state rows surface
    /// remediation guidance in the feedback line instead of navigating.
    pub(crate) fn doctor_primary_action(&mut self) {
        let check = DOCTOR_CHECKS
            .get(self.doctor_selected_index)
            .copied()
            .unwrap_or("git");
        match check {
            "codex" | "antigravity" => {
                self.runtime_detail_provider = Some(check.to_owned());
                self.screen = Screen::RuntimeDetail;
                self.begin_health_refresh(true);
            }
            _ => self.feedback = doctor_remediation_hint(check, self),
        }
    }

    pub(crate) async fn poll_health_refresh(&mut self) {
        let Some(task) = self.health_refresh.as_ref() else {
            return;
        };
        if !task.is_finished() {
            return;
        }
        let task = self.health_refresh.take().expect("health task exists");
        match task.await {
            Ok(snapshot) => {
                self.git_health = snapshot.git_health;
                if snapshot.runtime_health != CodexRuntimeHealth::Checking {
                    if let CodexRuntimeHealth::Available { ref account, .. } =
                        snapshot.runtime_health
                    {
                        self.cached_codex_account = account.clone();
                    }
                    self.runtime_health = snapshot.runtime_health;
                }
                if snapshot.antigravity_health != AntigravityRuntimeHealth::Checking {
                    self.antigravity_health = snapshot.antigravity_health;
                }
                self.github_cli_health = snapshot.github_cli_health;

                let provider = self
                    .runtime_detail_provider
                    .as_deref()
                    .unwrap_or(&self.active_runtime_provider);
                if let Some(integration) = build_runtime_router().integration(provider) {
                    self.cached_model_catalog = Some(integration.models().await);
                }
            }
            Err(_) => {
                self.git_health = ToolHealth::Unavailable("health probe failed".to_owned());
                self.runtime_health = CodexRuntimeHealth::NotInstalled {
                    error: "health probe failed".to_owned(),
                };
                self.antigravity_health = AntigravityRuntimeHealth::NotInstalled {
                    error: "health probe failed".to_owned(),
                };
                self.github_cli_health = "health probe failed".to_owned();
                self.feedback = "Health probe failed; statuses marked unavailable.".to_owned();
            }
        }
    }

    pub(crate) fn start_detected_project(&mut self, path: Option<PathBuf>) {
        let path = path.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        match discover_project(&path) {
            Ok(discovered) => {
                self.start_add_project();
                self.add_project.path = discovered.repository_root.display().to_string();
                self.add_project.key = discovered.suggested_key.clone();
                self.add_project.expected_remote = discovered.remote.clone().unwrap_or_default();
                self.add_project.default_stack = discovered
                    .suggested_stack
                    .clone()
                    .unwrap_or_else(|| "chatgpt-codex".to_owned());
                self.add_project.github_owner = discovered.github_owner.clone().unwrap_or_default();
                self.add_project.github_repo = discovered.github_repo.clone().unwrap_or_default();
                self.discovered_project = Some(discovered);
                self.feedback = "Detected repository. Review the suggested values and press Enter to add it, or edit details first.".to_owned();
            }
            Err(error) => {
                self.start_add_project();
                self.feedback = format!(
                    "Repository detection failed: {error}\nAdvanced setup is available for manual registration."
                );
            }
        }
    }

    pub(crate) fn select_task_by_selector(&mut self, selector: &str) -> bool {
        let matches = self
            .durable_tasks
            .iter()
            .filter(|task| task.id.to_string().starts_with(selector))
            .map(|task| task.id)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [task_id] => {
                self.selected_task = Some(*task_id);
                true
            }
            [] => {
                self.feedback = "No Relay task matches that selector.".to_owned();
                false
            }
            _ => {
                self.feedback =
                    "Task selector is ambiguous; provide more of the TaskId.".to_owned();
                false
            }
        }
    }

    pub(crate) async fn run_task_selector(&mut self, selector: &str) {
        if self.select_task_by_selector(selector) {
            self.submit_selected_task().await;
        }
    }

    pub(crate) fn build_completion_context(&self) -> command_completion::CompletionContext {
        let codex_available = self.runtime_health.is_ready();
        let antigravity_available = matches!(
            self.antigravity_health,
            AntigravityRuntimeHealth::Available { .. }
                | AntigravityRuntimeHealth::AuthUnverifiable { .. }
        );
        let providers = vec![
            command_completion::ProviderCompletion {
                provider_id: "codex".to_owned(),
                display_name: "Codex".to_owned(),
                status_note: self.runtime_health.display_status().to_owned(),
            },
            command_completion::ProviderCompletion {
                provider_id: "antigravity".to_owned(),
                display_name: "Antigravity".to_owned(),
                status_note: self.antigravity_health.display_status().to_owned(),
            },
        ];

        let profiles = vec![
            command_completion::ProfileCompletion {
                profile_id: "codex/implement".to_owned(),
                provider_id: "codex".to_owned(),
                display_name: "Codex / Implement".to_owned(),
                description: "Code generation and workspace edits".to_owned(),
                available: codex_available,
                disabled_reason: (!codex_available)
                    .then(|| self.runtime_health.display_status().to_owned()),
            },
            command_completion::ProfileCompletion {
                profile_id: "codex/review".to_owned(),
                provider_id: "codex".to_owned(),
                display_name: "Codex / Review".to_owned(),
                description: "Read-only inspection & review".to_owned(),
                available: codex_available,
                disabled_reason: (!codex_available)
                    .then(|| self.runtime_health.display_status().to_owned()),
            },
            command_completion::ProfileCompletion {
                profile_id: "codex/deep".to_owned(),
                provider_id: "codex".to_owned(),
                display_name: "Codex / Deep".to_owned(),
                description: "High reasoning effort agent turns".to_owned(),
                available: codex_available,
                disabled_reason: (!codex_available)
                    .then(|| self.runtime_health.display_status().to_owned()),
            },
            command_completion::ProfileCompletion {
                profile_id: "antigravity/implement".to_owned(),
                provider_id: "antigravity".to_owned(),
                display_name: "Antigravity / Implement".to_owned(),
                description: "Google Antigravity autonomous edits".to_owned(),
                available: antigravity_available,
                disabled_reason: (!antigravity_available)
                    .then(|| self.antigravity_health.display_status().to_owned()),
            },
            command_completion::ProfileCompletion {
                profile_id: "antigravity/review".to_owned(),
                provider_id: "antigravity".to_owned(),
                display_name: "Antigravity / Review".to_owned(),
                description: "Read-only planning & inspection mode".to_owned(),
                available: antigravity_available,
                disabled_reason: (!antigravity_available)
                    .then(|| self.antigravity_health.display_status().to_owned()),
            },
            command_completion::ProfileCompletion {
                profile_id: "antigravity/deep".to_owned(),
                provider_id: "antigravity".to_owned(),
                display_name: "Antigravity / Deep".to_owned(),
                description: "High effort agent turns".to_owned(),
                available: antigravity_available,
                disabled_reason: (!antigravity_available)
                    .then(|| self.antigravity_health.display_status().to_owned()),
            },
        ];

        let mut ready_tasks = Vec::new();
        let mut cancellable_tasks = Vec::new();
        let mut inspectable_tasks = Vec::new();

        for task in &self.durable_tasks {
            let id_str = task.id.to_string();
            let short_id = if id_str.len() >= 8 {
                id_str[..8].to_owned()
            } else {
                id_str.clone()
            };

            let item = command_completion::TaskCompletion {
                id_str: id_str.clone(),
                short_id,
                project_key: task.project_key.to_string(),
                title: task.handoff.title.clone(),
                state: task.state,
                eligible_for_run: task.state == TaskState::Ready,
            };

            if task.state == TaskState::Ready {
                ready_tasks.push(item.clone());
            }
            if matches!(task.state, TaskState::Running | TaskState::Verifying)
                || self
                    .execution_views
                    .get(&task.id)
                    .map(|v| v.scheduler_status.is_some())
                    .unwrap_or(false)
            {
                cancellable_tasks.push(item.clone());
            }
            inspectable_tasks.push(item);
        }

        let models = self
            .cached_model_catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .available_models
                    .iter()
                    .map(|m| m.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let reasoning_efforts = vec![
            "low".to_owned(),
            "medium".to_owned(),
            "high".to_owned(),
            "xhigh".to_owned(),
        ];

        command_completion::CompletionContext {
            providers,
            profiles,
            ready_tasks,
            cancellable_tasks,
            inspectable_tasks,
            models,
            reasoning_efforts,
        }
    }

    pub(crate) fn get_current_suggestions(&self) -> Vec<command_completion::CommandSuggestion> {
        let ctx = self.build_completion_context();
        command_completion::compute_suggestions(&self.command_input, &ctx)
    }

    /// Built-in profiles offered by a provider through the runtime seam.
    pub(crate) fn provider_profiles(
        provider: &str,
    ) -> Vec<relay_runtime::RuntimeProfileDescriptor> {
        build_runtime_router()
            .integration(provider)
            .map(|integration| integration.profiles())
            .unwrap_or_default()
    }

    /// Applies a runtime profile selection through the same validation path as
    /// `/runtime use`: provider lookup, profile resolution, cross-provider
    /// tuning re-resolution (C8/C9), and settings persistence.
    pub(crate) fn apply_runtime_profile(&mut self, profile: &str) {
        let Some(provider) = profile.split('/').next().filter(|p| !p.is_empty()) else {
            self.feedback = "Runtime profile must use provider/name format.".to_owned();
            return;
        };
        let router = build_runtime_router();
        let Some(integration) = router.integration(provider) else {
            self.feedback = format!("Unknown runtime provider '{provider}'.");
            return;
        };
        if let Err(error) = integration.resolve_profile(profile) {
            self.feedback = format!("Cannot use runtime profile '{profile}': {error}");
            return;
        }
        self.active_runtime_provider = provider.to_owned();
        self.active_profile_id = profile.to_owned();
        self.runtime_settings.runtime.default_profile = profile.to_owned();
        // C8/C9: Re-resolve tuning from the new profile to prevent
        // cross-provider model/reasoning leakage.
        let resolved = self
            .runtime_settings
            .resolve_selection(None, Some(profile), None, None);
        self.active_model = resolved.model;
        self.active_reasoning_effort = resolved.reasoning_effort;
        self.one_run_model_override = None;
        self.one_run_reasoning_override = None;
        match self
            .runtime_settings
            .save_to_path(&self.runtime_config_path)
        {
            Ok(()) => self.feedback = format!("Active runtime profile: {}", self.active_profile_id),
            Err(error) => {
                self.feedback =
                    format!("Runtime selected in memory but could not persist settings: {error}")
            }
        }
    }

    /// Reasoning efforts offered for selection. When a model catalog for the
    /// active provider is available, options are filtered to the effective
    /// model's `supported_reasoning_efforts`; without a catalog the standard
    /// effort list is offered.
    pub(crate) fn reasoning_options_for_model(
        &self,
        preferred_model: Option<&str>,
    ) -> Vec<relay_core::ReasoningEffort> {
        const ALL: [relay_core::ReasoningEffort; 4] = [
            relay_core::ReasoningEffort::Low,
            relay_core::ReasoningEffort::Medium,
            relay_core::ReasoningEffort::High,
            relay_core::ReasoningEffort::XHigh,
        ];
        let Some(catalog) = &self.cached_model_catalog else {
            return ALL.to_vec();
        };
        let effective_model = preferred_model
            .filter(|model| !model.is_empty() && *model != "default")
            .or_else(|| self.active_model.as_option())
            .map(str::to_owned)
            .unwrap_or_else(|| catalog.default_model.clone());
        let Some(model_info) = catalog
            .available_models
            .iter()
            .find(|candidate| candidate.id == effective_model)
        else {
            return ALL.to_vec();
        };
        if model_info.supported_reasoning_efforts.is_empty() {
            return ALL.to_vec();
        }
        ALL.into_iter()
            .filter(|effort| model_info.supported_reasoning_efforts.contains(effort))
            .collect()
    }

    pub(crate) fn cancel_task_selector(&mut self, selector: &str) {
        let matches = self
            .durable_tasks
            .iter()
            .filter(|task| task.id.to_string().starts_with(selector))
            .map(|task| task.id)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [task_id] => {
                self.selected_task = Some(*task_id);
                self.cancel_confirmation = Some(*task_id);
                self.screen = Screen::Execution;
                self.feedback = "Cancel execution? Press Y to confirm.".to_owned();
            }
            [] => self.feedback = "No Relay task matches that selector.".to_owned(),
            _ => {
                self.feedback = "Task selector is ambiguous; provide more of the TaskId.".to_owned()
            }
        }
    }
    pub(crate) fn runtime_label(&self) -> String {
        self.runtime_label_for_profile(&self.active_profile_id)
    }

    pub(crate) fn runtime_label_for_profile(&self, profile: &str) -> String {
        let provider = profile.split('/').next().unwrap_or("codex");
        format!(
            "{}\nProfile: {}\nStatus: {}",
            if provider == "antigravity" {
                "Antigravity"
            } else {
                "Codex"
            },
            profile,
            if provider == "antigravity" {
                self.antigravity_health.display_status()
            } else {
                self.runtime_health.display_status()
            }
        )
    }
    #[cfg(test)]
    pub(crate) fn new(
        relay: Relay,
        config_path: PathBuf,
        registry_writable: bool,
        feedback: String,
    ) -> Self {
        Self::new_with_store(
            relay,
            config_path,
            registry_writable,
            PathBuf::from("relay-state.sqlite3"),
            None,
            Vec::new(),
            feedback,
        )
    }

    pub(crate) fn new_with_store(
        relay: Relay,
        config_path: PathBuf,
        registry_writable: bool,
        database_path: PathBuf,
        store: Option<SqliteStore>,
        durable_tasks: Vec<StoredTask>,
        feedback: String,
    ) -> Self {
        // Test builds stay hermetic: the developer's real runtimes.toml
        // (e.g. an antigravity default profile) must not decide test outcomes.
        #[cfg(test)]
        let runtime_settings = runtime_settings::RuntimeSettings::default();
        #[cfg(not(test))]
        let runtime_settings = runtime_settings::RuntimeSettings::load_from_path(
            &runtime_settings::RuntimeSettings::default_path(),
        )
        .unwrap_or_default();
        let default_profile = runtime_settings.runtime.default_profile.clone();
        let scheduler = store.as_ref().map(|store| {
            let mut router = build_runtime_router();
            router.set_default_profile(default_profile.clone());
            Arc::new(ExecutionScheduler::new(
                relay.clone(),
                store.clone(),
                Arc::new(router),
                app_execution_limits(),
            ))
        });
        let scheduler_events = scheduler.as_ref().map(|scheduler| scheduler.subscribe());
        Self {
            relay,
            config_path,
            registry_writable,
            database_path,
            store,
            durable_tasks,
            selected_task: None,
            scheduler,
            runtime_health: CodexRuntimeHealth::Checking,
            git_health: ToolHealth::Checking,
            github_cli_health: "not checked (optional integration)".to_owned(),
            antigravity_health: AntigravityRuntimeHealth::Checking,
            active_runtime_provider: default_profile
                .split('/')
                .next()
                .unwrap_or("codex")
                .to_owned(),
            active_profile_id: default_profile.clone(),
            active_model: {
                let resolved =
                    runtime_settings.resolve_selection(None, Some(&default_profile), None, None);
                resolved.model
            },
            active_reasoning_effort: {
                let resolved =
                    runtime_settings.resolve_selection(None, Some(&default_profile), None, None);
                resolved.reasoning_effort
            },
            one_run_model_override: None,
            one_run_reasoning_override: None,
            runtime_settings,
            runtime_detail_provider: None,
            diff_output: None,
            diff_scroll: 0,
            logs_scroll: 0,
            execution_scroll: 0,
            execution_follow_tail: true,
            result_scroll: 0,
            health_refresh: None,
            command_input: String::new(),
            command_mode: false,
            command_selected_index: 0,
            command_scroll_offset: 0,
            scheduler_events,
            scheduler_snapshot: SchedulerSnapshot { tasks: Vec::new() },
            scheduler_driver: None,
            execution_views: HashMap::new(),
            last_reports: HashMap::new(),
            execution_records: HashMap::new(),
            preflight_retry_pending: HashSet::new(),
            scheduler_rebind_pending: false,
            cancel_confirmation: None,
            screen: Screen::Dashboard,
            add_project: AddProjectForm::default(),
            discovered_project: None,
            add_field: AddProjectField::Key,
            import_file_path: String::new(),
            paste_text: String::new(),
            task_candidate: None,
            new_task_option: 0,
            repository_inspections: BTreeMap::new(),
            inspection_refresh: None,
            preflight_cache: HashMap::new(),
            preflight_refreshes: HashMap::new(),
            feedback,
            pending_permissions: Vec::new(),
            cached_model_catalog: None,
            runtime_detail_menu_index: 0,
            runtime_detail_submenu: None,
            runtimes_list_index: 0,
            doctor_selected_index: 0,
            task_review_menu_index: 0,
            task_review_submenu: None,
            cached_codex_account: None,
            runtime_config_path: runtime_settings::RuntimeSettings::default_path(),
            execution_limits: app_execution_limits(),
            settings_form: SettingsForm::default(),
        }
    }

    pub(crate) fn handle_paste(&mut self, text: &str) {
        if self.command_mode {
            self.command_input.push_str(text);
        } else if self.screen == Screen::PasteTask {
            self.paste_text.push_str(text);
        }
    }

    pub fn input_context(&self) -> events::InputContext {
        if self.command_mode {
            events::InputContext::CommandPrompt
        } else {
            match self.screen {
                Screen::PasteTask | Screen::ImportFile => events::InputContext::TextEditor,
                Screen::AddProject => {
                    if self.discovered_project.is_some() {
                        events::InputContext::Selection
                    } else {
                        events::InputContext::TextEditor
                    }
                }
                Screen::Tasks
                | Screen::Projects
                | Screen::NewTask
                | Screen::Runtimes
                | Screen::RuntimeDetail
                | Screen::Settings
                | Screen::Doctor => events::InputContext::Selection,
                Screen::TaskReview => events::InputContext::Confirmation,
                Screen::Dashboard
                | Screen::TaskPreview
                | Screen::Execution
                | Screen::Result
                | Screen::Diff
                | Screen::Logs => events::InputContext::PassiveView,
                Screen::Updates => events::InputContext::PassiveView,
            }
        }
    }

    pub(crate) fn enter_projects(&mut self) {
        self.refresh_repository_inspections();
        self.screen = Screen::Projects;
    }

    pub(crate) fn select_next_runtime(&mut self) {
        self.runtimes_list_index = (self.runtimes_list_index + 1) % RUNTIME_PROVIDERS.len();
    }

    pub(crate) fn select_previous_runtime(&mut self) {
        self.runtimes_list_index =
            (self.runtimes_list_index + RUNTIME_PROVIDERS.len() - 1) % RUNTIME_PROVIDERS.len();
    }

    /// Opens Runtime Detail for the provider highlighted on the Runtimes
    /// screen, mirroring `/runtime status <provider>`.
    pub(crate) fn open_selected_runtime(&mut self) {
        let index = self.runtimes_list_index.min(RUNTIME_PROVIDERS.len() - 1);
        let provider = RUNTIME_PROVIDERS[index];
        self.runtime_detail_provider = Some(provider.to_owned());
        self.screen = Screen::RuntimeDetail;
        self.begin_health_refresh(true);
    }

    pub(crate) fn enter_tasks(&mut self) {
        if self.selected_task.is_none() {
            self.selected_task = self.durable_tasks.first().map(|task| task.id);
        }
        self.screen = Screen::Tasks;
    }

    pub(crate) fn selected_stored_task(&self) -> Option<&StoredTask> {
        self.selected_task
            .and_then(|id| self.durable_tasks.iter().find(|task| task.id == id))
    }

    pub(crate) fn select_next_task(&mut self) {
        if self.durable_tasks.is_empty() {
            return;
        }
        let current = self
            .selected_task
            .and_then(|id| self.durable_tasks.iter().position(|task| task.id == id))
            .unwrap_or(0);
        self.selected_task = Some(self.durable_tasks[(current + 1) % self.durable_tasks.len()].id);
    }

    pub(crate) fn select_previous_task(&mut self) {
        if self.durable_tasks.is_empty() {
            return;
        }
        let current = self
            .selected_task
            .and_then(|id| self.durable_tasks.iter().position(|task| task.id == id))
            .unwrap_or(0);
        self.selected_task = Some(
            self.durable_tasks[(current + self.durable_tasks.len() - 1) % self.durable_tasks.len()]
                .id,
        );
    }

    pub(crate) fn open_selected_task(&mut self) {
        let Some(task) = self.selected_stored_task() else {
            return;
        };
        let active = self
            .scheduler_snapshot
            .tasks
            .iter()
            .any(|snapshot| snapshot.task_id == task.id);
        self.screen = if active {
            self.execution_scroll = 0;
            self.execution_follow_tail = true;
            Screen::Execution
        } else if matches!(
            task.state,
            TaskState::Done | TaskState::Failed | TaskState::Cancelled
        ) {
            self.result_scroll = 0;
            Screen::Result
        } else {
            self.begin_preflight_refresh(task.id);
            Screen::TaskReview
        };
    }

    /// Local-only preflight checks that never shell out to git.
    /// Returns `Some(reason)` when the task is unavailable for reasons other
    /// than repository eligibility; `None` means the repository inspection
    /// decides the outcome.
    pub(crate) fn preflight_without_repository(&self, task: &StoredTask) -> Option<String> {
        if task.state != TaskState::Ready {
            return Some("unavailable: task is not READY".to_owned());
        }
        if self.scheduler.is_none() {
            return Some("unavailable: local runtime or store is unavailable".to_owned());
        }
        let provider = self.active_profile_id.split('/').next().unwrap_or_default();
        let health_ready = match provider {
            "codex" => self.runtime_health.is_ready(),
            "antigravity" => matches!(
                self.antigravity_health,
                AntigravityRuntimeHealth::Available { .. }
                    | AntigravityRuntimeHealth::AuthUnverifiable { .. }
            ),
            _ => false,
        };
        if !health_ready {
            return Some(format!(
                "unavailable: runtime provider '{provider}' is not ready"
            ));
        }
        match self.effective_stack(task) {
            Ok("chatgpt-codex" | "codex" | "antigravity" | "agy") => {}
            Ok(stack) => return Some(format!("unavailable: unsupported runtime stack '{stack}'")),
            Err(error) => return Some(format!("unavailable: {error}")),
        }
        if !task.handoff.extensions.is_empty() {
            return Some("unavailable: unsupported task extensions".to_owned());
        }
        None
    }

    /// Cached, render-safe preflight status. Reads only from the preflight
    /// cache; git inspection happens asynchronously via `begin_preflight_refresh`.
    pub(crate) fn ui_preflight(&self, task: &StoredTask) -> String {
        if let Some(reason) = self.preflight_without_repository(task) {
            return reason;
        }
        match self.preflight_cache.get(&task.id) {
            Some((status, refreshed_at)) if refreshed_at.elapsed() <= PREFLIGHT_CACHE_TTL => {
                status.clone()
            }
            _ => "checking repository…".to_owned(),
        }
    }

    /// Authoritative preflight check for run gating; performs the repository
    /// inspection off the UI thread and refreshes the cache.
    pub(crate) async fn fresh_ui_preflight(&mut self, task: &StoredTask) -> String {
        if let Some(reason) = self.preflight_without_repository(task) {
            return reason;
        }
        let Ok(project) = self.relay.resolve_project(task.project_key.as_str()) else {
            return "unavailable: project missing".to_owned();
        };
        let project = project.clone();
        let status = tokio::task::spawn_blocking(move || repository_preflight_status(&project))
            .await
            .unwrap_or_else(|_| "unavailable: repository inspection failed".to_owned());
        self.preflight_cache
            .insert(task.id, (status.clone(), Instant::now()));
        status
    }

    pub(crate) fn begin_preflight_refresh(&mut self, task_id: TaskId) {
        if self.preflight_refreshes.contains_key(&task_id) {
            return;
        }
        let Some(task) = self.durable_tasks.iter().find(|task| task.id == task_id) else {
            return;
        };
        if let Some(reason) = self.preflight_without_repository(task) {
            self.preflight_cache
                .insert(task_id, (reason, Instant::now()));
            return;
        }
        let Ok(project) = self.relay.resolve_project(task.project_key.as_str()) else {
            self.preflight_cache.insert(
                task_id,
                ("unavailable: project missing".to_owned(), Instant::now()),
            );
            return;
        };
        let project = project.clone();
        let handle = tokio::task::spawn_blocking(move || repository_preflight_status(&project));
        self.preflight_refreshes.insert(task_id, handle);
    }

    pub(crate) async fn poll_preflight_refreshes(&mut self) {
        let finished: Vec<TaskId> = self
            .preflight_refreshes
            .iter()
            .filter(|(_, handle)| handle.is_finished())
            .map(|(task_id, _)| *task_id)
            .collect();
        for task_id in finished {
            let handle = self
                .preflight_refreshes
                .remove(&task_id)
                .expect("preflight handle exists");
            if let Ok(status) = handle.await {
                self.preflight_cache
                    .insert(task_id, (status, Instant::now()));
            } else {
                self.preflight_cache.remove(&task_id);
            }
        }
    }

    pub(crate) fn effective_stack<'a>(&'a self, task: &'a StoredTask) -> Result<&'a str, String> {
        let project = self
            .relay
            .resolve_project(task.project_key.as_str())
            .map_err(|_| "project is not registered".to_owned())?;
        task.handoff
            .stack
            .as_deref()
            .or_else(|| project.default_stack())
            .ok_or_else(|| "no runtime stack is configured".to_owned())
    }

    pub(crate) async fn submit_selected_task(&mut self) {
        self.reap_finished_scheduler_driver().await;
        if self.scheduler_rebind_pending {
            self.rebind_scheduler_if_idle();
        }
        if self.scheduler_rebind_pending {
            self.feedback =
                "Execution refresh pending; new Run approvals resume after active work is idle."
                    .to_owned();
            return;
        }
        let Some(task) = self.selected_stored_task().cloned() else {
            return;
        };
        if task.state != TaskState::Ready
            || self
                .scheduler_snapshot
                .tasks
                .iter()
                .any(|snapshot| snapshot.task_id == task.id)
            || self
                .execution_views
                .get(&task.id)
                .and_then(|view| view.scheduler_status.as_ref())
                .is_some()
        {
            self.feedback = "Run is available only once for a durable READY task.".to_owned();
            return;
        }
        let preflight = self.fresh_ui_preflight(&task).await;
        if preflight != "eligible" {
            self.feedback = format!("Run unavailable: {preflight}");
            return;
        }
        let Some(scheduler) = self.scheduler.clone() else {
            return;
        };
        let one_run_effort = match self.one_run_reasoning_override.as_deref() {
            None => None,
            Some("low") => Some(relay_core::ReasoningEffort::Low),
            Some("medium") => Some(relay_core::ReasoningEffort::Medium),
            Some("high") => Some(relay_core::ReasoningEffort::High),
            Some("xhigh") => Some(relay_core::ReasoningEffort::XHigh),
            Some(other) => {
                self.feedback = format!("Unsupported one-run reasoning effort '{other}'.");
                self.one_run_reasoning_override = None;
                return;
            }
        };
        let one_run_model = self
            .one_run_model_override
            .as_deref()
            .filter(|model| *model != "default")
            .map(str::to_owned);
        // User intent only; Relay Core owns all execution policy decisions.
        let intent = relay_core::ExecutionPlanIntent {
            preferred_profile_id: Some(self.active_profile_id.clone()),
            preferred_model: one_run_model
                .or_else(|| self.active_model.as_option().map(str::to_owned)),
            preferred_reasoning_effort: one_run_effort.or(self.active_reasoning_effort.as_option()),
        };
        let mut environment = relay_core::ExecutionPlanEnvironment::default();
        if let Some(integration) = build_runtime_router()
            .integration(self.active_profile_id.split('/').next().unwrap_or_default())
        {
            environment.effective_profile =
                integration.resolve_profile(&self.active_profile_id).ok();
            if intent.preferred_model.is_some() || intent.preferred_reasoning_effort.is_some() {
                environment.model_catalog = Some(integration.models().await);
            }
        }
        let plan = match scheduler
            .plan_execution(task.id, intent, &environment)
            .await
        {
            Ok(plan) => plan,
            Err(error) => {
                self.feedback = format!("Run unavailable: {error}");
                return;
            }
        };
        let policy = plan.policy;
        self.execution_views
            .entry(task.id)
            .or_default()
            .frozen_profile = Some(policy.runtime_selection.profile_id.clone());
        if let Some(view) = self.execution_views.get_mut(&task.id) {
            view.frozen_model = policy
                .runtime_selection
                .model
                .as_option()
                .map(str::to_owned);
            view.frozen_reasoning =
                policy
                    .runtime_selection
                    .reasoning_effort
                    .as_option()
                    .map(|e| {
                        match e {
                            relay_core::ReasoningEffort::Default => "default",
                            relay_core::ReasoningEffort::Low => "low",
                            relay_core::ReasoningEffort::Medium => "medium",
                            relay_core::ReasoningEffort::High => "high",
                            relay_core::ReasoningEffort::XHigh => "xhigh",
                        }
                        .to_owned()
                    });
        }
        match scheduler.submit_with_policy(task.id, policy).await {
            Ok(()) => {
                self.one_run_model_override = None;
                self.one_run_reasoning_override = None;
                self.execution_views
                    .entry(task.id)
                    .or_default()
                    .scheduler_status = Some(SchedulerTaskStatus::QueuedReady);
                self.feedback = "Run approved. Queued for execution.".to_owned();
                self.screen = Screen::Execution;
                self.ensure_scheduler_driver(scheduler).await;
            }
            Err(error) => self.feedback = format!("Run was not queued: {error}"),
        }
    }

    pub(crate) async fn confirm_cancel(&mut self) {
        let Some(task_id) = self.cancel_confirmation.take() else {
            return;
        };
        let Some(scheduler) = self.scheduler.clone() else {
            return;
        };
        match scheduler.cancel(task_id).await {
            Ok(()) => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .cancellation_requested = true;
                self.feedback = "Cancellation requested...".to_owned();
            }
            Err(error) => self.feedback = format!("Cancellation request failed: {error}"),
        }
    }

    pub(crate) async fn refresh_durable_tasks(&mut self) {
        if let Some(store) = &self.store {
            match list_stored_tasks(store).await {
                Ok(tasks) => {
                    self.durable_tasks = tasks.clone();
                    self.execution_records.clear();
                    for task in tasks {
                        if let Ok(records) =
                            store.list_executions_for_task(&task.id.to_string()).await
                        {
                            if let Some(record) = records
                                .into_iter()
                                .max_by(|a, b| a.started_at.cmp(&b.started_at))
                            {
                                self.execution_records.insert(task.id, record);
                            }
                        }
                    }
                }
                Err(error) => {
                    self.feedback = format!("Could not refresh durable task state: {error}")
                }
            }
        }
    }

    pub(crate) fn execution_profile(&self, task_id: TaskId) -> Option<String> {
        self.execution_views
            .get(&task_id)
            .and_then(|v| v.frozen_profile.clone())
            .or_else(|| {
                self.execution_records
                    .get(&task_id)
                    .map(|r| r.runtime_profile.clone())
            })
    }

    pub(crate) fn execution_model(&self, task_id: TaskId) -> Option<String> {
        self.execution_views
            .get(&task_id)
            .and_then(|v| v.frozen_model.clone())
            .or_else(|| {
                self.execution_records
                    .get(&task_id)
                    .and_then(|r| r.runtime_metadata_json.as_deref())
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                    .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(str::to_owned))
            })
    }

    pub(crate) fn execution_reasoning(&self, task_id: TaskId) -> Option<String> {
        self.execution_views
            .get(&task_id)
            .and_then(|v| v.frozen_reasoning.clone())
            .or_else(|| {
                self.execution_records
                    .get(&task_id)
                    .and_then(|r| r.runtime_metadata_json.as_deref())
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                    .and_then(|v| {
                        v.get("reasoning_effort")
                            .and_then(|m| m.as_str())
                            .map(str::to_owned)
                    })
            })
    }

    pub(crate) fn clear_preflight_retry_state(&mut self) {
        for task_id in self.preflight_retry_pending.drain() {
            if let Some(view) = self.execution_views.get_mut(&task_id) {
                view.scheduler_status = None;
                view.cancellation_requested = false;
            }
        }
    }

    pub(crate) fn rebind_scheduler_if_idle(&mut self) {
        if self.scheduler_driver.is_some() || !self.scheduler_snapshot.tasks.is_empty() {
            self.scheduler_rebind_pending = true;
            return;
        }
        let Some(store) = &self.store else {
            return;
        };
        let scheduler = Arc::new(ExecutionScheduler::new(
            self.relay.clone(),
            store.clone(),
            Arc::new({
                let mut router = build_runtime_router();
                router.set_default_profile(self.active_profile_id.clone());
                router
            }),
            self.execution_limits.clone(),
        ));
        self.scheduler_events = Some(scheduler.subscribe());
        self.scheduler = Some(scheduler);
        self.scheduler_snapshot = SchedulerSnapshot { tasks: Vec::new() };
        self.scheduler_rebind_pending = false;
    }

    pub(crate) fn settings_field_value(&self, field: SettingsField) -> String {
        let limits = &self.execution_limits;
        match field {
            SettingsField::MaxParallel => limits.max_parallel.to_string(),
            SettingsField::PerProjectMaxParallel => limits.per_project_max_parallel.to_string(),
            SettingsField::MaxRepairAttempts => limits.max_repair_attempts.to_string(),
            SettingsField::MaxDurationMinutes => (limits.max_duration.as_secs() / 60).to_string(),
        }
    }

    /// Starts editing the selected field. Read-only rows (safety policy,
    /// storage, runtime status) never enter edit mode. `initial_digit` seeds
    /// the buffer when the user starts editing by typing a digit directly.
    pub(crate) fn begin_settings_edit(&mut self, initial_digit: Option<char>) {
        let Some(field) = self.settings_form.selected.editable_field() else {
            return;
        };
        self.settings_form.editing = true;
        self.settings_form.error = None;
        self.settings_form.saved = false;
        self.settings_form.buffer = match initial_digit {
            Some(digit) => digit.to_string(),
            None => self.settings_field_value(field),
        };
    }

    pub(crate) fn cancel_settings_edit(&mut self) {
        self.settings_form.editing = false;
        self.settings_form.buffer.clear();
        self.settings_form.error = None;
    }

    /// Commits the edit buffer for the selected field: validates against
    /// relay-core's execution-limit rules, persists through the runtime
    /// settings TOML path, and only then adopts the new value in memory.
    pub(crate) fn commit_settings_edit(&mut self) {
        let Some(field) = self.settings_form.selected.editable_field() else {
            return;
        };
        let raw = self.settings_form.buffer.trim().to_owned();
        let value = match raw.parse::<u64>() {
            Ok(value) if value > 0 => value,
            _ => {
                self.settings_form.error =
                    Some(format!("{} must be a positive integer.", field.label()));
                return;
            }
        };

        let mut limits = self.execution_limits.clone();
        match field {
            SettingsField::MaxParallel => limits.max_parallel = value as usize,
            SettingsField::PerProjectMaxParallel => {
                limits.per_project_max_parallel = value as usize
            }
            SettingsField::MaxRepairAttempts => {
                if value > u32::MAX as u64 {
                    self.settings_form.error = Some("max_repair_attempts is too large.".to_owned());
                    return;
                }
                limits.max_repair_attempts = value as u32;
            }
            SettingsField::MaxDurationMinutes => {
                let max_minutes = crate::util::CODEX_MAX_DURATION_SECS / 60;
                if value * 60 > crate::util::CODEX_MAX_DURATION_SECS {
                    self.settings_form.error = Some(format!(
                        "max_duration must be between 1 and {max_minutes} minutes"
                    ));
                    return;
                }
                limits.max_duration = Duration::from_secs(value * 60);
            }
        }

        if let Err(error) = validate_execution_limits(&limits) {
            self.settings_form.error = Some(error);
            return;
        }
        if limits.per_project_max_parallel > limits.max_parallel {
            self.settings_form.error = Some(format!(
                "Per-project parallelism cannot exceed global parallelism ({}).",
                limits.max_parallel
            ));
            return;
        }

        self.runtime_settings.execution = runtime_settings::ExecutionSettings {
            max_parallel: Some(limits.max_parallel),
            per_project_max_parallel: Some(limits.per_project_max_parallel),
            max_repair_attempts: Some(limits.max_repair_attempts),
            max_duration_secs: Some(limits.max_duration.as_secs()),
        };
        match self
            .runtime_settings
            .save_to_path(&self.runtime_config_path)
        {
            Ok(()) => {
                self.execution_limits = limits;
                self.settings_form.editing = false;
                self.settings_form.buffer.clear();
                self.settings_form.error = None;
                self.settings_form.saved = true;
            }
            Err(error) => {
                self.settings_form.error = Some(format!("Save failed: {error}"));
            }
        }
    }

    pub(crate) async fn reap_finished_scheduler_driver(&mut self) {
        let Some(driver) = self.scheduler_driver.as_ref() else {
            return;
        };
        if !driver.is_finished() {
            return;
        }
        let driver = self.scheduler_driver.take().expect("checked above");
        match driver.await {
            Ok(Ok(reports)) => {
                for report in reports {
                    self.last_reports.insert(report.task_id, report);
                }
            }
            Ok(Err(error)) => self.feedback = format!("Execution driver failed: {error}"),
            Err(error) => self.feedback = format!("Execution driver stopped: {error}"),
        }
        self.refresh_durable_tasks().await;
    }

    pub(crate) async fn ensure_scheduler_driver(
        &mut self,
        scheduler: Arc<ExecutionScheduler<RuntimeRouter>>,
    ) {
        self.reap_finished_scheduler_driver().await;
        self.scheduler_snapshot = scheduler.snapshot().await;
        self.start_scheduler_driver_if_queued(scheduler);
    }

    pub(crate) fn start_scheduler_driver_if_queued(
        &mut self,
        scheduler: Arc<ExecutionScheduler<RuntimeRouter>>,
    ) {
        let has_queued_work = self.scheduler_snapshot.tasks.iter().any(|task| {
            matches!(
                task.status,
                SchedulerTaskStatus::QueuedReady
                    | SchedulerTaskStatus::QueuedGlobal
                    | SchedulerTaskStatus::QueuedProject
            )
        });
        if self.scheduler_driver.is_none() && !self.scheduler_rebind_pending && has_queued_work {
            self.scheduler_driver = Some(tokio::spawn(
                async move { scheduler.run_until_idle().await },
            ));
        }
    }

    pub(crate) async fn tick(&mut self) {
        self.poll_health_refresh().await;
        self.poll_preflight_refreshes().await;
        self.poll_repository_inspection_refresh().await;
        if self.screen == Screen::TaskReview
            && let Some(task_id) = self.selected_task
        {
            let needs_refresh = self
                .preflight_cache
                .get(&task_id)
                .map(|(_, refreshed_at)| refreshed_at.elapsed() > PREFLIGHT_CACHE_TTL)
                .unwrap_or(true);
            if needs_refresh {
                self.begin_preflight_refresh(task_id);
            }
        }
        self.reap_finished_scheduler_driver().await;
        let mut reload = false;
        let mut lagged = false;
        let mut drained = Vec::new();
        if let Some(events) = &mut self.scheduler_events {
            loop {
                match events.try_recv() {
                    Ok(event) => drained.push(event),
                    Err(broadcast::error::TryRecvError::Empty) => break,
                    Err(broadcast::error::TryRecvError::Lagged(_)) => {
                        lagged = true;
                        break;
                    }
                    Err(broadcast::error::TryRecvError::Closed) => {
                        self.feedback = "Execution event stream closed.".to_owned();
                        break;
                    }
                }
            }
        }
        for event in drained {
            reload |= self.apply_execution_event(event);
        }
        if let Some(scheduler) = &self.scheduler {
            self.scheduler_snapshot = scheduler.snapshot().await;
        }
        if lagged {
            self.feedback = "Execution event stream lagged; refreshing canonical state.".to_owned();
            reload = true;
        }
        if reload {
            self.refresh_durable_tasks().await;
            self.clear_preflight_retry_state();
        }
        if self.scheduler_rebind_pending
            && self.scheduler_driver.is_none()
            && self.scheduler_snapshot.tasks.is_empty()
        {
            self.rebind_scheduler_if_idle();
        }
        if let Some(scheduler) = self.scheduler.clone() {
            self.start_scheduler_driver_if_queued(scheduler);
        }
    }

    pub(crate) fn apply_execution_event(&mut self, event: CoreExecutionEvent) -> bool {
        match event {
            CoreExecutionEvent::ExecutionQueued { task_id, .. } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .scheduler_status = Some(SchedulerTaskStatus::QueuedReady);
            }
            CoreExecutionEvent::SchedulerStatusChanged { task_id, status } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .scheduler_status = Some(status);
            }
            CoreExecutionEvent::ExecutionStarted { task_id, .. } => {
                let view = self.execution_views.entry(task_id).or_default();
                view.started_at = Some(Instant::now());
                view.durable_state = Some(TaskState::Running);
            }
            CoreExecutionEvent::RuntimeOutput {
                task_id,
                stream,
                bytes,
                ..
            } => self.append_output(task_id, stream, &bytes),
            CoreExecutionEvent::RuntimeActivity {
                task_id, activity, ..
            } => {
                self.append_activity(task_id, &activity);
            }
            CoreExecutionEvent::PermissionRequested {
                task_id, request, ..
            } => {
                let view = self.execution_views.entry(task_id).or_default();
                view.scheduler_status = Some(SchedulerTaskStatus::WaitingPermission);
                self.feedback = format!("Permission requested: {:?}", request.kind);
                if !self
                    .pending_permissions
                    .iter()
                    .any(|(pending_task, pending_request)| {
                        pending_task == &task_id && pending_request.request_id == request.request_id
                    })
                {
                    self.pending_permissions.push((task_id, request));
                }
                return true;
            }
            CoreExecutionEvent::VerificationStarted { task_id, .. } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .durable_state = Some(TaskState::Verifying)
            }
            CoreExecutionEvent::RepairStarted {
                task_id, attempt, ..
            } => {
                let view = self.execution_views.entry(task_id).or_default();
                view.durable_state = Some(TaskState::Repairing);
                view.repair_attempt = attempt;
            }
            CoreExecutionEvent::ClarificationNeeded { task_id, .. } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .durable_state = Some(TaskState::NeedsClarification)
            }
            CoreExecutionEvent::DecisionNeeded { task_id, .. } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .durable_state = Some(TaskState::NeedsDecision)
            }
            CoreExecutionEvent::ExecutionPersistenceFailed { task_id, error, .. } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .persistence_error = Some(error);
                return true;
            }
            CoreExecutionEvent::ExecutionPreflightFailed { task_id, failure } => {
                self.feedback = format!("Execution preflight failed for {task_id}: {failure:?}");
                self.preflight_retry_pending.insert(task_id);
                return true;
            }
            CoreExecutionEvent::ExecutionFinished {
                task_id,
                final_state,
                ..
            } => {
                let view = self.execution_views.entry(task_id).or_default();
                view.cancellation_requested = false;
                view.durable_state = Some(final_state);
                return true;
            }
            CoreExecutionEvent::ExecutionCancelled { task_id, .. }
            | CoreExecutionEvent::TaskStateChanged { task_id, .. } => {
                self.execution_views
                    .entry(task_id)
                    .or_default()
                    .cancellation_requested = false;
                return true;
            }
            CoreExecutionEvent::ExecutionPreflightStarted { .. }
            | CoreExecutionEvent::RepairAttempted { .. } => {}
        }
        false
    }

    pub(crate) fn append_output(
        &mut self,
        task_id: TaskId,
        stream: RuntimeOutputStream,
        bytes: &[u8],
    ) {
        let view = self.execution_views.entry(task_id).or_default();
        let safe: String = String::from_utf8_lossy(bytes)
            .chars()
            .map(|character| {
                if character == '\n' || character == '\t' || !character.is_control() {
                    character
                } else {
                    '�'
                }
            })
            .collect();
        let line = format!(
            "[{}] {}",
            match stream {
                RuntimeOutputStream::Stdout => "stdout",
                RuntimeOutputStream::Stderr => "stderr",
            },
            safe
        );
        view.output_bytes += line.len();
        view.output.push_back(line);
        while view.output_bytes > UI_OUTPUT_BYTE_LIMIT {
            if let Some(dropped) = view.output.pop_front() {
                view.output_bytes = view.output_bytes.saturating_sub(dropped.len());
            } else {
                break;
            }
        }
        if self.execution_follow_tail {
            self.execution_scroll = u16::MAX;
        }
    }

    pub(crate) fn append_activity(&mut self, task_id: TaskId, activity: &RuntimeActivity) {
        let line = match activity {
            RuntimeActivity::SessionStarted => "session started".to_owned(),
            RuntimeActivity::Analyzing { description } => format!("analyzing: {description}"),
            RuntimeActivity::FileRead { path, lines_read } => format!(
                "read {}{}",
                path.display(),
                lines_read
                    .map(|lines| format!(" ({lines} lines)"))
                    .unwrap_or_default()
            ),
            RuntimeActivity::CommandStarted {
                logical_command,
                working_directory,
                ..
            } => format!(
                "$ {logical_command}{}",
                working_directory
                    .as_ref()
                    .map(|cwd| format!("  (cwd {})", self.concise_activity_path(task_id, cwd)))
                    .unwrap_or_default()
            ),
            RuntimeActivity::CommandProgress { progress_line, .. } => progress_line.clone(),
            RuntimeActivity::CommandCompleted {
                logical_command,
                success,
                exit_code,
                ..
            } => format!(
                "{} {logical_command}{}",
                if *success { "✓" } else { "×" },
                exit_code
                    .map(|code| format!(" (exit {code})"))
                    .unwrap_or_default()
            ),
            RuntimeActivity::FileChanged { path, change_kind } => {
                format!(
                    "{} {}",
                    change_kind.marker(),
                    self.concise_activity_path(task_id, path)
                )
            }
            RuntimeActivity::VerificationStarted { description } => {
                format!("verify: {description}")
            }
            RuntimeActivity::VerificationProgress { detail, .. } => detail.clone(),
            RuntimeActivity::VerificationCompleted {
                description,
                passed,
            } => format!("{} {description}", if *passed { "✓" } else { "×" }),
            RuntimeActivity::AgentMessage { message } => match message.format {
                relay_runtime::RuntimeTextFormat::Markdown => markdown::render(&message.content),
                relay_runtime::RuntimeTextFormat::Plain => message.content.clone(),
            },
            RuntimeActivity::Warning { message } => format!("warning: {message}"),
            RuntimeActivity::ProviderDiagnostic { message } => format!("diagnostic: {message}"),
        };
        self.append_output(
            task_id,
            RuntimeOutputStream::Stdout,
            format!("{line}\n").as_bytes(),
        );
    }

    pub(crate) fn concise_activity_path(&self, task_id: TaskId, path: &std::path::Path) -> String {
        let Some(task) = self.durable_tasks.iter().find(|task| task.id == task_id) else {
            return path.display().to_string();
        };
        let Ok(project) = self.relay.resolve_project(task.project_key.as_str()) else {
            return path.display().to_string();
        };
        path.strip_prefix(project.local_path())
            .unwrap_or(path)
            .display()
            .to_string()
    }

    pub(crate) fn begin_repository_inspection_refresh(&mut self) {
        if self.inspection_refresh.is_some() {
            return;
        }
        let projects: Vec<TrustedProject> = self
            .relay
            .list_projects()
            .map(TrustedProject::clone)
            .collect();
        self.inspection_refresh = Some(tokio::task::spawn_blocking(move || {
            projects
                .into_iter()
                .map(|project| {
                    let inspection = inspect_trusted_project_repository(&project);
                    (project.key().clone(), inspection)
                })
                .collect::<BTreeMap<_, _>>()
        }));
    }

    pub(crate) async fn poll_repository_inspection_refresh(&mut self) {
        let Some(handle) = &self.inspection_refresh else {
            return;
        };
        if !handle.is_finished() {
            return;
        }
        let handle = self
            .inspection_refresh
            .take()
            .expect("inspection handle exists");
        if let Ok(inspections) = handle.await {
            self.repository_inspections = inspections;
        }
    }

    /// Requests a repository inspection refresh; results land on a later tick.
    pub(crate) fn refresh_repository_inspections(&mut self) {
        self.begin_repository_inspection_refresh();
    }

    pub(crate) fn try_start_add_project(&mut self) {
        if self.registry_writable {
            self.start_add_project();
        } else {
            self.feedback =
                "Project registration is disabled until the config is fixed.".to_owned();
        }
    }

    pub(crate) fn start_add_project(&mut self) {
        self.screen = Screen::AddProject;
        self.add_project = AddProjectForm::default();
        self.discovered_project = None;
        self.add_field = AddProjectField::Key;
        self.feedback = "Enter trusted local project metadata.".to_owned();
    }

    pub(crate) fn start_new_task(&mut self) {
        self.screen = Screen::NewTask;
        self.task_candidate = None;
        self.feedback =
            "Choose a local ingress method. Import will not execute anything.".to_owned();
    }

    pub(crate) fn start_import_file(&mut self) {
        self.screen = Screen::ImportFile;
        self.import_file_path.clear();
        self.task_candidate = None;
        self.feedback = "Enter a .relay.json, .json, .relay.md, or .md path.".to_owned();
    }

    pub(crate) fn start_paste_task(&mut self) {
        self.screen = Screen::PasteTask;
        self.paste_text.clear();
        self.task_candidate = None;
        self.feedback = "Paste the task, then press Enter to validate.".to_owned();
    }

    pub(crate) fn submit_import_file(&mut self) {
        match self.relay.preview_local_file(self.import_file_path.trim()) {
            Ok(candidate) => {
                self.task_candidate = Some(candidate);
                self.feedback =
                    "Validated local task file. Confirm import to create a Relay task.".to_owned();
                self.screen = Screen::TaskPreview;
            }
            Err(error) => {
                self.task_candidate = None;
                self.feedback = error.to_string();
            }
        }
    }

    pub(crate) fn submit_paste_task(&mut self) {
        match self.relay.preview_local_paste(&self.paste_text) {
            Ok(candidate) => {
                self.task_candidate = Some(candidate);
                self.feedback =
                    "Validated pasted task. Confirm import to create a Relay task.".to_owned();
                self.screen = Screen::TaskPreview;
            }
            Err(error) => {
                self.task_candidate = None;
                self.feedback = error.to_string();
            }
        }
    }

    pub(crate) async fn confirm_task_import(&mut self) {
        let Some(store) = &self.store else {
            self.feedback = format!(
                "Task import is disabled until the database is fixed: {}",
                self.database_path.display()
            );
            return;
        };

        let Some(candidate) = self.task_candidate.take() else {
            self.feedback = "No validated task is ready to import.".to_owned();
            return;
        };

        match self
            .relay
            .confirm_task_import_with_store(store, candidate.clone())
            .await
        {
            Ok(stored) => {
                let task_id = stored.id.to_string();
                self.selected_task = Some(stored.id);
                self.durable_tasks.push(stored);
                self.feedback =
                    format!("Imported Relay task {task_id}. Execution has not started.");
                self.screen = Screen::TaskReview;
            }
            Err(error) => {
                self.task_candidate = Some(candidate);
                self.feedback = format!("Failed to persist Relay task: {error}");
            }
        }
    }

    pub(crate) fn cancel_task_import(&mut self) {
        self.task_candidate = None;
        self.feedback = "Task import canceled. No Relay task was created.".to_owned();
        self.screen = Screen::NewTask;
    }

    pub(crate) fn active_add_field_mut(&mut self) -> &mut String {
        match self.add_field {
            AddProjectField::Key => &mut self.add_project.key,
            AddProjectField::Path => &mut self.add_project.path,
            AddProjectField::ExpectedRemote => &mut self.add_project.expected_remote,
            AddProjectField::DefaultStack => &mut self.add_project.default_stack,
            AddProjectField::GitHubOwner => &mut self.add_project.github_owner,
            AddProjectField::GitHubRepo => &mut self.add_project.github_repo,
        }
    }

    pub(crate) fn submit_add_project(&mut self) -> Result<(), Box<dyn Error>> {
        if !self.registry_writable {
            self.feedback =
                "Project registration is disabled until the config is fixed.".to_owned();
            return Ok(());
        }

        let github = self.add_project.github_metadata()?;
        let request = RegisterProject {
            key: self.add_project.key.clone(),
            name: None,
            local_path: PathBuf::from(self.add_project.path.trim()),
            expected_remote: non_empty_field(&self.add_project.expected_remote),
            default_stack: non_empty_field(&self.add_project.default_stack),
            github,
        };
        let key = request.key.trim().to_owned();

        let mut registry = self.relay.project_registry().clone();
        let result = match registry.register(request) {
            Ok(_) => registry.save(&self.config_path),
            Err(error) => Err(error),
        };

        match result {
            Ok(()) => {
                self.relay = Relay::with_project_registry(registry);
                self.rebind_scheduler_if_idle();
                self.feedback = format!(
                    "Registered project '{key}' in {}",
                    self.config_path.display()
                );
                self.refresh_repository_inspections();
                self.screen = Screen::Projects;
            }
            Err(error) => {
                self.feedback = error.to_string();
            }
        }

        Ok(())
    }
}

/// Providers listed on the Runtimes screen, in display order.
pub(crate) const RUNTIME_PROVIDERS: [&str; 2] = ["codex", "antigravity"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Dashboard,
    Tasks,
    Projects,
    AddProject,
    NewTask,
    ImportFile,
    PasteTask,
    TaskPreview,
    TaskReview,
    Execution,
    Result,
    Runtimes,
    RuntimeDetail,
    Diff,
    Logs,
    Settings,
    Doctor,
    Updates,
}

/// Editable rows of the Settings form, in display order within EXECUTION.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsField {
    MaxParallel,
    PerProjectMaxParallel,
    MaxRepairAttempts,
    MaxDurationMinutes,
}

impl SettingsField {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::MaxParallel => "Global parallelism",
            Self::PerProjectMaxParallel => "Per-project parallelism",
            Self::MaxRepairAttempts => "Repair attempts",
            Self::MaxDurationMinutes => "Maximum execution (minutes)",
        }
    }
}

/// Every selectable row of the Settings screen, in display order across all
/// sections (EXECUTION / SAFETY / STORAGE / RUNTIME). Read-only policy and
/// storage rows are browsable but never enter edit mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsRow {
    Execution(SettingsField),
    PolicyApproval,
    PolicyCommit,
    PolicyPush,
    StorageConfigPath,
    StorageSqlitePath,
    RuntimeStatus,
}

pub(crate) const SETTINGS_ROWS: [SettingsRow; 10] = [
    SettingsRow::Execution(SettingsField::MaxParallel),
    SettingsRow::Execution(SettingsField::PerProjectMaxParallel),
    SettingsRow::Execution(SettingsField::MaxRepairAttempts),
    SettingsRow::Execution(SettingsField::MaxDurationMinutes),
    SettingsRow::PolicyApproval,
    SettingsRow::PolicyCommit,
    SettingsRow::PolicyPush,
    SettingsRow::StorageConfigPath,
    SettingsRow::StorageSqlitePath,
    SettingsRow::RuntimeStatus,
];

impl Default for SettingsRow {
    fn default() -> Self {
        Self::Execution(SettingsField::MaxParallel)
    }
}

impl SettingsRow {
    /// Section header this row renders under, in display order.
    pub(crate) fn section(self) -> &'static str {
        match self {
            Self::Execution(_) => "EXECUTION",
            Self::PolicyApproval | Self::PolicyCommit | Self::PolicyPush => "SAFETY",
            Self::StorageConfigPath | Self::StorageSqlitePath => "STORAGE",
            Self::RuntimeStatus => "RUNTIME",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Execution(field) => field.label(),
            Self::PolicyApproval => "Explicit Run approval",
            Self::PolicyCommit => "Automatic commit",
            Self::PolicyPush => "Automatic push",
            Self::StorageConfigPath => "Config",
            Self::StorageSqlitePath => "SQLite",
            Self::RuntimeStatus => "Provider / profile",
        }
    }

    /// The editable field behind a row, if any. Read-only rows return `None`
    /// so they can never enter edit mode.
    pub(crate) fn editable_field(self) -> Option<SettingsField> {
        match self {
            Self::Execution(field) => Some(field),
            _ => None,
        }
    }

    /// Fixed value shown for read-only rows; editable fields render through
    /// `AppState::settings_field_value` instead.
    pub(crate) fn readonly_value(self, state: &AppState) -> Option<String> {
        match self {
            Self::PolicyApproval => Some("On".to_owned()),
            Self::PolicyCommit => Some("Off".to_owned()),
            Self::PolicyPush => Some("Off".to_owned()),
            Self::StorageConfigPath => Some(state.config_path.display().to_string()),
            Self::StorageSqlitePath => Some(state.database_path.display().to_string()),
            Self::RuntimeStatus => Some(format!(
                "{} · {} · {}",
                state.active_runtime_provider,
                state.active_profile_id,
                state.runtime_health.display_status()
            )),
            Self::Execution(_) => None,
        }
    }

    /// True for safety-policy rows that are fixed by Relay's product
    /// invariants (explicit approval required; never auto-commit/push).
    /// They are browsable and explained but never editable.
    pub(crate) fn locked(self) -> bool {
        matches!(
            self,
            Self::PolicyApproval | Self::PolicyCommit | Self::PolicyPush
        )
    }

    /// Muted inline hint shown while a read-only row is selected.
    pub(crate) fn hint(self) -> Option<&'static str> {
        match self {
            Self::PolicyApproval => Some("fixed by policy (v0.1 requires explicit approval)"),
            Self::PolicyCommit => Some("fixed by policy (v0.1 never commits automatically)"),
            Self::PolicyPush => Some("fixed by policy (v0.1 never pushes automatically)"),
            Self::RuntimeStatus => Some("change via /runtimes"),
            _ => None,
        }
    }
}

/// Interactive state for the Settings screen. Values under edit live in
/// `buffer`; committed values live in `AppState::execution_limits`.
#[derive(Debug, Default)]
pub(crate) struct SettingsForm {
    pub(crate) selected: SettingsRow,
    pub(crate) editing: bool,
    pub(crate) buffer: String,
    pub(crate) error: Option<String>,
    pub(crate) saved: bool,
}

impl SettingsForm {
    pub(crate) fn select_previous(&mut self) {
        let index = SETTINGS_ROWS
            .iter()
            .position(|row| *row == self.selected)
            .unwrap_or(0);
        self.selected = SETTINGS_ROWS[(index + SETTINGS_ROWS.len() - 1) % SETTINGS_ROWS.len()];
        self.saved = false;
    }

    pub(crate) fn select_next(&mut self) {
        let index = SETTINGS_ROWS
            .iter()
            .position(|row| *row == self.selected)
            .unwrap_or(0);
        self.selected = SETTINGS_ROWS[(index + 1) % SETTINGS_ROWS.len()];
        self.saved = false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AddProjectField {
    Key,
    Path,
    ExpectedRemote,
    DefaultStack,
    GitHubOwner,
    GitHubRepo,
}

impl AddProjectField {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Key => Self::Path,
            Self::Path => Self::ExpectedRemote,
            Self::ExpectedRemote => Self::DefaultStack,
            Self::DefaultStack => Self::GitHubOwner,
            Self::GitHubOwner => Self::GitHubRepo,
            Self::GitHubRepo => Self::Key,
        }
    }

    pub(crate) fn previous(self) -> Self {
        match self {
            Self::Key => Self::GitHubRepo,
            Self::Path => Self::Key,
            Self::ExpectedRemote => Self::Path,
            Self::DefaultStack => Self::ExpectedRemote,
            Self::GitHubOwner => Self::DefaultStack,
            Self::GitHubRepo => Self::GitHubOwner,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AddProjectForm {
    pub(crate) key: String,
    pub(crate) path: String,
    pub(crate) expected_remote: String,
    pub(crate) default_stack: String,
    pub(crate) github_owner: String,
    pub(crate) github_repo: String,
}

impl AddProjectForm {
    fn github_metadata(&self) -> Result<Option<GitHubRepositoryMetadata>, Box<dyn Error>> {
        let owner = self.github_owner.trim();
        let repo = self.github_repo.trim();

        match (owner.is_empty(), repo.is_empty()) {
            (true, true) => Ok(None),
            (false, false) => Ok(Some(GitHubRepositoryMetadata {
                owner: owner.to_owned(),
                repo: repo.to_owned(),
                default_labels: Vec::new(),
            })),
            _ => Err("GitHub owner and repo must either both be set or both be blank".into()),
        }
    }
}

impl Default for AddProjectForm {
    fn default() -> Self {
        Self {
            key: String::new(),
            path: std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            expected_remote: String::new(),
            default_stack: "chatgpt-codex".to_owned(),
            github_owner: String::new(),
            github_repo: String::new(),
        }
    }
}

/// Validates candidate limits through relay-core's own rules so the form
/// surfaces exactly the engine's rejection reasons.
fn validate_execution_limits(limits: &ExecutionLimits) -> Result<(), String> {
    ExecutionLimits::new(
        limits.max_parallel,
        limits.per_project_max_parallel,
        limits.max_repair_attempts,
        limits.max_duration,
        limits.output_byte_limit,
    )
    .map(|_| ())
    .map_err(|error| match error {
        ExecutionSchedulerError::InvalidLimit { name } => format!("{name} must be positive"),
        other => other.to_string(),
    })
}

fn non_empty_field(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}
