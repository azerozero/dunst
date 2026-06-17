//! Core Graphics one-shot window capture (owner: Codex, P1a).

use std::{
    collections::HashSet,
    ffi::c_void,
    fmt,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use core_foundation::{
    base::TCFType,
    dictionary::{CFDictionaryGetValueIfPresent, CFDictionaryRef},
    number::{kCFNumberFloat64Type, kCFNumberSInt64Type, CFNumberGetValue, CFNumberRef},
    string::CFString,
};
use foreign_types::ForeignType;

use core_graphics::{
    data_provider::CGDataProvider,
    display::{CGDisplay, CGRectNull},
    geometry::{CGPoint, CGRect, CGSize},
    window::{
        copy_window_info, create_image, kCGNullWindowID, kCGWindowBounds,
        kCGWindowImageBestResolution, kCGWindowImageBoundsIgnoreFraming, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionAll,
        kCGWindowListOptionIncludingWindow, kCGWindowListOptionOnScreenOnly, kCGWindowName,
        kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
    },
};

use crate::CaptureGeometry;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_png_path(prefix: &str) -> PathBuf {
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

#[derive(Debug)]
pub enum CaptureError {
    WindowNotFound(u32),
    CoreGraphicsBounds(u32),
    CoreGraphicsImage(u32),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WindowNotFound(window_id) => write!(f, "window id {window_id} not found"),
            Self::CoreGraphicsBounds(window_id) => {
                write!(
                    f,
                    "CoreGraphics could not read bounds for window id {window_id}"
                )
            }
            Self::CoreGraphicsImage(window_id) => {
                write!(f, "CoreGraphics could not capture window id {window_id}")
            }
        }
    }
}

impl std::error::Error for CaptureError {}

pub struct CapturedWindow {
    pub image: core_graphics::image::CGImage,
    pub geometry: CaptureGeometry,
}

pub fn capture_window(window_id: u32) -> Result<CapturedWindow, CaptureError> {
    let bounds = cg_window_bounds(window_id)?;
    capture_cg_window_with_bounds(window_id, bounds)
}

/// Capture a rectangle of the **composited display** (what is actually on screen,
/// including GPU/WebGL overlays such as a chart crosshair that a window capture
/// misses) around a global screen-point rect. Returns the same [`CapturedWindow`]
/// shape, so the OCR + coord-mapping path is unchanged. App/browser agnostic —
/// it reads pixels off the screen, not a specific window's backing store.
pub fn capture_screen_rect(x: f64, y: f64, w: f64, h: f64) -> Result<CapturedWindow, CaptureError> {
    let display = display_containing(x + w / 2.0, y + h / 2.0);
    let db = display.bounds();
    // CGDisplayCreateImageForRect is inconsistent across display/backing setups:
    // some paths want display-local coordinates, others global desktop coordinates.
    // Try both first because it is cheap, then fall back to screencapture -R which
    // is slower but matches the visible composited screen and works on secondary displays.
    let local = CGRect::new(
        &CGPoint::new(x - db.origin.x, y - db.origin.y),
        &CGSize::new(w, h),
    );
    let image = display.image_for_rect(local).or_else(|| {
        display.image_for_rect(CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h)))
    });
    if let Some(image) = image {
        return Ok(captured_from_rect_image(image, x, y, w, h));
    }
    capture_screen_rect_via_screencapture(x, y, w, h)
}

fn captured_from_rect_image(
    image: core_graphics::image::CGImage,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> CapturedWindow {
    let image_size_px = (image.width() as f64, image.height() as f64);
    let backing_scale = if w > 0.0 && h > 0.0 {
        ((image_size_px.0 / w) + (image_size_px.1 / h)) / 2.0
    } else {
        1.0
    };
    let global = CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h));
    CapturedWindow {
        image,
        geometry: geometry_from_rect(global, image_size_px, backing_scale),
    }
}

