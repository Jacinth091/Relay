//! In-memory Herdr operations with Herdr-shaped, server-assigned identifiers.

use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use super::{
    HerdrCliPaneInfo, HerdrClientError, HerdrOps, HerdrPaneProcess, HerdrProcessInfo,
    HerdrSplitDirection, HerdrTab, HerdrTabCreation, HerdrVersion, HerdrWorkspace,
    HerdrWorkspaceCreation, TrustedToken,
};

#[derive(Default)]
struct FakeHerdrState {
    next_workspace: u32,
    next_tab: HashMap<String, u32>,
    next_pane: HashMap<String, u32>,
    workspaces: BTreeMap<String, HerdrWorkspace>,
    tabs: BTreeMap<String, HerdrTab>,
    panes: BTreeMap<String, HerdrCliPaneInfo>,
    pane_text: HashMap<String, String>,
    process_info: HashMap<String, HerdrProcessInfo>,
    pane_run_trusted_calls: Vec<(String, Vec<TrustedToken>)>,
    pane_send_keys_calls: Vec<(String, Vec<String>)>,
    version: Option<HerdrVersion>,
    protocol: Option<u32>,
}

/// Deterministic in-memory implementation of the typed Herdr operations.
#[derive(Default)]
pub struct FakeHerdr {
    state: Mutex<FakeHerdrState>,
    server_down: AtomicBool,
    operation_count: std::sync::atomic::AtomicUsize,
}

impl FakeHerdr {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_server_down(&self, down: bool) {
        self.server_down.store(down, Ordering::SeqCst);
    }

    pub fn set_version(&self, version: HerdrVersion) {
        self.state.lock().expect("FakeHerdr mutex poisoned").version = Some(version);
    }

    pub fn set_protocol(&self, protocol: u32) {
        self.state
            .lock()
            .expect("FakeHerdr mutex poisoned")
            .protocol = Some(protocol);
    }

    pub fn operation_count(&self) -> usize {
        self.operation_count.load(Ordering::SeqCst)
    }

    pub fn set_pane_read_text(&self, pane: &str, text: impl Into<String>) {
        self.state
            .lock()
            .expect("FakeHerdr mutex poisoned")
            .pane_text
            .insert(pane.to_owned(), text.into());
    }

    pub fn set_pane_process_info(&self, pane: &str, info: HerdrProcessInfo) {
        self.state
            .lock()
            .expect("FakeHerdr mutex poisoned")
            .process_info
            .insert(pane.to_owned(), info);
    }

    pub fn pane_run_trusted_calls(&self) -> Vec<(String, Vec<TrustedToken>)> {
        self.state
            .lock()
            .expect("FakeHerdr mutex poisoned")
            .pane_run_trusted_calls
            .clone()
    }

    pub fn pane_send_keys_calls(&self) -> Vec<(String, Vec<String>)> {
        self.state
            .lock()
            .expect("FakeHerdr mutex poisoned")
            .pane_send_keys_calls
            .clone()
    }

    fn check_server(&self) -> Result<(), HerdrClientError> {
        self.operation_count.fetch_add(1, Ordering::SeqCst);
        if self.server_down.load(Ordering::SeqCst) {
            Err(HerdrClientError::Transport(
                "FakeHerdr server is unavailable".into(),
            ))
        } else {
            Ok(())
        }
    }
}

fn new_pane(workspace: &str, tab: &str, pane_number: u32, cwd: Option<String>) -> HerdrCliPaneInfo {
    HerdrCliPaneInfo {
        pane_id: format!("{workspace}:p{pane_number}"),
        terminal_id: format!("term-{workspace}-p{pane_number}"),
        workspace_id: workspace.to_owned(),
        tab_id: tab.to_owned(),
        focused: false,
        agent_status: "unknown".into(),
        revision: 0,
        cwd,
        foreground_cwd: None,
        label: None,
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent: None,
        display_agent: None,
        state_labels: HashMap::new(),
        tokens: HashMap::new(),
    }
}

