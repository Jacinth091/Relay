//! Shared process-tree termination, liveness and identity helpers.
//!
//! Provider processes spawn descendants (shells, build tools, grandchild
//! agents). Killing only the direct child leaks the rest of the tree, so every
//! local host terminates the whole tree by tree-kill on Windows or by
//! process-group kill on Unix.
//!
//! Terminating a *bare* PID is only safe once the PID is proven to still belong
//! to the process Relay launched: operating systems recycle PIDs, so a PID
//! recorded before a daemon restart may now name an unrelated user program.
//! [`process_identity`] performs that proof and fails closed — an unproven
//! identity is never reported as "ours".

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
/// This answers only "is *something* running at this PID". It deliberately has
/// no opinion about *what* is running there, so it must never be used on its own
/// to authorize terminating a PID. Use [`process_identity`] for that.
pub async fn process_is_alive(pid: u32) -> bool {
    probe_pid_alive(pid).await
}

/// Clock slack applied when comparing a recorded launch time with the operating
/// system's process start time.
///
/// Launch timestamps are whole seconds, the OS start time is derived from a
/// different clock, and a process can start between Relay reading its own clock
/// and the OS stamping the child. A start time within this window is therefore
/// still accepted as our own process.
const CREATION_TIME_SLACK_SECS: u64 = 2;

/// Whether a live PID is provably the process Relay launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessIdentity {
    /// A process is alive and it started at (or before) the recorded launch.
    Same,
    /// A process is alive but it started after the recorded launch, so the PID
    /// has been recycled and now belongs to something else.
    Different,
    /// Nothing is alive at that PID.
    Dead,
    /// Identity could not be proven: the probe failed, the recorded timestamp is
    /// unusable, or the host cannot expose process start times. Never treat this
    /// as "ours".
    Unverifiable,
}

/// Proves whether the process currently at `pid` is the one Relay launched.
///
/// `created_at` is the epoch-seconds string Relay recorded on the
/// [`crate::RuntimeHandle`] at launch. A recycled PID always belongs to a
/// process created *after* that launch, so a start time later than
/// `created_at + slack` proves the PID is not ours.
///
/// Fails closed: every OS-call failure and every unusable `created_at` reports
/// [`ProcessIdentity::Unverifiable`], never [`ProcessIdentity::Same`].
pub async fn process_identity(pid: u32, created_at: &str) -> ProcessIdentity {
    if created_at.trim().parse::<u64>().is_err() {
        return ProcessIdentity::Unverifiable;
    }

    match probe_process_start_epoch_secs(pid).await {
        ProbeOutcome::Alive(start_epoch_secs) => {
            classify_process_identity(start_epoch_secs, created_at)
        }
        ProbeOutcome::Dead => ProcessIdentity::Dead,
        ProbeOutcome::Unverifiable => ProcessIdentity::Unverifiable,
    }
}

/// Pure creation-time comparison: does a process that started at
/// `process_start_epoch_secs` match the launch timestamp `created_at`?
///
/// An unparseable `created_at` cannot prove anything and is
/// [`ProcessIdentity::Unverifiable`].
fn classify_process_identity(process_start_epoch_secs: u64, created_at: &str) -> ProcessIdentity {
    let Ok(launched_at_epoch_secs) = created_at.trim().parse::<u64>() else {
        return ProcessIdentity::Unverifiable;
    };

    match launched_at_epoch_secs.checked_add(CREATION_TIME_SLACK_SECS) {
        Some(latest_acceptable_start) if process_start_epoch_secs <= latest_acceptable_start => {
            ProcessIdentity::Same
        }
        Some(_) => ProcessIdentity::Different,
        None => ProcessIdentity::Unverifiable,
    }
}

/// Raw result of probing a PID's operating-system start time.
enum ProbeOutcome {
    /// The process is alive; this is its start time in Unix epoch seconds.
    Alive(u64),
    /// No process exists at that PID.
    Dead,
    /// The probe could not establish either fact.
    Unverifiable,
}