fn capture_screen_rect_via_screencapture(
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<CapturedWindow, CaptureError> {
    let path = unique_png_path("dunst_rect");
    let rect = format!(
        "{},{},{},{}",
        x.round() as i64,
        y.round() as i64,
        w.ceil().max(1.0) as i64,
        h.ceil().max(1.0) as i64
    );
    let ok = std::process::Command::new("/usr/sbin/screencapture")
        .args(["-x", "-R", &rect])
        .arg(&path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return Err(CaptureError::CoreGraphicsImage(0));
    }
    let image = load_png_cg(&path.to_string_lossy());
    let _ = std::fs::remove_file(&path);
    let image = image.ok_or(CaptureError::CoreGraphicsImage(0))?;
    Ok(captured_from_rect_image(image, x, y, w, h))
}

/// Sample a spaced luminance grid from a captured image. This is intended for
/// cheap visual-change detection: it avoids retaining all colour channels and
/// compares only `columns * rows` cells.
pub fn sample_luma_signature(
    captured: &CapturedWindow,
    columns: usize,
    rows: usize,
) -> Option<Vec<u8>> {
    let image = &captured.image;
    let src_w = image.width();
    let src_h = image.height();
    if src_w == 0 || src_h == 0 || columns == 0 || rows == 0 {
        return None;
    }
    let bpp = (image.bits_per_pixel() / 8).max(1);
    let bpr = image.bytes_per_row();
    let bytes = image.data();
    let raw = bytes.bytes();
    if raw.is_empty() || bpr == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(columns * rows);
    for row in 0..rows {
        let y = (((row as f64 + 0.5) * src_h as f64 / rows as f64).floor() as usize).min(src_h - 1);
        for col in 0..columns {
            let x = (((col as f64 + 0.5) * src_w as f64 / columns as f64).floor() as usize)
                .min(src_w - 1);
            out.push(sample_luma(raw, bpr, bpp, x, y));
        }
    }
    Some(out)
}

/// Capture a window **composited** (via `screencapture -l<window_id>`), which —
/// unlike `CGWindowListCreateImage` — includes the GPU/WebGL canvas (a rendered
/// chart curve) and works even when the window is off-screen / occluded. Returns
/// the same [`CapturedWindow`] shape (geometry from the window bounds).
pub fn capture_window_composited(window_id: u32) -> Result<CapturedWindow, CaptureError> {
    let bounds = cg_window_bounds(window_id)?;
    let path = unique_png_path(&format!("dunst_win_{window_id}"));
    let ok = std::process::Command::new("/usr/sbin/screencapture")
        .args(["-x", "-o", &format!("-l{window_id}")])
        .arg(&path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return Err(CaptureError::CoreGraphicsImage(window_id));
    }
    let image = load_png_cg(&path.to_string_lossy());
    let _ = std::fs::remove_file(&path);
    let image = image.ok_or(CaptureError::CoreGraphicsImage(window_id))?;
    let image_size_px = (image.width() as f64, image.height() as f64);
    let backing_scale = if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
        ((image_size_px.0 / bounds.size.width) + (image_size_px.1 / bounds.size.height)) / 2.0
    } else {
        1.0
    };
    Ok(CapturedWindow {
        image,
        geometry: geometry_from_rect(bounds, image_size_px, backing_scale),
    })
}

/// Decode a PNG file into a [`CGImage`](core_graphics::image::CGImage).
fn load_png_cg(path: &str) -> Option<core_graphics::image::CGImage> {
    let bytes = std::fs::read(path).ok()?;
    let provider = CGDataProvider::from_buffer(Arc::new(bytes));
    // SAFETY: `provider` is a valid CGDataProviderRef; the function returns a +1
    // owned CGImageRef or null (checked). Null decode array / default intent.
    let raw = unsafe {
        CGImageCreateWithPNGDataProvider(provider.as_ptr().cast(), std::ptr::null(), false, 0)
    };
    if raw.is_null() {
        return None;
    }
    // SAFETY: `raw` is a +1 owned CGImageRef handed to CGImage's create-rule owner.
    Some(unsafe { core_graphics::image::CGImage::from_ptr(raw.cast()) })
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGImageCreateWithPNGDataProvider(
        source: *const c_void,
        decode: *const f64,
        should_interpolate: bool,
        intent: u32,
    ) -> *mut c_void;
}

/// One top-level (layer-0) window, for target discovery.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub window_id: u32,
    pub pid: i32,
    pub app: String,
    pub title: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub on_screen: bool,
}

/// One active display in macOS global screen-point coordinates.
///
/// `index` is Dunst's stable 1-based display number for operators: the main
/// display first, then the remaining displays sorted by their global origin.
#[derive(Debug, Clone)]
pub struct DisplayInfo {
    pub index: usize,
    pub display_id: u32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub pixels_wide: u64,
    pub pixels_high: u64,
    pub scale: f64,
    pub is_main: bool,
}

/// The screen bounds `(x, y, w, h)` of a window by id — for mapping a screen
/// point to the window-local coordinate a background click needs.
pub fn window_bounds(window_id: u32) -> Option<(f64, f64, f64, f64)> {
    cg_window_bounds(window_id)
        .ok()
        .map(|r| (r.origin.x, r.origin.y, r.size.width, r.size.height))
}

