//! Trusted one-shot process host used by Herdr panes.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::{io::AsyncReadExt, process::Command, task::JoinHandle};

use crate::{
    RuntimeProvisionRequest, environment::resolved_child_environment, herdr::TrustedToken,
    process_tree::terminate_process_tree,
};

const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleHostSpec {
    pub attempt_id: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub output_byte_limit: usize,
    pub result_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleHostResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub interrupted: bool,
}

#[derive(Debug, Error)]
pub enum RoleHostError {
    #[error("invalid role-host attempt ID: {0:?}")]
    InvalidAttemptId(String),
    #[error("invalid role-host specification: {0}")]
    InvalidSpec(String),
    #[error("role-host result already exists: {0}")]
    ResultAlreadyExists(PathBuf),
    #[error("role-host I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("role-host JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl RoleHostSpec {
    pub fn from_request(
        role_host_dir: &Path,
        request: &RuntimeProvisionRequest,
    ) -> Result<Self, RoleHostError> {
        validate_attempt_id(&request.role_attempt_id)?;
        let provider_executable = request.launch.executable.to_str().ok_or_else(|| {
            RoleHostError::InvalidSpec("provider executable path is not valid UTF-8".into())
        })?;
        let mut argv = Vec::with_capacity(request.launch.args.len() + 1);
        argv.push(provider_executable.to_owned());
        argv.extend(request.launch.args.iter().cloned());
        Ok(Self {
            attempt_id: request.role_attempt_id.clone(),
            argv,
            cwd: request.trusted_cwd.clone(),
            env: resolved_child_environment(&request.environment),
            output_byte_limit: request.output_byte_limit,
            result_path: role_host_result_path(role_host_dir, &request.role_attempt_id)?,
        })
    }
}

pub fn validate_attempt_id(attempt_id: &str) -> Result<(), RoleHostError> {
    TrustedToken::new(attempt_id.to_owned())
        .map_err(|_| RoleHostError::InvalidAttemptId(attempt_id.to_owned()))?;
    // Attempt IDs also become Windows filenames, so reject path and stream separators.
    if attempt_id.contains('/')
        || attempt_id.contains('\\')
        || attempt_id.contains(':')
        || attempt_id == "."
        || attempt_id == ".."
    {
        return Err(RoleHostError::InvalidAttemptId(attempt_id.to_owned()));
    }
    Ok(())
}

pub fn role_host_spec_path(
    role_host_dir: &Path,
    attempt_id: &str,
) -> Result<PathBuf, RoleHostError> {
    validate_attempt_id(attempt_id)?;
    Ok(role_host_dir.join(format!("{attempt_id}.json")))
}

pub fn role_host_result_path(
    role_host_dir: &Path,
    attempt_id: &str,
) -> Result<PathBuf, RoleHostError> {
    validate_attempt_id(attempt_id)?;
    Ok(role_host_dir.join(format!("{attempt_id}.result.json")))
}

pub fn write_role_host_spec(
    role_host_dir: &Path,
    spec: &RoleHostSpec,
) -> Result<PathBuf, RoleHostError> {
    validate_attempt_id(&spec.attempt_id)?;
    let spec_path = role_host_spec_path(role_host_dir, &spec.attempt_id)?;
    let expected_result = role_host_result_path(role_host_dir, &spec.attempt_id)?;
    if spec.result_path != expected_result || spec.argv.is_empty() {
        return Err(RoleHostError::InvalidSpec(
            "attempt, argv, or result path does not match its role-host directory".into(),
        ));
    }
    fs::create_dir_all(role_host_dir)?;
    if expected_result.exists() {
        return Err(RoleHostError::ResultAlreadyExists(expected_result));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&spec_path)?;
    serde_json::to_writer(&mut file, spec)?;
    file.sync_all()?;
    Ok(spec_path)
}