fn resource_not_found(kind: &str, id: &str) -> HerdrClientError {
    HerdrClientError::ResourceNotFound(format!("{kind} {id} not found"))
}

impl HerdrOps for FakeHerdr {
    async fn version(&self) -> Result<HerdrVersion, HerdrClientError> {
        self.check_server()?;
        Ok(self
            .state
            .lock()
            .expect("FakeHerdr mutex poisoned")
            .version
            .clone()
            .unwrap_or(HerdrVersion {
                major: 0,
                minor: 9,
                patch: 5,
                pre_release: None,
                build: None,
            }))
    }

    async fn protocol(&self) -> Result<u32, HerdrClientError> {
        self.check_server()?;
        Ok(self
            .state
            .lock()
            .expect("FakeHerdr mutex poisoned")
            .protocol
            .unwrap_or(22))
    }

    async fn workspace_create(
        &self,
        cwd: &Path,
        label: &str,
    ) -> Result<HerdrWorkspaceCreation, HerdrClientError> {
        self.check_server()?;
        let mut state = self.state.lock().expect("FakeHerdr mutex poisoned");
        state.next_workspace += 1;
        let workspace_id = format!("w{}", state.next_workspace);
        let tab_id = format!("{workspace_id}:t1");
        let pane_id = format!("{workspace_id}:p1");
        let workspace = HerdrWorkspace {
            workspace_id: workspace_id.clone(),
            number: state.next_workspace,
            label: label.to_owned(),
            focused: false,
            pane_count: 1,
            tab_count: 1,
            active_tab_id: tab_id.clone(),
            agent_status: "unknown".into(),
            tokens: HashMap::new(),
        };
        let tab = HerdrTab {
            tab_id: tab_id.clone(),
            workspace_id: workspace_id.clone(),
            number: 1,
            label: "1".into(),
            focused: false,
            pane_count: 1,
            agent_status: "unknown".into(),
        };
        let pane = new_pane(
            &workspace_id,
            &tab_id,
            1,
            Some(cwd.to_string_lossy().into_owned()),
        );
        state.next_tab.insert(workspace_id.clone(), 2);
        state.next_pane.insert(workspace_id.clone(), 2);
        state.workspaces.insert(workspace_id, workspace.clone());
        state.tabs.insert(tab_id, tab.clone());
        state.panes.insert(pane_id, pane.clone());
        Ok(HerdrWorkspaceCreation {
            workspace,
            tab,
            root_pane: pane,
        })
    }

    async fn workspace_get(&self, id: &str) -> Result<HerdrWorkspace, HerdrClientError> {
        self.check_server()?;
        self.state
            .lock()
            .expect("FakeHerdr mutex poisoned")
            .workspaces
            .get(id)
            .cloned()
            .ok_or_else(|| resource_not_found("workspace", id))
    }

    async fn workspace_list(&self) -> Result<Vec<HerdrWorkspace>, HerdrClientError> {
        self.check_server()?;
        Ok(self
            .state
            .lock()
            .expect("FakeHerdr mutex poisoned")
            .workspaces
            .values()
            .cloned()
            .collect())
    }

    async fn tab_create(
        &self,
        workspace: &str,
        cwd: &Path,
        label: &str,
    ) -> Result<HerdrTabCreation, HerdrClientError> {
        self.check_server()?;
        let mut state = self.state.lock().expect("FakeHerdr mutex poisoned");
        if !state.workspaces.contains_key(workspace) {
            return Err(resource_not_found("workspace", workspace));
        }
        let number = state.next_tab.entry(workspace.to_owned()).or_insert(2);
        let tab_number = *number;
        *number += 1;
        let pane_number = state.next_pane.entry(workspace.to_owned()).or_insert(2);
        let root_pane_number = *pane_number;
        *pane_number += 1;

        let tab_id = format!("{workspace}:t{tab_number}");
        let pane_id = format!("{workspace}:p{root_pane_number}");
        let tab = HerdrTab {
            tab_id: tab_id.clone(),
            workspace_id: workspace.to_owned(),
            number: tab_number,
            label: label.to_owned(),
            focused: false,
            pane_count: 1,
            agent_status: "unknown".into(),
        };
        let pane = new_pane(
            workspace,
            &tab_id,
            root_pane_number,
            Some(cwd.to_string_lossy().into_owned()),
        );
        state.tabs.insert(tab_id, tab.clone());
        state.panes.insert(pane_id, pane.clone());
        if let Some(info) = state.workspaces.get_mut(workspace) {
            info.tab_count += 1;
            info.pane_count += 1;
        }
        Ok(HerdrTabCreation {
            tab,
            root_pane: pane,
        })
    }