#[cfg(windows)]
async fn probe_process_start_epoch_secs(pid: u32) -> ProbeOutcome {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, HANDLE, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // 100-nanosecond ticks between 1601-01-01 (the FILETIME epoch) and
    // 1970-01-01 (the Unix epoch).
    const FILETIME_UNIX_EPOCH_SECS: u64 = 11_644_473_600;
    const TICKS_PER_SECOND: u64 = 10_000_000;

    // SAFETY: `OpenProcess` only acquires a query handle for an existing PID; it
    // writes nothing to our memory, and the handle is closed before returning.
    let handle: HANDLE = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // A missing PID is reported as ERROR_INVALID_PARAMETER; anything else
        // (access denied, transient failure) leaves identity unproven.
        return match std::io::Error::last_os_error().raw_os_error() {
            Some(code) if code == ERROR_INVALID_PARAMETER as i32 => ProbeOutcome::Dead,
            _ => ProbeOutcome::Unverifiable,
        };
    }

    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut kernel = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut user = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };

    // SAFETY: `handle` is a live query handle and all four out-parameters point
    // at live locals for the duration of the call.
    let queried =
        unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };

    let mut exit_code = 0;
    // SAFETY: `handle` is still the live query handle and `exit_code` is a
    // writable local for the duration of the call.
    let queried_exit_code = unsafe { GetExitCodeProcess(handle, &mut exit_code) };

    // SAFETY: `handle` came from `OpenProcess` above and is not used again.
    let closed = unsafe { CloseHandle(handle) };

    if queried == 0 || queried_exit_code == 0 || closed == 0 {
        return ProbeOutcome::Unverifiable;
    }
    if exit_code != STILL_ACTIVE as u32 {
        return ProbeOutcome::Dead;
    }

    let ticks = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
    match (ticks / TICKS_PER_SECOND).checked_sub(FILETIME_UNIX_EPOCH_SECS) {
        Some(epoch_secs) => ProbeOutcome::Alive(epoch_secs),
        None => ProbeOutcome::Unverifiable,
    }
}

#[cfg(not(windows))]
async fn probe_process_start_epoch_secs(pid: u32) -> ProbeOutcome {
    // `/proc/<pid>/stat` is the only portable-enough source of a process start
    // time on the Unix-like targets Relay runs on. Where `/proc` is absent,
    // identity stays unverifiable rather than being guessed.
    let Some(boot_time_epoch_secs) = read_boot_time_epoch_secs() else {
        return ProbeOutcome::Unverifiable;
    };

    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ProbeOutcome::Dead;
        }
        Err(_) => return ProbeOutcome::Unverifiable,
    };

    // Field 2 (`comm`) is wrapped in parentheses and may contain spaces, so cut
    // at the last ')' first; everything after it is field 3 onwards.
    let Some((_, fields_from_state)) = stat.rsplit_once(')') else {
        return ProbeOutcome::Unverifiable;
    };
    // `starttime` is field 22, i.e. the 20th token counting from field 3, and is
    // measured in clock ticks since boot.
    let Some(starttime_ticks) = fields_from_state
        .split_whitespace()
        .nth(19)
        .and_then(|raw| raw.parse::<u64>().ok())
    else {
        return ProbeOutcome::Unverifiable;
    };

    let ticks_per_second = clock_ticks_per_second();
    if ticks_per_second == 0 {
        return ProbeOutcome::Unverifiable;
    }

    match boot_time_epoch_secs.checked_add(starttime_ticks / ticks_per_second) {
        Some(start_epoch_secs) => ProbeOutcome::Alive(start_epoch_secs),
        None => ProbeOutcome::Unverifiable,
    }
}

/// Reads `/proc/stat`'s `btime` line (system boot time in Unix epoch seconds).
#[cfg(not(windows))]
fn read_boot_time_epoch_secs() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    stat.lines()
        .find_map(|line| line.strip_prefix("btime ")?.trim().parse::<u64>().ok())
}

/// `sysconf(_SC_CLK_TCK)`: clock ticks per second, as used by `/proc/<pid>/stat`.
#[cfg(not(windows))]
fn clock_ticks_per_second() -> u64 {
    // SAFETY: `sysconf` with a valid name only reads a constant; it has no
    // other side effects.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 { ticks as u64 } else { 0 }
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

#[cfg(test)]
mod tests {
    use super::{ProcessIdentity, classify_process_identity};

    const LAUNCHED_AT: &str = "1700000000";

    #[test]
    fn a_process_started_at_or_before_launch_is_the_same_process() {
        // Started well before Relay launched it.
        assert_eq!(
            classify_process_identity(1_699_999_000, LAUNCHED_AT),
            ProcessIdentity::Same
        );
        // Exactly at launch.
        assert_eq!(
            classify_process_identity(1_700_000_000, LAUNCHED_AT),
            ProcessIdentity::Same
        );
        // Within the clock-granularity slack.
        assert_eq!(
            classify_process_identity(1_700_000_002, LAUNCHED_AT),
            ProcessIdentity::Same
        );
    }

    #[test]
    fn a_process_started_after_launch_plus_slack_is_a_recycled_pid() {
        // One second past the slack window.
        assert_eq!(
            classify_process_identity(1_700_000_003, LAUNCHED_AT),
            ProcessIdentity::Different
        );
        // An hour after launch: the classic recycled PID.
        assert_eq!(
            classify_process_identity(1_700_003_600, LAUNCHED_AT),
            ProcessIdentity::Different
        );
    }

    #[test]
    fn an_unusable_launch_timestamp_never_proves_identity() {
        for created_at in ["unknown", "", "  ", "not-a-number", "-1", "12.5"] {
            assert_eq!(
                classify_process_identity(1_700_000_000, created_at),
                ProcessIdentity::Unverifiable,
                "created_at {created_at:?} must stay unverifiable"
            );
        }
        assert_eq!(
            classify_process_identity(1_700_000_000, "18446744073709551615"),
            ProcessIdentity::Unverifiable
        );
    }
}