/// List every top-level (layer-0) window — including off-screen / other-Space
/// ones — for picking a `window_id` to drive. Fills the MCP's target-discovery
/// gap (previously external to the daemon).
pub fn list_windows() -> Vec<WindowInfo> {
    let on_screen: HashSet<u32> = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    )
    .map(|arr| {
        arr.get_all_values()
            .into_iter()
            .filter_map(|r| window_number(r as CFDictionaryRef))
            .collect()
    })
    .unwrap_or_default();

    let Some(all) = copy_window_info(kCGWindowListOptionAll, kCGNullWindowID) else {
        return Vec::new();
    };
    // SAFETY: reading the immutable CoreGraphics CFString key statics.
    let (k_layer, k_pid, k_owner, k_name) = unsafe {
        (
            kCGWindowLayer.cast::<c_void>(),
            kCGWindowOwnerPID.cast::<c_void>(),
            kCGWindowOwnerName.cast::<c_void>(),
            kCGWindowName.cast::<c_void>(),
        )
    };
    all.get_all_values()
        .into_iter()
        .filter_map(|r| {
            let info = r as CFDictionaryRef;
            // top-level app windows only (layer 0); skip menubar/dock/shadows.
            if dict_number_key(info, k_layer)? != 0.0 {
                return None;
            }
            let window_id = window_number(info)?;
            let (x, y, w, h) = bounds_from_window_info(info)
                .map(|r| (r.origin.x, r.origin.y, r.size.width, r.size.height))
                .unwrap_or((0.0, 0.0, 0.0, 0.0));
            Some(WindowInfo {
                window_id,
                pid: dict_number_key(info, k_pid).map(|n| n as i32).unwrap_or(0),
                app: dict_string_key(info, k_owner).unwrap_or_default(),
                title: dict_string_key(info, k_name).unwrap_or_default(),
                x,
                y,
                w,
                h,
                on_screen: on_screen.contains(&window_id),
            })
        })
        .collect()
}

/// List active displays with bounds in global screen points and native pixels.
pub fn list_displays() -> Vec<DisplayInfo> {
    let mut ids = CGDisplay::active_displays().unwrap_or_else(|_| vec![CGDisplay::main().id]);
    if ids.is_empty() {
        ids.push(CGDisplay::main().id);
    }
    let main_id = CGDisplay::main().id;
    let mut displays: Vec<(u32, CGDisplay)> =
        ids.into_iter().map(|id| (id, CGDisplay::new(id))).collect();
    displays.sort_by(|(a_id, a), (b_id, b)| {
        let a_main = *a_id == main_id;
        let b_main = *b_id == main_id;
        b_main
            .cmp(&a_main)
            .then_with(|| a.bounds().origin.x.total_cmp(&b.bounds().origin.x))
            .then_with(|| a.bounds().origin.y.total_cmp(&b.bounds().origin.y))
            .then_with(|| a_id.cmp(b_id))
    });
    displays
        .into_iter()
        .enumerate()
        .map(|(offset, (display_id, display))| display_info(offset + 1, display_id, display))
        .filter(valid_display_info)
        .enumerate()
        .map(|(offset, mut display)| {
            display.index = offset + 1;
            display
        })
        .collect()
}

/// Pick the display that owns the largest area of `rect`. If a window straddles
/// displays, this reflects where most of the window is visible.
pub fn display_for_rect(x: f64, y: f64, w: f64, h: f64) -> Option<DisplayInfo> {
    let displays = list_displays();
    let mut best = displays
        .iter()
        .enumerate()
        .map(|(idx, d)| {
            (
                idx,
                rect_intersection_area((x, y, w, h), (d.x, d.y, d.w, d.h)),
            )
        })
        .max_by(|a, b| a.1.total_cmp(&b.1));

    if let Some((idx, area)) = best.take() {
        if area > 0.0 {
            return displays.get(idx).cloned();
        }
    }

    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
    displays
        .iter()
        .find(|d| cx >= d.x && cx < d.x + d.w && cy >= d.y && cy < d.y + d.h)
        .cloned()
}

/// Read a CFNumber value from a CGWindow dict by a static key pointer.
fn dict_number_key(info: CFDictionaryRef, key: *const c_void) -> Option<f64> {
    let mut p: *const c_void = std::ptr::null();
    // SAFETY: `info`/`key` are valid CF objects; `p` is a checked out-parameter.
    let found = unsafe { CFDictionaryGetValueIfPresent(info, key, &mut p) };
    if found == 0 || p.is_null() {
        return None;
    }
    cf_number_f64(p as CFNumberRef)
}