    async fn tab_list(&self, workspace: &str) -> Result<Vec<HerdrTab>, HerdrClientError> {
        self.check_server()?;
        let state = self.state.lock().expect("FakeHerdr mutex poisoned");
        if !state.workspaces.contains_key(workspace) {
            return Err(resource_not_found("workspace", workspace));
        }
        Ok(state
            .tabs
            .values()
            .filter(|tab| tab.workspace_id == workspace)
            .cloned()
            .collect())
    }

    async fn tab_close(&self, tab: &str) -> Result<(), HerdrClientError> {
        self.check_server()?;
        let mut state = self.state.lock().expect("FakeHerdr mutex poisoned");
        let closed = state
            .tabs
            .remove(tab)
            .ok_or_else(|| resource_not_found("tab", tab))?;
        let pane_ids = state
            .panes
            .values()
            .filter(|pane| pane.tab_id == tab)
            .map(|pane| pane.pane_id.clone())
            .collect::<Vec<_>>();
        for pane_id in pane_ids {
            state.panes.remove(&pane_id);
            state.pane_text.remove(&pane_id);
            state.process_info.remove(&pane_id);
        }
        if let Some(workspace) = state.workspaces.get_mut(&closed.workspace_id) {
            workspace.tab_count = workspace.tab_count.saturating_sub(1);
            workspace.pane_count = workspace.pane_count.saturating_sub(closed.pane_count);
        }
        Ok(())
    }

    async fn pane_get(&self, id: &str) -> Result<Option<HerdrCliPaneInfo>, HerdrClientError> {
        self.check_server()?;
        Ok(self
            .state
            .lock()
            .expect("FakeHerdr mutex poisoned")
            .panes
            .get(id)
            .cloned())
    }

    async fn pane_list(&self, workspace: &str) -> Result<Vec<HerdrCliPaneInfo>, HerdrClientError> {
        self.check_server()?;
        let state = self.state.lock().expect("FakeHerdr mutex poisoned");
        if !state.workspaces.contains_key(workspace) {
            return Err(resource_not_found("workspace", workspace));
        }
        Ok(state
            .panes
            .values()
            .filter(|pane| pane.workspace_id == workspace)
            .cloned()
            .collect())
    }

    async fn pane_split(
        &self,
        pane: &str,
        _direction: HerdrSplitDirection,
        cwd: &Path,
    ) -> Result<HerdrCliPaneInfo, HerdrClientError> {
        self.check_server()?;
        let mut state = self.state.lock().expect("FakeHerdr mutex poisoned");
        let parent = state
            .panes
            .get(pane)
            .cloned()
            .ok_or_else(|| resource_not_found("pane", pane))?;
        let number = state
            .next_pane
            .entry(parent.workspace_id.clone())
            .or_insert(2);
        let pane_number = *number;
        *number += 1;
        let pane_id = format!("{}:p{pane_number}", parent.workspace_id);
        let new = new_pane(
            &parent.workspace_id,
            &parent.tab_id,
            pane_number,
            Some(cwd.to_string_lossy().into_owned()),
        );
        state.panes.insert(pane_id, new.clone());
        if let Some(info) = state.workspaces.get_mut(&parent.workspace_id) {
            info.pane_count += 1;
        }
        if let Some(info) = state.tabs.get_mut(&parent.tab_id) {
            info.pane_count += 1;
        }
        Ok(new)
    }

