use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::fd::AsRawFd;

use dunst_core::SessionIdentity;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const DEFAULT_LEASE_TTL_MS: u64 = 30_000;
const DEFAULT_LOCK_WAIT_MS: u64 = 2_500;

pub(super) struct CoordinationGuard {
    lock: GlobalMutationLock,
    summary: CoordinationSummary,
}

impl CoordinationGuard {
    pub(super) fn acquire(
        session: &SessionIdentity,
        window_id: u32,
        tool_name: &str,
        args: &Value,
    ) -> Result<Self, CoordinationFailure> {
        let root = coordination_dir();
        ensure_dir(&root)?;
        let lock_path = root.join("raw-input.lock");
        let lease_path = root.join(format!("window-{window_id}.json"));
        let lease_ttl_ms =
            env_u64("DUNST_MCP_WINDOW_LEASE_TTL_MS", DEFAULT_LEASE_TTL_MS).clamp(1_000, 300_000);
        let lock_wait_ms =
            env_u64("DUNST_MCP_MUTATION_LOCK_WAIT_MS", DEFAULT_LOCK_WAIT_MS).clamp(0, 30_000);
        let requested_token = arg_string(args, "fencing_token");

        let lock = GlobalMutationLock::acquire(&lock_path, lock_wait_ms).map_err(|err| {
            CoordinationFailure::new(CoordinationSummary {
                mode: "single_writer".into(),
                tool: tool_name.into(),
                target_window_id: window_id,
                lock_path: path_string(&lock_path),
                lease_path: path_string(&lease_path),
                lease_ttl_ms,
                lock_wait_ms,
                waited_ms: err.waited_ms,
                owner: session.clone(),
                fencing_token: requested_token.clone(),
                lease_expires_at_ms: None,
                blocked_by: None,
                status: "lock_unavailable".into(),
                reason: Some(err.message),
            })
        })?;

        let waited_ms = lock.waited_ms;
        let now = dunst_core::now_ms();
        let existing = read_lease(&lease_path);
        if let Some(record) = existing.filter(|record| record.expires_at_ms > now) {
            if record.owner.session_id != session.session_id {
                return Err(CoordinationFailure::new(CoordinationSummary {
                    mode: "single_writer".into(),
                    tool: tool_name.into(),
                    target_window_id: window_id,
                    lock_path: path_string(&lock_path),
                    lease_path: path_string(&lease_path),
                    lease_ttl_ms,
                    lock_wait_ms,
                    waited_ms,
                    owner: session.clone(),
                    fencing_token: requested_token.clone(),
                    lease_expires_at_ms: Some(record.expires_at_ms),
                    blocked_by: Some(LeaseOwnerSummary::from_record(&record)),
                    status: "window_lease_blocked".into(),
                    reason: Some(format!(
                        "target window {window_id} is leased by session {} until {}",
                        record.owner.session_id, record.expires_at_ms
                    )),
                }));
            }
            if let Some(token) = requested_token.as_deref() {
                if token != record.fencing_token {
                    return Err(CoordinationFailure::new(CoordinationSummary {
                        mode: "single_writer".into(),
                        tool: tool_name.into(),
                        target_window_id: window_id,
                        lock_path: path_string(&lock_path),
                        lease_path: path_string(&lease_path),
                        lease_ttl_ms,
                        lock_wait_ms,
                        waited_ms,
                        owner: session.clone(),
                        fencing_token: requested_token.clone(),
                        lease_expires_at_ms: Some(record.expires_at_ms),
                        blocked_by: Some(LeaseOwnerSummary::from_record(&record)),
                        status: "fencing_token_mismatch".into(),
                        reason: Some(
                            "fencing_token does not match the active window lease; discard the stale plan and re-read get_hit_targets".into(),
                        ),
                    }));
                }
            }
            let renewed = LeaseRecord {
                expires_at_ms: now.saturating_add(lease_ttl_ms),
                updated_at_ms: now,
                tool: tool_name.into(),
                ..record
            };
            write_lease(&lease_path, &renewed)?;
            return Ok(Self {
                lock,
                summary: CoordinationSummary {
                    mode: "single_writer".into(),
                    tool: tool_name.into(),
                    target_window_id: window_id,
                    lock_path: path_string(&lock_path),
                    lease_path: path_string(&lease_path),
                    lease_ttl_ms,
                    lock_wait_ms,
                    waited_ms,
                    owner: session.clone(),
                    fencing_token: Some(renewed.fencing_token),
                    lease_expires_at_ms: Some(renewed.expires_at_ms),
                    blocked_by: None,
                    status: "lease_renewed".into(),
                    reason: None,
                },
            });
        }

        if requested_token.is_some() {
            return Err(CoordinationFailure::new(CoordinationSummary {
                mode: "single_writer".into(),
                tool: tool_name.into(),
                target_window_id: window_id,
                lock_path: path_string(&lock_path),
                lease_path: path_string(&lease_path),
                lease_ttl_ms,
                lock_wait_ms,
                waited_ms,
                owner: session.clone(),
                fencing_token: requested_token,
                lease_expires_at_ms: None,
                blocked_by: None,
                status: "fencing_token_expired".into(),
                reason: Some(
                    "fencing_token was supplied but no active matching window lease exists; re-read and retry without stale state".into(),
                ),
            }));
        }

        let fencing_token = format!(
            "lease-{}-{window_id}-{now}",
            token_safe_session_id(&session.session_id)
        );
        let record = LeaseRecord {
            window_id,
            owner: session.clone(),
            fencing_token: fencing_token.clone(),
            acquired_at_ms: now,
            updated_at_ms: now,
            expires_at_ms: now.saturating_add(lease_ttl_ms),
            tool: tool_name.into(),
        };
        write_lease(&lease_path, &record)?;

        Ok(Self {
            lock,
            summary: CoordinationSummary {
                mode: "single_writer".into(),
                tool: tool_name.into(),
                target_window_id: window_id,
                lock_path: path_string(&lock_path),
                lease_path: path_string(&lease_path),
                lease_ttl_ms,
                lock_wait_ms,
                waited_ms,
                owner: session.clone(),
                fencing_token: Some(fencing_token),
                lease_expires_at_ms: Some(record.expires_at_ms),
                blocked_by: None,
                status: "lease_acquired".into(),
                reason: None,
            },
        })
    }

