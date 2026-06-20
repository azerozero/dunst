use super::*;

pub(super) fn unique_png_path(prefix: &str) -> PathBuf {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "{prefix}_{}_{}_{}.png",
        std::process::id(),
        nanos,
        n
    ))
}

#[cfg(target_os = "macos")]
pub(super) struct BorrowedHoverUiGuard {
    saved_cursor: Option<(f64, f64)>,
    previous_front_pid: Option<String>,
}

#[cfg(target_os = "macos")]
impl BorrowedHoverUiGuard {
    pub(super) fn start(target: &WindowRef, x: f64, y: f64) -> dunst_core::Result<Self> {
        let previous_front_pid = Engine::borrow_target_frontmost(target)?;
        std::thread::sleep(Duration::from_millis(120));
        let saved_cursor = dunst_platform::cursor_borrow_to(x, y)?;
        Ok(Self {
            saved_cursor: Some(saved_cursor),
            previous_front_pid,
        })
    }
}

#[cfg(target_os = "macos")]
impl Drop for BorrowedHoverUiGuard {
    fn drop(&mut self) {
        if let Some((x, y)) = self.saved_cursor.take() {
            let _ = dunst_platform::cursor_restore(x, y);
        }
        if let Some(pid) = self.previous_front_pid.take() {
            let _ = Engine::restore_frontmost_pid(&pid);
        }
    }
}

#[derive(Clone)]
pub(super) struct TimedCache<T> {
    pub(super) captured_at: Instant,
    pub(super) value: T,
}

impl<T: Clone> TimedCache<T> {
    pub(super) fn fresh(&self, ttl: Duration) -> Option<T> {
        (self.captured_at.elapsed() <= ttl).then(|| self.value.clone())
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct OcrCacheKey {
    window_id: u32,
    region: Option<(i64, i64, i64, i64)>,
    accurate: bool,
}

#[derive(Clone)]
pub(super) struct OcrCacheEntry {
    pub(super) key: OcrCacheKey,
    pub(super) hits: Vec<TextHit>,
}

pub(super) fn ocr_cache_key(window_id: u32, region: Option<Bbox>, accurate: bool) -> OcrCacheKey {
    OcrCacheKey {
        window_id,
        region: region.map(|b| {
            (
                b.x.round() as i64,
                b.y.round() as i64,
                b.w.round() as i64,
                b.h.round() as i64,
            )
        }),
        accurate,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct DesktopCacheKey {
    pub(super) all: bool,
}

#[derive(Clone)]
pub(super) struct DesktopCacheEntry {
    pub(super) key: DesktopCacheKey,
    pub(super) view: DesktopView,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct VisualProbeKey {
    pub(super) region: (i64, i64, i64, i64),
    pub(super) columns: usize,
    pub(super) rows: usize,
}

#[derive(Clone)]
pub(super) struct VisualProbeCacheEntry {
    pub(super) key: VisualProbeKey,
    pub(super) signature: Vec<u8>,
}

/// Standard base64 of `data` (for returning a screenshot PNG as MCP image data).
pub(super) fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