pub async fn run_role_host(role_host_dir: &Path, attempt_id: &str) -> Result<i32, RoleHostError> {
    let spec_path = role_host_spec_path(role_host_dir, attempt_id)?;
    let spec: RoleHostSpec = serde_json::from_slice(&fs::read(spec_path)?)?;
    let expected_result = role_host_result_path(role_host_dir, attempt_id)?;
    if spec.attempt_id != attempt_id || spec.result_path != expected_result || spec.argv.is_empty()
    {
        return Err(RoleHostError::InvalidSpec(
            "attempt, argv, or result path does not match the requested attempt".into(),
        ));
    }
    if spec.result_path.exists() {
        return Err(RoleHostError::ResultAlreadyExists(spec.result_path));
    }

    let mut command = Command::new(&spec.argv[0]);
    command
        .args(&spec.argv[1..])
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(spec.env.clone())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            write_result_atomically(
                &spec.result_path,
                &RoleHostResult {
                    exit_code: None,
                    stdout: String::new(),
                    stderr: error.to_string(),
                    interrupted: false,
                },
            )?;
            return Ok(1);
        }
    };
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let captured_bytes = Arc::new(AtomicUsize::new(0));
    let stdout_reader = child.stdout.take().map(|pipe| {
        tokio::spawn(capture_pipe(
            pipe,
            Arc::clone(&stdout),
            Arc::clone(&captured_bytes),
            spec.output_byte_limit,
            false,
        ))
    });
    let stderr_reader = child.stderr.take().map(|pipe| {
        tokio::spawn(capture_pipe(
            pipe,
            Arc::clone(&stderr),
            Arc::clone(&captured_bytes),
            spec.output_byte_limit,
            true,
        ))
    });

    let (status, interrupted) = tokio::select! {
        status = child.wait() => (status, false),
        signal = tokio::signal::ctrl_c() => {
            if signal.is_ok() {
                terminate_process_tree(&mut child).await;
                (child.wait().await, true)
            } else {
                (child.wait().await, false)
            }
        }
    };
    drain_reader(stdout_reader).await;
    drain_reader(stderr_reader).await;

    let (exit_code, process_exit_code) = match status {
        Ok(status) => (status.code(), status.code().unwrap_or(1)),
        Err(error) => {
            let mut stderr = String::from_utf8_lossy(&stderr.lock().unwrap()).into_owned();
            if !stderr.is_empty() {
                stderr.push('\n');
            }
            stderr.push_str(&format!("failed to wait for provider process: {error}"));
            let result = RoleHostResult {
                exit_code: None,
                stdout: String::from_utf8_lossy(&stdout.lock().unwrap()).into_owned(),
                stderr,
                interrupted,
            };
            write_result_atomically(&spec.result_path, &result)?;
            return Ok(1);
        }
    };
    let result = RoleHostResult {
        exit_code,
        stdout: String::from_utf8_lossy(&stdout.lock().unwrap()).into_owned(),
        stderr: String::from_utf8_lossy(&stderr.lock().unwrap()).into_owned(),
        interrupted,
    };
    write_result_atomically(&spec.result_path, &result)?;
    Ok(if interrupted {
        if process_exit_code == 0 {
            1
        } else {
            process_exit_code
        }
    } else {
        process_exit_code
    })
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn attempt_ids_use_the_token_charset_and_stay_in_the_spec_directory() {
        for attempt in ["attempt-1", "wfatt-abc_1.2"] {
            assert!(role_host_spec_path(Path::new("state/role-host"), attempt).is_ok());
        }
        for attempt in [
            "",
            "a b",
            "a;b",
            "../escape",
            r"..\escape",
            "C:attempt",
            "$x",
        ] {
            assert!(role_host_spec_path(Path::new("state/role-host"), attempt).is_err());
        }
    }
}

async fn capture_pipe<R>(
    mut pipe: R,
    captured: Arc<Mutex<Vec<u8>>>,
    total: Arc<AtomicUsize>,
    limit: usize,
    is_stderr: bool,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut chunk = [0_u8; 4096];
    loop {
        let Ok(read) = pipe.read(&mut chunk).await else {
            break;
        };
        if read == 0 {
            break;
        }
        let mut writer = if is_stderr {
            io::stderr()
        } else {
            io::stdout()
        };
        let _ = writer.write_all(&chunk[..read]);
        let _ = writer.flush();
        let count = reserve_capture(&total, limit, read);
        if count > 0 {
            captured.lock().unwrap().extend_from_slice(&chunk[..count]);
        }
    }
}

fn reserve_capture(total: &AtomicUsize, limit: usize, requested: usize) -> usize {
    let mut current = total.load(Ordering::Relaxed);
    loop {
        let reserved = requested.min(limit.saturating_sub(current));
        if reserved == 0 {
            return 0;
        }
        match total.compare_exchange_weak(
            current,
            current + reserved,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return reserved,
            Err(actual) => current = actual,
        }
    }
}

