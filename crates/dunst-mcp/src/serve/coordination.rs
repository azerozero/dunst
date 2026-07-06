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

/// The invariant lock/lease context for one [`CoordinationGuard::acquire`] call:
/// the paths and env-derived timings that every outcome summary repeats. Held by
/// reference so the outcome helpers ([`resolve_lease`] and friends) can rebuild a
/// [`CoordinationSummary`] scaffold via [`CoordinationSummary::base`].
struct LeaseContext<'a> {
    session: &'a SessionIdentity,
    window_id: u32,
    tool_name: &'a str,
    lock_path: PathBuf,
    lease_path: PathBuf,
    lease_ttl_ms: u64,
    lock_wait_ms: u64,
}

impl<'a> LeaseContext<'a> {
    /// Resolve the coordination directory (creating it), derive the lock/lease
    /// paths for `window_id`, and read the clamped lease-TTL / lock-wait from the
    /// environment.
    fn new(
        session: &'a SessionIdentity,
        window_id: u32,
        tool_name: &'a str,
    ) -> Result<Self, CoordinationFailure> {
        let root = coordination_dir();
        ensure_dir(&root)?;
        Ok(Self {
            session,
            window_id,
            tool_name,
            lock_path: root.join("raw-input.lock"),
            lease_path: root.join(format!("window-{window_id}.json")),
            lease_ttl_ms: env_u64("DUNST_MCP_WINDOW_LEASE_TTL_MS", DEFAULT_LEASE_TTL_MS)
                .clamp(1_000, 300_000),
            lock_wait_ms: env_u64("DUNST_MCP_MUTATION_LOCK_WAIT_MS", DEFAULT_LOCK_WAIT_MS)
                .clamp(0, 30_000),
        })
    }
}

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
        let ctx = LeaseContext::new(session, window_id, tool_name)?;
        let requested_token = arg_string(args, "fencing_token");

        let lock =
            GlobalMutationLock::acquire(&ctx.lock_path, ctx.lock_wait_ms).map_err(|err| {
                CoordinationFailure::new(CoordinationSummary {
                    fencing_token: requested_token.clone(),
                    status: "lock_unavailable".into(),
                    reason: Some(err.message),
                    ..CoordinationSummary::base(&ctx, err.waited_ms)
                })
            })?;

        // The global lock is now held; pick the lease outcome and pair it with
        // the guard so `Drop` releases the lock either way.
        let summary = resolve_lease(&ctx, lock.waited_ms, requested_token)?;
        Ok(Self { lock, summary })
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
    /// Common scaffold for every outcome of [`CoordinationGuard::acquire`]: fills
    /// the invariant lock/lease context from `ctx` and leaves the outcome-specific
    /// fields (`fencing_token`, `lease_expires_at_ms`, `blocked_by`, `status`,
    /// `reason`) at neutral defaults for the caller to override via `..`.
    fn base(ctx: &LeaseContext<'_>, waited_ms: u64) -> Self {
        Self {
            mode: "single_writer".into(),
            tool: ctx.tool_name.into(),
            target_window_id: ctx.window_id,
            lock_path: path_string(&ctx.lock_path),
            lease_path: path_string(&ctx.lease_path),
            lease_ttl_ms: ctx.lease_ttl_ms,
            lock_wait_ms: ctx.lock_wait_ms,
            waited_ms,
            owner: ctx.session.clone(),
            fencing_token: None,
            lease_expires_at_ms: None,
            blocked_by: None,
            status: String::new(),
            reason: None,
        }
    }

    fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({ "status": "serialization_failed" }))
    }
}

/// Decide the lease outcome once the global mutation lock is held: renew a
/// still-valid lease we own, or mint a fresh one. Returns the success summary,
/// or a [`CoordinationFailure`] for a foreign/mismatched/expired lease.
fn resolve_lease(
    ctx: &LeaseContext<'_>,
    waited_ms: u64,
    requested_token: Option<String>,
) -> Result<CoordinationSummary, CoordinationFailure> {
    let now = dunst_core::now_ms();
    match read_lease(&ctx.lease_path).filter(|record| record.expires_at_ms > now) {
        Some(record) => renew_existing_lease(ctx, waited_ms, requested_token, record, now),
        None => acquire_new_lease(ctx, waited_ms, requested_token, now),
    }
}