/// Read a CFString value from a CGWindow dict by a static key pointer.
fn dict_string_key(info: CFDictionaryRef, key: *const c_void) -> Option<String> {
    let mut p: *const c_void = std::ptr::null();
    // SAFETY: `info`/`key` are valid CF objects; `p` is a checked out-parameter.
    let found = unsafe { CFDictionaryGetValueIfPresent(info, key, &mut p) };
    if found == 0 || p.is_null() {
        return None;
    }
    // SAFETY: `p` is a borrowed CFString from the dictionary; get-rule wraps it.
    let s = unsafe { CFString::wrap_under_get_rule(p.cast()) };
    Some(s.to_string())
}

/// The active display whose global bounds contain `(x, y)`, or the main display.
fn display_containing(x: f64, y: f64) -> CGDisplay {
    if let Ok(ids) = CGDisplay::active_displays() {
        for id in ids {
            let d = CGDisplay::new(id);
            let b = d.bounds();
            if x >= b.origin.x
                && x < b.origin.x + b.size.width
                && y >= b.origin.y
                && y < b.origin.y + b.size.height
            {
                return d;
            }
        }
    }
    CGDisplay::main()
}

fn display_info(index: usize, display_id: u32, display: CGDisplay) -> DisplayInfo {
    let bounds = display.bounds();
    let pixels_wide = display.pixels_wide();
    let pixels_high = display.pixels_high();
    let scale = if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
        ((pixels_wide as f64 / bounds.size.width) + (pixels_high as f64 / bounds.size.height)) / 2.0
    } else {
        1.0
    };
    DisplayInfo {
        index,
        display_id,
        x: bounds.origin.x,
        y: bounds.origin.y,
        w: bounds.size.width,
        h: bounds.size.height,
        pixels_wide,
        pixels_high,
        scale,
        is_main: display_id == CGDisplay::main().id,
    }
}

fn valid_display_info(display: &DisplayInfo) -> bool {
    display.w > 0.0
        && display.h > 0.0
        && display.pixels_wide > 0
        && display.pixels_high > 0
        && display.display_id != 0
}

fn sample_luma(raw: &[u8], bytes_per_row: usize, bytes_per_pixel: usize, x: usize, y: usize) -> u8 {
    let offset = y
        .saturating_mul(bytes_per_row)
        .saturating_add(x.saturating_mul(bytes_per_pixel));
    if offset >= raw.len() {
        return 0;
    }
    if bytes_per_pixel >= 3 && offset + 2 < raw.len() {
        ((raw[offset] as u16 + raw[offset + 1] as u16 + raw[offset + 2] as u16) / 3) as u8
    } else {
        raw[offset]
    }
}

fn rect_intersection_area(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> f64 {
    let ax2 = a.0 + a.2;
    let ay2 = a.1 + a.3;
    let bx2 = b.0 + b.2;
    let by2 = b.1 + b.3;
    let w = ax2.min(bx2) - a.0.max(b.0);
    let h = ay2.min(by2) - a.1.max(b.1);
    w.max(0.0) * h.max(0.0)
}

fn capture_cg_window_with_bounds(
    window_id: u32,
    bounds: CGRect,
) -> Result<CapturedWindow, CaptureError> {
    // SAFETY: `CGRectNull` is a CoreGraphics sentinel constant used here to ask
    // CGWindowListCreateImage to derive the capture rect from `window_id`.
    let image = create_image(
        unsafe { CGRectNull },
        kCGWindowListOptionIncludingWindow,
        window_id,
        kCGWindowImageBoundsIgnoreFraming | kCGWindowImageBestResolution,
    )
    .ok_or(CaptureError::CoreGraphicsImage(window_id))?;
    let image_size_px = (image.width() as f64, image.height() as f64);
    let backing_scale = if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
        ((image_size_px.0 / bounds.size.width) + (image_size_px.1 / bounds.size.height)) / 2.0
    } else {
        1.0
    };

    Ok(CapturedWindow {
        image,
        geometry: geometry_from_rect(bounds, image_size_px, backing_scale),
    })
}

