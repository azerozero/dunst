//! Apple Vision OCR (owner: Codex, P1a).

use std::{fmt, ptr::NonNull};

use foreign_types::ForeignType;
use objc2::{rc::Retained, AnyThread, ClassType};
use objc2_core_foundation::{CGRect, CGPoint, CGSize};
use objc2_core_graphics::CGImage as ObjcCgImage;
use objc2_foundation::{NSArray, NSDictionary};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRecognizedText, VNRequest,
    VNRequestTextRecognitionLevel, VNRecognizedTextObservation,
};
use core_graphics::image::CGImage;

use crate::{coords::vision_norm_to_screen_pt, CaptureGeometry, NormRect, OcrBox};

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
    let request = unsafe { VNRecognizeTextRequest::init(VNRecognizeTextRequest::alloc()) };
    request.setRecognitionLevel(match mode {
        RecognitionMode::Fast => VNRequestTextRecognitionLevel::Fast,
        RecognitionMode::Accurate => VNRequestTextRecognitionLevel::Accurate,
    });
    request.setUsesLanguageCorrection(false);
    unsafe {
        request.setRegionOfInterest(region_to_vision_roi(region_screen_pt, geometry));
    }

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
            if let Some(ocr_box) = observation_to_box(&observation, geometry) {
                out.push(ocr_box);
            }
        }
    }
    Ok(out)
}

fn observation_to_box(
    observation: &VNRecognizedTextObservation,
    geometry: &CaptureGeometry,
) -> Option<OcrBox> {
    let candidate: Retained<VNRecognizedText> = observation.topCandidates(1).firstObject()?;
    let text = candidate.string().to_string();
    if text.trim().is_empty() {
        return None;
    }
    let rect = unsafe { observation.boundingBox() };
    let norm = NormRect {
        x: rect.origin.x,
        y: rect.origin.y,
        w: rect.size.width,
        h: rect.size.height,
    };
    let _screen_box = vision_norm_to_screen_pt(norm, geometry);
    Some(OcrBox {
        text,
        norm,
        confidence: candidate.confidence(),
    })
}

fn region_to_vision_roi(region: Option<visualops_core::Bbox>, geometry: &CaptureGeometry) -> CGRect {
    let Some(region) = region else {
        return CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: 1.0,
                height: 1.0,
            },
        };
    };

    let (origin_x, origin_y) = geometry.window_origin_pt;
    let (win_w, win_h) = geometry.window_size_pt;
    let x = ((region.x - origin_x) / win_w).clamp(0.0, 1.0);
    let top = ((region.y - origin_y) / win_h).clamp(0.0, 1.0);
    let w = (region.w / win_w).clamp(0.0, 1.0 - x);
    let h = (region.h / win_h).clamp(0.0, 1.0 - top);
    let y = (1.0 - top - h).clamp(0.0, 1.0);

    CGRect {
        origin: CGPoint { x, y },
        size: CGSize {
            width: w,
            height: h,
        },
    }
}

unsafe fn borrowed_objc_cg_image(image: &CGImage) -> &ObjcCgImage {
    let ptr = NonNull::new(image.as_ptr().cast::<ObjcCgImage>())
        .expect("Core Graphics returned null CGImage");
    ptr.as_ref()
}
