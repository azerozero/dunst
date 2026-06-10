//! Apple Vision OCR (owner: Codex, P1a).

use std::{fmt, ptr::NonNull};

use core_graphics::image::CGImage;
use foreign_types::ForeignType;
use objc2::{rc::Retained, AnyThread, ClassType};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGImage as ObjcCgImage;
use objc2_foundation::{NSArray, NSDictionary};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedText,
    VNRecognizedTextObservation, VNRequest, VNRequestTextRecognitionLevel,
};

use crate::{coords::window_rect_to_vision_roi, CaptureGeometry, NormRect, OcrBox};

#[derive(Debug)]
pub enum OcrError {
    Vision(String),
}

impl fmt::Display for OcrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vision(err) => write!(f, "Vision OCR failed: {err}"),
        }
    }
}

impl std::error::Error for OcrError {}

#[derive(Debug, Clone, Copy)]
pub enum RecognitionMode {
    Fast,
    Accurate,
}

pub fn ocr_region(
    image: &CGImage,
    geometry: &CaptureGeometry,
    region_screen_pt: Option<visualops_core::Bbox>,
) -> Result<Vec<OcrBox>, OcrError> {
    ocr_region_with_mode(image, geometry, region_screen_pt, RecognitionMode::Fast)
}

pub fn ocr_region_with_mode(
    image: &CGImage,
    geometry: &CaptureGeometry,
    region_screen_pt: Option<visualops_core::Bbox>,
    mode: RecognitionMode,
) -> Result<Vec<OcrBox>, OcrError> {
    // SAFETY: objc2 allocation/init follows the framework convention; the
    // returned retained request owns the Objective-C object.
    let request = unsafe { VNRecognizeTextRequest::init(VNRecognizeTextRequest::alloc()) };
    request.setRecognitionLevel(match mode {
        RecognitionMode::Fast => VNRequestTextRecognitionLevel::Fast,
        RecognitionMode::Accurate => VNRequestTextRecognitionLevel::Accurate,
    });
    request.setUsesLanguageCorrection(false);
    // SAFETY: `region_to_vision_roi` returns a finite normalized CGRect in
    // Vision coordinates; the request object is alive for the call.
    unsafe {
        request.setRegionOfInterest(region_to_vision_roi(region_screen_pt, geometry));
    }

    // SAFETY: `borrowed_objc_cg_image` returns an ObjC-compatible borrowed view
    // of the live CGImage; `options` and the image live through handler init.
    let handler = unsafe {
        let image_ref = borrowed_objc_cg_image(image);
        let options = NSDictionary::<VNImageOption, objc2::runtime::AnyObject>::new();
        VNImageRequestHandler::initWithCGImage_options(
            VNImageRequestHandler::alloc(),
            image_ref,
            &options,
        )
    };

    let request_ref: &VNRecognizeTextRequest = &request;
    let request_base: &VNRequest = request_ref.as_super().as_super();
    let requests: Retained<NSArray<VNRequest>> = NSArray::from_slice(&[request_base]);
    handler
        .performRequests_error(&requests)
        .map_err(|err| OcrError::Vision(err.localizedDescription().to_string()))?;

    let mut out = Vec::new();
    if let Some(results) = request.results() {
        for observation in results.iter() {
            if let Some(ocr_box) = observation_to_box(&observation) {
                out.push(ocr_box);
            }
        }
    }
    Ok(out)
}

/// An [`OcrBox`] carries only the Vision-normalised box; mapping it to screen
/// points is the consumer's job (via `coords::vision_norm_to_screen_pt`), so we
/// do not compute a screen box here (audit #2 — that result was being discarded).
fn observation_to_box(observation: &VNRecognizedTextObservation) -> Option<OcrBox> {
    let candidate: Retained<VNRecognizedText> = observation.topCandidates(1).firstObject()?;
    let text = candidate.string().to_string();
    if text.trim().is_empty() {
        return None;
    }
    // SAFETY: `observation` is a live Vision object yielded by the request
    // results; `boundingBox` returns a value CGRect without retained pointers.
    let rect = unsafe { observation.boundingBox() };
    let norm = NormRect {
        x: rect.origin.x,
        y: rect.origin.y,
        w: rect.size.width,
        h: rect.size.height,
    };
    Some(OcrBox {
        text,
        norm,
        confidence: candidate.confidence(),
    })
}