fn cg_window_bounds(window_id: u32) -> Result<CGRect, CaptureError> {
    let infos = copy_window_info(kCGWindowListOptionIncludingWindow, window_id)
        .ok_or(CaptureError::CoreGraphicsBounds(window_id))?;
    if let Some(info_ref) = infos.get_all_values().first().copied() {
        let info_ref = info_ref as CFDictionaryRef;
        if let Some(bounds) = bounds_from_window_info(info_ref) {
            return Ok(bounds);
        }
    }

    let infos = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        0,
    )
    .ok_or(CaptureError::CoreGraphicsBounds(window_id))?;
    for info_ref in infos.get_all_values() {
        let info_ref = info_ref as CFDictionaryRef;
        if window_number(info_ref) == Some(window_id) {
            return bounds_from_window_info(info_ref)
                .ok_or(CaptureError::CoreGraphicsBounds(window_id));
        }
    }

    Err(CaptureError::WindowNotFound(window_id))
}

fn bounds_from_window_info(info: CFDictionaryRef) -> Option<CGRect> {
    let mut bounds_ptr: *const c_void = std::ptr::null();
    // SAFETY: `info` is a CGWindow dictionary borrowed from CoreGraphics;
    // `bounds_ptr` is a valid out-parameter and is null-checked before use.
    let found = unsafe {
        CFDictionaryGetValueIfPresent(info, kCGWindowBounds.cast::<c_void>(), &mut bounds_ptr)
    };
    if found == 0 || bounds_ptr.is_null() {
        if std::env::var("DUNST_CAPTURE_DEBUG").as_deref() == Ok("1") {
            eprintln!("debug: kCGWindowBounds not present in CGWindow info");
        }
        return None;
    }
    let bounds_ref = bounds_ptr as CFDictionaryRef;
    let parsed = rect_from_bounds_dict(bounds_ref);
    if parsed.is_none() && std::env::var("DUNST_CAPTURE_DEBUG").as_deref() == Ok("1") {
        eprintln!("debug: could not parse kCGWindowBounds dictionary");
    }
    parsed
}

fn window_number(info: CFDictionaryRef) -> Option<u32> {
    let mut number_ptr: *const c_void = std::ptr::null();
    // SAFETY: `info` is a CGWindow dictionary borrowed from CoreGraphics;
    // `number_ptr` is a valid out-parameter and is null-checked before use.
    let found = unsafe {
        CFDictionaryGetValueIfPresent(info, kCGWindowNumber.cast::<c_void>(), &mut number_ptr)
    };
    if found == 0 || number_ptr.is_null() {
        return None;
    }
    let value = cf_number_i64(number_ptr as CFNumberRef)?;
    u32::try_from(value).ok()
}

fn rect_from_bounds_dict(bounds: CFDictionaryRef) -> Option<CGRect> {
    let x = dict_number(bounds, "X")?;
    let y = dict_number(bounds, "Y")?;
    let width = dict_number(bounds, "Width")?;
    let height = dict_number(bounds, "Height")?;
    Some(CGRect::new(
        &CGPoint::new(x, y),
        &CGSize::new(width, height),
    ))
}

fn dict_number(dict: CFDictionaryRef, key: &str) -> Option<f64> {
    let key = CFString::new(key);
    let mut number_ptr: *const c_void = std::ptr::null();
    // SAFETY: `dict` and `key` are valid CF objects for this scope;
    // `number_ptr` is a valid out-parameter and is null-checked before use.
    let found = unsafe {
        CFDictionaryGetValueIfPresent(dict, key.as_CFTypeRef().cast::<c_void>(), &mut number_ptr)
    };
    if found == 0 || number_ptr.is_null() {
        return None;
    }
    cf_number_f64(number_ptr as CFNumberRef)
}

fn cf_number_i64(number: CFNumberRef) -> Option<i64> {
    let mut value = 0_i64;
    // SAFETY: `number` is expected to be a CFNumberRef from the CGWindow
    // dictionary; `value` is a correctly sized out-parameter.
    let ok = unsafe {
        CFNumberGetValue(
            number,
            kCFNumberSInt64Type,
            (&mut value as *mut i64).cast::<c_void>(),
        )
    };
    ok.then_some(value)
}

fn cf_number_f64(number: CFNumberRef) -> Option<f64> {
    let mut value = 0.0_f64;
    // SAFETY: `number` is expected to be a CFNumberRef from the CGWindow
    // dictionary; `value` is a correctly sized out-parameter.
    let ok = unsafe {
        CFNumberGetValue(
            number,
            kCFNumberFloat64Type,
            (&mut value as *mut f64).cast::<c_void>(),
        )
    };
    ok.then_some(value)
}

fn geometry_from_rect(
    rect: CGRect,
    image_size_px: (f64, f64),
    backing_scale: f64,
) -> CaptureGeometry {
    CaptureGeometry {
        window_origin_pt: (rect.origin.x, rect.origin.y),
        window_size_pt: (rect.size.width, rect.size.height),
        image_size_px,
        backing_scale,
    }
}
