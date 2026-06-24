//! Apple Vision OCR (owner: Codex, P1a).

use std::{fmt, ptr::NonNull};

use core_graphics::image::CGImage;
use foreign_types::ForeignType;
use objc2::{rc::Retained, AnyThread, ClassType};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGImage as ObjcCgImage;
use objc2_foundation::{NSArray, NSDictionary, NSRange, NSString, NSURL};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedText,
    VNRecognizedTextObservation, VNRectangleObservation, VNRequest, VNRequestTextRecognitionLevel,
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
    region_screen_pt: Option<dunst_core::Bbox>,
) -> Result<Vec<OcrBox>, OcrError> {
    ocr_region_with_mode(image, geometry, region_screen_pt, RecognitionMode::Fast)
}

pub fn ocr_region_with_mode(
    image: &CGImage,
    geometry: &CaptureGeometry,
    region_screen_pt: Option<dunst_core::Bbox>,
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
            out.extend(observation_to_boxes(&observation));
        }
    }
    Ok(out)
}

/// OCR an image **file** by URL (e.g. a `screencapture` PNG). The composited
/// screen grab includes GPU/WebGL overlays — a chart crosshair value-at-cursor —
/// that a CGImage window or display capture misses. Returns boxes in normalized
/// Vision coords (bottom-left origin); the caller maps them with the geometry of
/// the captured rect. Whole-image (no region of interest).
pub fn ocr_image_file(path: &str, mode: RecognitionMode) -> Result<Vec<OcrBox>, OcrError> {
    // SAFETY: objc2 allocation/init follows the framework convention; the
    // returned retained request owns the Objective-C object.
    let request = unsafe { VNRecognizeTextRequest::init(VNRecognizeTextRequest::alloc()) };
    request.setRecognitionLevel(match mode {
        RecognitionMode::Fast => VNRequestTextRecognitionLevel::Fast,
        RecognitionMode::Accurate => VNRequestTextRecognitionLevel::Accurate,
    });
    request.setUsesLanguageCorrection(false);

    let ns_path = NSString::from_str(path);
    let url = NSURL::fileURLWithPath(&ns_path);
    // SAFETY: objc2 alloc/init per framework convention; `url` and `options` live
    // through handler init.
    let handler = unsafe {
        let options = NSDictionary::<VNImageOption, objc2::runtime::AnyObject>::new();
        VNImageRequestHandler::initWithURL_options(VNImageRequestHandler::alloc(), &url, &options)
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
            out.extend(observation_to_boxes(&observation));
        }
    }
    Ok(out)
}

/// Convert one Vision text observation into one or more [`OcrBox`]es. Vision
/// returns one observation per recognized *line*, but a line can span visually
/// separate UI runs that merely happen to be collinear — e.g. a dropdown row
/// label overlapping background page text on the same band ("Choose the
/// minimal… Contents"). Those merge into one observation and one box, so an
/// OCR-bound click on either run lands between them. We split a line into
/// spatially-separated runs using Vision's per-character-range boxes, but only at
/// LARGE horizontal gaps (clear layout separations, not normal word spacing). On
/// any uncertainty we fall back to the single whole-line box — worst case is the
/// previous behaviour. An [`OcrBox`] carries only the Vision-normalised box;
/// mapping it to screen points is the consumer's job.
fn observation_to_boxes(observation: &VNRecognizedTextObservation) -> Vec<OcrBox> {
    let Some(candidate) = observation.topCandidates(1).firstObject() else {
        return Vec::new();
    };
    let text = candidate.string().to_string();
    if text.trim().is_empty() {
        return Vec::new();
    }
    let confidence = candidate.confidence();
    // SAFETY: `observation` is a live Vision object yielded by the request
    // results; `boundingBox` returns a value CGRect without retained pointers.
    let line_rect = unsafe { observation.boundingBox() };
    let line_box = OcrBox {
        text: text.clone(),
        norm: NormRect {
            x: line_rect.origin.x,
            y: line_rect.origin.y,
            w: line_rect.size.width,
            h: line_rect.size.height,
        },
        confidence,
    };

    let words = word_boxes(&candidate, &text);
    let split = split_line_by_gaps(&words, confidence);
    if split.len() >= 2 {
        split
    } else {
        vec![line_box]
    }
}

/// Per-word boxes for a recognized line via Vision's `boundingBoxForRange`.
/// Returns empty if any range query fails (caller falls back to the whole line).
fn word_boxes(candidate: &VNRecognizedText, text: &str) -> Vec<(String, NormRect)> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut word_start_u16 = 0usize;
    let mut u16_offset = 0usize;
    for ch in text.chars() {
        let w = ch.len_utf16();
        if ch.is_whitespace() {
            if !word.is_empty() {
                match word_rect(candidate, word_start_u16, u16_offset - word_start_u16) {
                    Some(rect) => out.push((std::mem::take(&mut word), rect)),
                    None => return Vec::new(),
                }
            }
            word_start_u16 = u16_offset + w;
        } else {
            if word.is_empty() {
                word_start_u16 = u16_offset;
            }
            word.push(ch);
        }
        u16_offset += w;
    }
    if !word.is_empty() {
        match word_rect(candidate, word_start_u16, u16_offset - word_start_u16) {
            Some(rect) => out.push((word, rect)),
            None => return Vec::new(),
        }
    }
    out
}

