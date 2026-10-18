//! Shared process-tree termination and liveness helpers.
//!
//! Provider processes spawn descendants (shells, build tools, grandchild
//! agents). Killing only the direct child leaks the rest of the tree, so every
//! local host terminates the whole tree by tree-kill on Windows or by
//! process-group kill on Unix.

use tokio::process::Child;
#[cfg(windows)]
use tokio::process::Command;

/// Terminates the whole process tree rooted at `child` and reaps it.
///
/// On Windows this runs `taskkill /T /F /PID <pid>`; on Unix the child is
/// expected to lead its own process group, so the group is signalled with
/// `kill(-pid, SIGKILL)`.
pub async fn terminate_process_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        if let Some(pid) = child.id() {
            terminate_pid_tree(pid).await;
            let _ = child.kill().await;
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(pid) = child.id() {
            terminate_pid_tree(pid).await;
        } else {
            let _ = child.kill().await;
        }
    }
    let _ = child.wait().await;
}

/// Terminates the process tree rooted at a bare `pid`, for callers that hold no
/// `Child` handle (for example an orphan observed after a daemon restart).
pub async fn terminate_pid_tree(pid: u32) {
    #[cfg(windows)]
    {
        let pid_arg = pid.to_string();
        let mut taskkill = Command::new("taskkill");
        taskkill
            .args(["/T", "/F", "/PID", pid_arg.as_str()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let _ = taskkill.status().await;
    }
    #[cfg(not(windows))]
    {
        unsafe {
            if libc::kill(-(pid as libc::pid_t), libc::SIGKILL) != 0 {
                // The PID may not be a process-group leader; fall back to
                // signalling the single process.
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
}

/// Best-effort liveness probe for a bare PID.
///
/// Used to detect provider processes that outlived the daemon that launched
/// them. A failed probe is reported as not-alive so callers never keep
/// authority over a process they cannot observe.
pub async fn process_is_alive(pid: u32) -> bool {
    probe_pid_alive(pid).await
}

#[cfg(windows)]
async fn probe_pid_alive(pid: u32) -> bool {
    let pid_arg = pid.to_string();
    let filter = format!("PID eq {pid_arg}");
    let output = Command::new("tasklist")
        .args(["/FI", filter.as_str(), "/NH", "/FO", "CSV"])
        .stdin(std::process::Stdio::null())
        .output()
        .await;

    match output {
        Ok(output) => String::from_utf8_lossy(&output.stdout).lines().any(|line| {
            line.split(',')
                .any(|field| field.trim().trim_matches('"') == pid_arg)
        }),
        Err(_) => false,
    }
}

#[cfg(not(windows))]
async fn probe_pid_alive(pid: u32) -> bool {
    // `kill(pid, 0)` delivers no signal; it only reports whether the process
    // exists. EPERM means it exists but is owned by another user.
    unsafe {
        if libc::kill(pid as libc::pid_t, 0) == 0 {
            true
        } else {
            std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
        }
    }
}