    pub(super) fn summary_value(&self) -> Value {
        self.summary.to_value()
    }
}

impl Drop for CoordinationGuard {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}

#[derive(Debug)]
pub(super) struct CoordinationFailure {
    pub(super) message: String,
    pub(super) summary: Value,
}

impl CoordinationFailure {
    fn new(summary: CoordinationSummary) -> Self {
        let message = summary
            .reason
            .clone()
            .unwrap_or_else(|| "mutation coordination failed".into());
        Self {
            message,
            summary: summary.to_value(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct CoordinationSummary {
    mode: String,
    tool: String,
    target_window_id: u32,
    lock_path: String,
    lease_path: String,
    lease_ttl_ms: u64,
    lock_wait_ms: u64,
    waited_ms: u64,
    owner: SessionIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    fencing_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lease_expires_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocked_by: Option<LeaseOwnerSummary>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

impl CoordinationSummary {
    fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({ "status": "serialization_failed" }))
    }
}

#[derive(Clone, Debug, Serialize)]
struct LeaseOwnerSummary {
    session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_process: Option<String>,
    fencing_token: String,
    lease_expires_at_ms: u64,
}

impl LeaseOwnerSummary {
    fn from_record(record: &LeaseRecord) -> Self {
        Self {
            session_id: record.owner.session_id.clone(),
            client_name: record.owner.client_name.clone(),
            client_version: record.owner.client_version.clone(),
            agent_id: record.owner.agent_id.clone(),
            parent_pid: record.owner.parent_pid,
            parent_process: record.owner.parent_process.clone(),
            fencing_token: record.fencing_token.clone(),
            lease_expires_at_ms: record.expires_at_ms,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LeaseRecord {
    window_id: u32,
    owner: SessionIdentity,
    fencing_token: String,
    acquired_at_ms: u64,
    updated_at_ms: u64,
    expires_at_ms: u64,
    tool: String,
}

struct GlobalMutationLock {
    file: File,
    waited_ms: u64,
}

impl GlobalMutationLock {
    fn acquire(path: &Path, wait_ms: u64) -> Result<Self, LockFailure> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|err| LockFailure {
                waited_ms: 0,
                message: format!("open global mutation lock {}: {err}", path.display()),
            })?;
        let started = Instant::now();
        loop {
            match try_lock_exclusive(&file) {
                Ok(()) => {
                    return Ok(Self {
                        file,
                        waited_ms: started.elapsed().as_millis() as u64,
                    });
                }
                Err(err) if started.elapsed() >= Duration::from_millis(wait_ms) => {
                    return Err(LockFailure {
                        waited_ms: started.elapsed().as_millis() as u64,
                        message: format!(
                            "global mutating lock {} is busy after {} ms: {err}",
                            path.display(),
                            started.elapsed().as_millis()
                        ),
                    });
                }
                Err(_) => thread::sleep(Duration::from_millis(25)),
            }
        }
    }

    fn unlock(&self) -> io::Result<()> {
        unlock_file(&self.file)
    }
}

struct LockFailure {
    waited_ms: u64,
    message: String,
}

#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> io::Result<()> {
    // SAFETY: flock only reads the valid file descriptor borrowed from `file`.
    // The descriptor remains open for the lifetime of the guard.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn try_lock_exclusive(_file: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn unlock_file(file: &File) -> io::Result<()> {
    // SAFETY: flock only reads the valid file descriptor borrowed from `file`.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn unlock_file(_file: &File) -> io::Result<()> {
    Ok(())
}

fn read_lease(path: &Path) -> Option<LeaseRecord> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn write_lease(path: &Path, record: &LeaseRecord) -> Result<(), CoordinationFailure> {
    let parent = path.parent().unwrap_or_else(|| Path::new("/tmp"));
    ensure_dir(parent)?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("window-lease"),
        std::process::id()
    ));
    let payload = serde_json::to_vec_pretty(record).map_err(|err| {
        CoordinationFailure::new(CoordinationSummary {
            mode: "single_writer".into(),
            tool: record.tool.clone(),
            target_window_id: record.window_id,
            lock_path: String::new(),
            lease_path: path_string(path),
            lease_ttl_ms: 0,
            lock_wait_ms: 0,
            waited_ms: 0,
            owner: record.owner.clone(),
            fencing_token: Some(record.fencing_token.clone()),
            lease_expires_at_ms: Some(record.expires_at_ms),
            blocked_by: None,
            status: "lease_serialization_failed".into(),
            reason: Some(err.to_string()),
        })
    })?;
    fs::write(&tmp, payload).map_err(io_failure)?;
    fs::rename(&tmp, path).map_err(io_failure)?;
    Ok(())
}

fn ensure_dir(path: &Path) -> Result<(), CoordinationFailure> {
    fs::create_dir_all(path).map_err(io_failure)
}

fn io_failure(err: io::Error) -> CoordinationFailure {
    CoordinationFailure {
        message: err.to_string(),
        summary: json!({
            "status": "io_error",
            "reason": err.to_string()
        }),
    }
}

fn coordination_dir() -> PathBuf {
    std::env::var("DUNST_MCP_COORDINATION_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/dunst-mcp"))
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

fn arg_string(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn token_safe_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}