/// Vision `regionOfInterest` for an optional screen-point region (`None` = whole
/// image). Audit #1: the Y-flip + edge-clamp is owned by
/// [`coords::window_rect_to_vision_roi`] (proven by 14 unit tests) — we convert the
/// screen-point region to window-local points and delegate, instead of
/// re-deriving the transform here with a subtly divergent clamp.
fn region_to_vision_roi(
    region_screen_pt: Option<visualops_core::Bbox>,
    geometry: &CaptureGeometry,
) -> CGRect {
    let Some(region) = region_screen_pt else {
        return CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: 1.0,
                height: 1.0,
            },
        };
    };

    // screen-point → window-local (window_rect_to_vision_roi re-adds the origin).
    let (origin_x, origin_y) = geometry.window_origin_pt;
    let rect_in_window = visualops_core::Bbox {
        x: region.x - origin_x,
        y: region.y - origin_y,
        w: region.w,
        h: region.h,
    };
    let roi = window_rect_to_vision_roi(rect_in_window, geometry);
    CGRect {
        origin: CGPoint { x: roi.x, y: roi.y },
        size: CGSize {
            width: roi.w,
            height: roi.h,
        },
    }
}

unsafe fn borrowed_objc_cg_image(image: &CGImage) -> &ObjcCgImage {
    let ptr = NonNull::new(image.as_ptr().cast::<ObjcCgImage>())
        .expect("Core Graphics returned null CGImage");
    // SAFETY: `CGImage` and objc2's `CGImage` are transparent wrappers for the
    // same CoreGraphics object. The returned reference is borrowed from `image`
    // and cannot outlive the input reference.
    ptr.as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use visualops_core::Bbox;

    fn geom() -> CaptureGeometry {
        CaptureGeometry {
            window_origin_pt: (100.0, 50.0),
            window_size_pt: (1000.0, 600.0),
            image_size_px: (2000.0, 1200.0),
            backing_scale: 2.0,
        }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn roi_none_is_full_unit_square() {
        let r = region_to_vision_roi(None, &geom());
        assert!(approx(r.origin.x, 0.0) && approx(r.origin.y, 0.0));
        assert!(approx(r.size.width, 1.0) && approx(r.size.height, 1.0));
    }

    #[test]
    fn roi_delegates_to_coords_transform() {
        let g = geom();
        // A concrete in-window region expressed in SCREEN points.
        let region = Bbox {
            x: 300.0,
            y: 200.0,
            w: 200.0,
            h: 120.0,
        };
        let got = region_to_vision_roi(Some(region), &g);

        // Reference: the same screen→window-local conversion through the tested
        // coords transform. Locks the unification (audit #1) against regressions.
        let (ox, oy) = g.window_origin_pt;
        let want = window_rect_to_vision_roi(
            Bbox {
                x: region.x - ox,
                y: region.y - oy,
                w: region.w,
                h: region.h,
            },
            &g,
        );
        assert!(
            approx(got.origin.x, want.x),
            "x {} vs {}",
            got.origin.x,
            want.x
        );
        assert!(
            approx(got.origin.y, want.y),
            "y {} vs {}",
            got.origin.y,
            want.y
        );
        assert!(approx(got.size.width, want.w));
        assert!(approx(got.size.height, want.h));

        // And the result is a valid sub-rectangle of the unit square.
        assert!(got.origin.x >= 0.0 && got.origin.y >= 0.0);
        assert!(got.origin.x + got.size.width <= 1.0 + 1e-9);
        assert!(got.origin.y + got.size.height <= 1.0 + 1e-9);
    }
}