/// A still-valid lease exists: reject a foreign owner or a mismatched fencing
/// token, otherwise renew it and return the `lease_renewed` summary.
fn renew_existing_lease(
    ctx: &LeaseContext<'_>,
    waited_ms: u64,
    requested_token: Option<String>,
    record: LeaseRecord,
    now: u64,
) -> Result<CoordinationSummary, CoordinationFailure> {
    if record.owner.session_id != ctx.session.session_id {
        return Err(CoordinationFailure::new(CoordinationSummary {
            fencing_token: requested_token.clone(),
            lease_expires_at_ms: Some(record.expires_at_ms),
            blocked_by: Some(LeaseOwnerSummary::from_record(&record)),
            status: "window_lease_blocked".into(),
            reason: Some(format!(
                "target window {} is leased by session {} until {}",
                ctx.window_id, record.owner.session_id, record.expires_at_ms
            )),
            ..CoordinationSummary::base(ctx, waited_ms)
        }));
    }
    if let Some(token) = requested_token.as_deref() {
        if token != record.fencing_token {
            return Err(CoordinationFailure::new(CoordinationSummary {
                fencing_token: requested_token.clone(),
                lease_expires_at_ms: Some(record.expires_at_ms),
                blocked_by: Some(LeaseOwnerSummary::from_record(&record)),
                status: "fencing_token_mismatch".into(),
                reason: Some(
                    "fencing_token does not match the active window lease; discard the stale plan and re-read get_hit_targets".into(),
                ),
                ..CoordinationSummary::base(ctx, waited_ms)
            }));
        }
    }
    let renewed = LeaseRecord {
        expires_at_ms: now.saturating_add(ctx.lease_ttl_ms),
        updated_at_ms: now,
        tool: ctx.tool_name.into(),
        ..record
    };
    write_lease(&ctx.lease_path, &renewed)?;
    Ok(CoordinationSummary {
        fencing_token: Some(renewed.fencing_token),
        lease_expires_at_ms: Some(renewed.expires_at_ms),
        status: "lease_renewed".into(),
        ..CoordinationSummary::base(ctx, waited_ms)
    })
}

/// No usable existing lease: reject a supplied-but-stale fencing token, otherwise
/// mint a fresh lease and return the `lease_acquired` summary.
fn acquire_new_lease(
    ctx: &LeaseContext<'_>,
    waited_ms: u64,
    requested_token: Option<String>,
    now: u64,
) -> Result<CoordinationSummary, CoordinationFailure> {
    if requested_token.is_some() {
        return Err(CoordinationFailure::new(CoordinationSummary {
            fencing_token: requested_token,
            status: "fencing_token_expired".into(),
            reason: Some(
                "fencing_token was supplied but no active matching window lease exists; re-read and retry without stale state".into(),
            ),
            ..CoordinationSummary::base(ctx, waited_ms)
        }));
    }

    let fencing_token = format!(
        "lease-{}-{}-{now}",
        token_safe_session_id(&ctx.session.session_id),
        ctx.window_id
    );
    let record = LeaseRecord {
        window_id: ctx.window_id,
        owner: ctx.session.clone(),
        fencing_token: fencing_token.clone(),
        acquired_at_ms: now,
        updated_at_ms: now,
        expires_at_ms: now.saturating_add(ctx.lease_ttl_ms),
        tool: ctx.tool_name.into(),
    };
    write_lease(&ctx.lease_path, &record)?;
    Ok(CoordinationSummary {
        fencing_token: Some(fencing_token),
        lease_expires_at_ms: Some(record.expires_at_ms),
        status: "lease_acquired".into(),
        ..CoordinationSummary::base(ctx, waited_ms)
    })
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

// NOTE: Off-unix this is a no-op that always "succeeds", so the `single_writer`
// mutation guarantee is NOT enforced there — the lease file still serialises
// intent, but two processes could hold the lock simultaneously. Acceptable
// because the server targets macOS (unix); revisit before shipping a Windows build.
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
