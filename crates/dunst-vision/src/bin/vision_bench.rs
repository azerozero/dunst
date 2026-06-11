use std::{
    env,
    error::Error,
    time::{Duration, Instant},
};

use dunst_core::Bbox;
use dunst_vision::{
    capture::{capture_window, CapturedWindow},
    ocr::ocr_region,
    CaptureGeometry,
};

const DEFAULT_RUNS: usize = 15;
const FOVEA_W_PT: f64 = 600.0;
const FOVEA_H_PT: f64 = 400.0;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let window_id = args
        .next()
        .ok_or("usage: vision_bench <window_id> [runs]")?
        .parse::<u32>()?;
    let runs = args
        .next()
        .map(|raw| raw.parse::<usize>())
        .transpose()?
        .unwrap_or(DEFAULT_RUNS)
        .max(3);

    println!("vision_bench window_id={window_id} runs={runs}");

    let warm = capture_window(window_id)?;
    let fovea = centered_fovea(&warm.geometry);
    let _ = ocr_region(&warm.image, &warm.geometry, Some(fovea))?;

    let mut capture_samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        let captured = capture_window(window_id)?;
        capture_samples.push(start.elapsed());
        std::hint::black_box(captured.geometry);
    }

    let captured = capture_window(window_id)?;
    print_geometry(&captured.geometry);
    let fovea = centered_fovea(&captured.geometry);

    let fovea_result = bench_ocr(&captured, Some(fovea), runs)?;
    let full_result = bench_ocr(&captured, None, runs)?;

    let capture_stats = stats_ms(capture_samples);
    println!(
        "capture_window: p50={:.2}ms p95={:.2}ms",
        capture_stats.p50, capture_stats.p95
    );
    println!(
        "ocr_fast_fovea_600x400pt: p50={:.2}ms p95={:.2}ms lines={}",
        fovea_result.stats.p50, fovea_result.stats.p95, fovea_result.lines
    );
    println!(
        "ocr_fast_full_window: p50={:.2}ms p95={:.2}ms lines={}",
        full_result.stats.p50, full_result.stats.p95, full_result.lines
    );

    let go = capture_stats.p95 < 15.0 && fovea_result.stats.p95 < 60.0;
    println!(
        "verdict: {} (capture_p95<15ms={}, ocr_fovea_p95<60ms={})",
        if go { "GO" } else { "NO-GO" },
        capture_stats.p95 < 15.0,
        fovea_result.stats.p95 < 60.0
    );

    Ok(())
}

struct BenchOcrResult {
    stats: Stats,
    lines: usize,
}

fn bench_ocr(
    captured: &CapturedWindow,
    region: Option<Bbox>,
    runs: usize,
) -> Result<BenchOcrResult, Box<dyn Error>> {
    let mut samples = Vec::with_capacity(runs);
    let mut lines = 0;

    for _ in 0..runs {
        let start = Instant::now();
        let boxes = ocr_region(&captured.image, &captured.geometry, region)?;
        samples.push(start.elapsed());
        lines = boxes.len();
        std::hint::black_box(&boxes);
    }

    Ok(BenchOcrResult {
        stats: stats_ms(samples),
        lines,
    })
}

fn centered_fovea(geometry: &CaptureGeometry) -> Bbox {
    let (origin_x, origin_y) = geometry.window_origin_pt;
    let (win_w, win_h) = geometry.window_size_pt;
    let w = FOVEA_W_PT.min(win_w).max(1.0);
    let h = FOVEA_H_PT.min(win_h).max(1.0);

    Bbox {
        x: origin_x + (win_w - w) / 2.0,
        y: origin_y + (win_h - h) / 2.0,
        w,
        h,
    }
}

fn print_geometry(geometry: &CaptureGeometry) {
    println!(
        "geometry: origin_pt=({:.1},{:.1}) window_pt=({:.1}x{:.1}) image_px=({:.0}x{:.0}) scale={:.2}",
        geometry.window_origin_pt.0,
        geometry.window_origin_pt.1,
        geometry.window_size_pt.0,
        geometry.window_size_pt.1,
        geometry.image_size_px.0,
        geometry.image_size_px.1,
        geometry.backing_scale
    );
}

#[derive(Debug, Clone, Copy)]
struct Stats {
    p50: f64,
    p95: f64,
}

fn stats_ms(mut samples: Vec<Duration>) -> Stats {
    samples.sort_unstable();
    let p50 = percentile_ms(&samples, 0.50);
    let p95 = percentile_ms(&samples, 0.95);
    Stats { p50, p95 }
}

fn percentile_ms(samples: &[Duration], percentile: f64) -> f64 {
    let idx = ((samples.len() as f64 * percentile).ceil() as usize).saturating_sub(1);
    samples[idx.min(samples.len() - 1)].as_secs_f64() * 1000.0
}