    async fn pane_read(&self, pane: &str, _lines: usize) -> Result<String, HerdrClientError> {
        self.check_server()?;
        let state = self.state.lock().expect("FakeHerdr mutex poisoned");
        if !state.panes.contains_key(pane) {
            return Err(resource_not_found("pane", pane));
        }
        Ok(state.pane_text.get(pane).cloned().unwrap_or_default())
    }

    async fn pane_process_info(&self, pane: &str) -> Result<HerdrProcessInfo, HerdrClientError> {
        self.check_server()?;
        let state = self.state.lock().expect("FakeHerdr mutex poisoned");
        if !state.panes.contains_key(pane) {
            return Err(resource_not_found("pane", pane));
        }
        Ok(state
            .process_info
            .get(pane)
            .cloned()
            .unwrap_or_else(|| HerdrProcessInfo {
                pane_id: pane.to_owned(),
                foreground_process_group_id: None,
                foreground_processes: vec![HerdrPaneProcess {
                    pid: 1,
                    name: "powershell.exe".into(),
                    argv: Some(vec!["powershell.exe".into()]),
                    argv0: Some("powershell.exe".into()),
                    cmdline: Some("powershell.exe".into()),
                    cwd: None,
                }],
                shell_pid: Some(1),
                tty: None,
            }))
    }

    async fn pane_send_keys(&self, pane: &str, keys: &[String]) -> Result<(), HerdrClientError> {
        self.check_server()?;
        let mut state = self.state.lock().expect("FakeHerdr mutex poisoned");
        if !state.panes.contains_key(pane) {
            return Err(resource_not_found("pane", pane));
        }
        state
            .pane_send_keys_calls
            .push((pane.to_owned(), keys.to_vec()));
        Ok(())
    }

    async fn pane_run_trusted(
        &self,
        pane: &str,
        command: &[TrustedToken],
    ) -> Result<(), HerdrClientError> {
        self.check_server()?;
        let mut state = self.state.lock().expect("FakeHerdr mutex poisoned");
        if !state.panes.contains_key(pane) {
            return Err(resource_not_found("pane", pane));
        }
        state
            .pane_run_trusted_calls
            .push((pane.to_owned(), command.to_vec()));
        Ok(())
    }

    async fn pane_close(&self, pane: &str) -> Result<(), HerdrClientError> {
        self.check_server()?;
        let mut state = self.state.lock().expect("FakeHerdr mutex poisoned");
        let removed = state
            .panes
            .remove(pane)
            .ok_or_else(|| resource_not_found("pane", pane))?;
        state.pane_text.remove(pane);
        state.process_info.remove(pane);
        if let Some(info) = state.workspaces.get_mut(&removed.workspace_id) {
            info.pane_count = info.pane_count.saturating_sub(1);
        }
        if let Some(info) = state.tabs.get_mut(&removed.tab_id) {
            info.pane_count = info.pane_count.saturating_sub(1);
        }
        Ok(())
    }

