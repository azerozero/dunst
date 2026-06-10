//! Core Graphics one-shot window capture (owner: Codex, P1a).

use std::{ffi::c_void, fmt};

use core_foundation::{
    base::TCFType,
    dictionary::{CFDictionaryGetValueIfPresent, CFDictionaryRef},
    number::{kCFNumberFloat64Type, kCFNumberSInt64Type, CFNumberGetValue, CFNumberRef},
    string::CFString,
};
use core_graphics::{
    display::CGRectNull,
    geometry::{CGPoint, CGRect, CGSize},
    window::{
        copy_window_info, create_image, kCGWindowBounds, kCGWindowImageBestResolution,
        kCGWindowImageBoundsIgnoreFraming, kCGWindowListExcludeDesktopElements,
        kCGWindowListOptionIncludingWindow, kCGWindowListOptionOnScreenOnly, kCGWindowNumber,
    },
};

use crate::CaptureGeometry;

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
        if std::env::var("VISUALOPS_CAPTURE_DEBUG").as_deref() == Ok("1") {
            eprintln!("debug: kCGWindowBounds not present in CGWindow info");
        }
        return None;
    }
    let bounds_ref = bounds_ptr as CFDictionaryRef;
    let parsed = rect_from_bounds_dict(bounds_ref);
    if parsed.is_none() && std::env::var("VISUALOPS_CAPTURE_DEBUG").as_deref() == Ok("1") {
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