fn word_rect(candidate: &VNRecognizedText, location: usize, length: usize) -> Option<NormRect> {
    if length == 0 {
        return None;
    }
    let range = NSRange { location, length };
    // SAFETY: `candidate` is a live VNRecognizedText; `range` is within the
    // candidate string (derived from its own UTF-16 offsets). Vision errors are
    // mapped to None and the caller falls back to the whole line.
    let observation: Retained<VNRectangleObservation> =
        unsafe { candidate.boundingBoxForRange_error(range) }.ok()?;
    // SAFETY: live observation; `boundingBox` returns a value CGRect.
    let r = unsafe { observation.boundingBox() };
    if r.size.width <= 0.0 || r.size.height <= 0.0 {
        return None;
    }
    Some(NormRect {
        x: r.origin.x,
        y: r.origin.y,
        w: r.size.width,
        h: r.size.height,
    })
}

/// Group consecutive (left-to-right) word boxes into runs, breaking only at a
/// LARGE horizontal gap (> 2.5x the line height) — a clear UI-layout separation,
/// not normal inter-word spacing. Returns one [`OcrBox`] per run, or empty when
/// the line is a single run (caller keeps the whole-line box). Pure: unit-tested.
fn split_line_by_gaps(words: &[(String, NormRect)], confidence: f32) -> Vec<OcrBox> {
    if words.len() < 2 {
        return Vec::new();
    }
    let mut sorted: Vec<&(String, NormRect)> = words.iter().collect();
    sorted.sort_by(|a, b| {
        a.1.x
            .partial_cmp(&b.1.x)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut runs: Vec<Vec<&(String, NormRect)>> = vec![vec![sorted[0]]];
    for word in &sorted[1..] {
        let prev = runs.last().unwrap().last().unwrap();
        let gap = word.1.x - (prev.1.x + prev.1.w);
        let line_h = prev.1.h.max(word.1.h);
        if gap > line_h * 2.5 {
            runs.push(vec![*word]);
        } else {
            runs.last_mut().unwrap().push(*word);
        }
    }
    if runs.len() < 2 {
        return Vec::new();
    }
    runs.into_iter()
        .map(|run| {
            let text = run
                .iter()
                .map(|(t, _)| t.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let minx = run.iter().map(|(_, r)| r.x).fold(f64::INFINITY, f64::min);
            let miny = run.iter().map(|(_, r)| r.y).fold(f64::INFINITY, f64::min);
            let maxx = run
                .iter()
                .map(|(_, r)| r.x + r.w)
                .fold(f64::NEG_INFINITY, f64::max);
            let maxy = run
                .iter()
                .map(|(_, r)| r.y + r.h)
                .fold(f64::NEG_INFINITY, f64::max);
            OcrBox {
                text,
                norm: NormRect {
                    x: minx,
                    y: miny,
                    w: maxx - minx,
                    h: maxy - miny,
                },
                confidence,
            }
        })
        .collect()
}

/// Vision `regionOfInterest` for an optional screen-point region (`None` = whole
/// image). Audit #1: the Y-flip + edge-clamp is owned by
/// [`coords::window_rect_to_vision_roi`] (proven by 14 unit tests) — we convert the
/// screen-point region to window-local points and delegate, instead of
/// re-deriving the transform here with a subtly divergent clamp.
fn region_to_vision_roi(
    region_screen_pt: Option<dunst_core::Bbox>,
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
    let rect_in_window = dunst_core::Bbox {
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
    use dunst_core::Bbox;

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

    fn word(t: &str, x: f64, w: f64) -> (String, NormRect) {
        (
            t.to_string(),
            NormRect {
                x,
                y: 0.5,
                w,
                h: 0.02,
            },
        )
    }

    #[test]
    fn split_keeps_normal_spacing_as_one_run() {
        // Two words a normal inter-word gap apart (< 2.5x height) stay together,
        // so the caller keeps the single whole-line box (empty split result).
        let words = vec![word("Choose", 0.10, 0.06), word("minimal", 0.17, 0.06)];
        assert!(split_line_by_gaps(&words, 1.0).is_empty());
    }

    #[test]
    fn split_breaks_on_large_layout_gap() {
        // Background text then a far-right dropdown label: the big gap splits them
        // into two tight runs (the "Choose the minimal… Contents" merge case).
        let words = vec![
            word("Choose", 0.05, 0.06),
            word("minimal", 0.12, 0.06),
            word("Contents", 0.60, 0.08),
        ];
        let runs = split_line_by_gaps(&words, 1.0);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "Choose minimal");
        assert_eq!(runs[1].text, "Contents");
        // The second run is tight around "Contents", not the whole line.
        assert!((runs[1].norm.x - 0.60).abs() < 1e-9);
        assert!((runs[1].norm.w - 0.08).abs() < 1e-9);
    }
}