    async fn pane_report_metadata(
        &self,
        pane: &str,
        _source: &str,
        title: &str,
        tokens: &[(String, String)],
    ) -> Result<(), HerdrClientError> {
        self.check_server()?;
        let mut state = self.state.lock().expect("FakeHerdr mutex poisoned");
        let pane = state
            .panes
            .get_mut(pane)
            .ok_or_else(|| resource_not_found("pane", pane))?;
        pane.title = Some(title.to_owned());
        for (key, value) in tokens {
            pane.tokens.insert(key.clone(), value.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[tokio::test]
    async fn assigns_herdr_shaped_ids_and_close_removes_the_pane() {
        let fake = FakeHerdr::new();
        let first = fake
            .workspace_create(Path::new(r"C:\work\one"), "first")
            .await
            .unwrap();
        let second = fake
            .workspace_create(Path::new(r"C:\work\two"), "second")
            .await
            .unwrap();
        assert_eq!(first.workspace.workspace_id, "w1");
        assert_eq!(first.tab.tab_id, "w1:t1");
        assert_eq!(first.root_pane.pane_id, "w1:p1");
        assert_eq!(second.workspace.workspace_id, "w2");
        assert_eq!(second.root_pane.pane_id, "w2:p1");

        let tab = fake
            .tab_create("w1", Path::new(r"C:\work\one"), "second tab")
            .await
            .unwrap();
        assert_eq!(tab.tab.tab_id, "w1:t2");
        assert_eq!(tab.root_pane.pane_id, "w1:p2");
        let split = fake
            .pane_split(
                "w1:p1",
                HerdrSplitDirection::Down,
                Path::new(r"C:\work\one"),
            )
            .await
            .unwrap();
        assert_eq!(split.pane_id, "w1:p3");

        fake.pane_close("w1:p3").await.unwrap();
        assert_eq!(fake.pane_get("w1:p3").await.unwrap(), None);
        assert_eq!(
            fake.pane_get("w1:p1").await.unwrap().unwrap().pane_id,
            "w1:p1"
        );
    }

    #[tokio::test]
    async fn pane_text_and_process_info_are_scriptable() {
        let fake = FakeHerdr::new();
        fake.workspace_create(Path::new(r"C:\work\repo"), "repo")
            .await
            .unwrap();
        fake.set_pane_read_text("w1:p1", "scripted output\n");
        fake.set_pane_process_info(
            "w1:p1",
            HerdrProcessInfo {
                pane_id: "w1:p1".into(),
                foreground_process_group_id: Some(42),
                foreground_processes: vec![HerdrPaneProcess {
                    pid: 42,
                    name: "codex.exe".into(),
                    argv: Some(vec!["codex".into(), "exec".into()]),
                    argv0: Some("codex".into()),
                    cmdline: Some("codex exec".into()),
                    cwd: Some(r"C:\work\repo".into()),
                }],
                shell_pid: Some(1),
                tty: None,
            },
        );
        assert_eq!(
            fake.pane_read("w1:p1", 10).await.unwrap(),
            "scripted output\n"
        );
        assert_eq!(
            fake.pane_process_info("w1:p1")
                .await
                .unwrap()
                .foreground_processes[0]
                .pid,
            42
        );
    }

    #[tokio::test]
    async fn server_down_fails_every_operation_with_transport() {
        let fake = FakeHerdr::new();
        fake.workspace_create(Path::new(r"C:\work\repo"), "repo")
            .await
            .unwrap();
        fake.set_server_down(true);
        let pane = "w1:p1";
        let keys = vec!["enter".into()];
        let command = vec![TrustedToken::new("role-host").unwrap()];
        let tokens = vec![("task".into(), "T01".into())];
        let results = [
            fake.version().await.map(|_| ()),
            fake.protocol().await.map(|_| ()),
            fake.workspace_create(Path::new(r"C:\work"), "x")
                .await
                .map(|_| ()),
            fake.workspace_get("w1").await.map(|_| ()),
            fake.workspace_list().await.map(|_| ()),
            fake.tab_create("w1", Path::new(r"C:\work"), "x")
                .await
                .map(|_| ()),
            fake.tab_list("w1").await.map(|_| ()),
            fake.tab_close("w1:t1").await,
            fake.pane_get(pane).await.map(|_| ()),
            fake.pane_list("w1").await.map(|_| ()),
            fake.pane_split(pane, HerdrSplitDirection::Right, Path::new(r"C:\work"))
                .await
                .map(|_| ()),
            fake.pane_read(pane, 1).await.map(|_| ()),
            fake.pane_process_info(pane).await.map(|_| ()),
            fake.pane_send_keys(pane, &keys).await,
            fake.pane_run_trusted(pane, &command).await,
            fake.pane_close(pane).await,
            fake.pane_report_metadata(pane, "relay", "title", &tokens)
                .await,
        ];
        for result in results {
            assert!(matches!(result, Err(HerdrClientError::Transport(_))));
        }
    }
}