async fn drain_reader(mut reader: Option<JoinHandle<()>>) {
    if let Some(mut reader) = reader.take()
        && tokio::time::timeout(OUTPUT_DRAIN_TIMEOUT, &mut reader)
            .await
            .is_err()
    {
        reader.abort();
    }
}

fn write_result_atomically(
    result_path: &Path,
    result: &RoleHostResult,
) -> Result<(), RoleHostError> {
    let mut temp_name = result_path
        .file_name()
        .ok_or_else(|| RoleHostError::InvalidSpec("result path has no file name".into()))?
        .to_os_string();
    temp_name.push(".tmp");
    let temp_path = result_path.with_file_name(temp_name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)?;
    serde_json::to_writer(&mut file, result)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temp_path, result_path)?;
    Ok(())
}

#[cfg(windows)]
#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "RELAY_ROLE_HOST_TEST_UNAPPROVED";

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("relay-role-host-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn publishes_child_exit_output_and_an_atomic_result() {
        let directory = temp_dir("exit");
        let path = role_host_result_path(&directory, "attempt-1").unwrap();
        let spec = RoleHostSpec {
            attempt_id: "attempt-1".into(),
            argv: vec![
                "powershell.exe".into(),
                "-NoProfile".into(),
                "-Command".into(),
                "Write-Output 'hi'; exit 3".into(),
            ],
            cwd: std::env::temp_dir(),
            env: Vec::new(),
            output_byte_limit: 3,
            result_path: path.clone(),
        };
        write_role_host_spec(&directory, &spec).unwrap();
        assert_eq!(run_role_host(&directory, "attempt-1").await.unwrap(), 3);
        assert!(path.is_file());
        assert!(!path.with_file_name("attempt-1.result.json.tmp").exists());
        let result: RoleHostResult = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(result.exit_code, Some(3));
        assert!(result.stdout.contains("hi"));
        assert!(result.stdout.len() <= 3);
    }

    #[tokio::test]
    async fn passes_metacharacters_verbatim_and_drops_unapproved_parent_environment() {
        struct RestoreEnvironment(Option<std::ffi::OsString>);
        impl Drop for RestoreEnvironment {
            fn drop(&mut self) {
                unsafe {
                    if let Some(value) = self.0.take() {
                        std::env::set_var(SECRET, value);
                    } else {
                        std::env::remove_var(SECRET);
                    }
                }
            }
        }
        let _restore = RestoreEnvironment(std::env::var_os(SECRET));
        unsafe {
            std::env::set_var(SECRET, "must-not-leak");
        }
        let directory = temp_dir("argv");
        fs::create_dir_all(&directory).unwrap();
        let script = directory.join("echo-arg.ps1");
        fs::write(&script, "[Console]::Write($args[0])").unwrap();
        let literal = "semi; amp& quote'\"";
        let path = role_host_result_path(&directory, "attempt-2").unwrap();
        let spec = RoleHostSpec {
            attempt_id: "attempt-2".into(),
            argv: vec![
                "powershell.exe".into(),
                "-NoProfile".into(),
                "-File".into(),
                script.to_string_lossy().into_owned(),
                literal.into(),
            ],
            cwd: directory.clone(),
            env: Vec::new(),
            output_byte_limit: 1024,
            result_path: path.clone(),
        };
        write_role_host_spec(&directory, &spec).unwrap();
        run_role_host(&directory, "attempt-2").await.unwrap();
        let result: RoleHostResult = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(result.stdout, literal);

        let env_path = role_host_result_path(&directory, "attempt-3").unwrap();
        let env_spec = RoleHostSpec {
            attempt_id: "attempt-3".into(),
            argv: vec![
                "powershell.exe".into(),
                "-NoProfile".into(),
                "-Command".into(),
                format!("[Console]::Write($env:{SECRET})"),
            ],
            cwd: directory.clone(),
            env: Vec::new(),
            output_byte_limit: 1024,
            result_path: env_path.clone(),
        };
        write_role_host_spec(&directory, &env_spec).unwrap();
        run_role_host(&directory, "attempt-3").await.unwrap();
        let result: RoleHostResult = serde_json::from_slice(&fs::read(env_path).unwrap()).unwrap();
        assert_eq!(result.stdout, "");
    }
}
