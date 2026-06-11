#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("shapes_bench is macOS-only");
}

#[cfg(target_os = "macos")]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    use std::{env, time::Instant};

    use dunst_vision::{capture::capture_window, shapes::detect_shapes};

    const DEFAULT_WINDOW_ID: u32 = 93;
    const DEFAULT_RUNS: usize = 15;

    let mut args = env::args().skip(1);
    let window_id = args
        .next()
        .map(|raw| raw.parse::<u32>())
        .transpose()?
        .unwrap_or(DEFAULT_WINDOW_ID);
    let runs = args
        .next()
        .map(|raw| raw.parse::<usize>())
        .transpose()?
        .unwrap_or(DEFAULT_RUNS)
        .max(3);

    println!("shapes_bench window_id={window_id} runs={runs}");

    let warm = capture_window(window_id)?;
    let _ = detect_shapes(&warm.image, &warm.geometry);

    let captured = capture_window(window_id)?;
    println!(
        "geometry: origin_pt=({:.1},{:.1}) window_pt=({:.1}x{:.1}) image_px=({:.0}x{:.0}) scale={:.2}",
        captured.geometry.window_origin_pt.0,
        captured.geometry.window_origin_pt.1,
        captured.geometry.window_size_pt.0,
        captured.geometry.window_size_pt.1,
        captured.geometry.image_size_px.0,
        captured.geometry.image_size_px.1,
        captured.geometry.backing_scale
    );

    let mut detect_samples = Vec::with_capacity(runs);
    let mut last_shapes = Vec::new();
    for _ in 0..runs {
        let start = Instant::now();
        last_shapes = detect_shapes(&captured.image, &captured.geometry);
        detect_samples.push(start.elapsed());
        std::hint::black_box(&last_shapes);
    }

    let mut capture_detect_samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        let captured = capture_window(window_id)?;
        let shapes = detect_shapes(&captured.image, &captured.geometry);
        capture_detect_samples.push(start.elapsed());
        std::hint::black_box(shapes);
    }

    let detect = stats_ms(detect_samples);
    let capture_detect = stats_ms(capture_detect_samples);
    let counts = count_kinds(&last_shapes);
    println!(
        "detect_shapes_only: p50={:.2}ms p95={:.2}ms shapes={}",
        detect.p50,
        detect.p95,
        last_shapes.len()
    );
    println!(
        "capture_plus_detect: p50={:.2}ms p95={:.2}ms",
        capture_detect.p50, capture_detect.p95
    );
    println!(
        "by_kind: rect={} bar={} circle={} line={} unknown={}",
        counts.rect, counts.bar, counts.circle, counts.line, counts.unknown
    );
    println!("samples:");
    for shape in last_shapes.iter().take(12) {
        println!(
            "  {:?} conf={:.2} bbox=({:.1},{:.1},{:.1},{:.1})",
            shape.kind, shape.confidence, shape.bbox.x, shape.bbox.y, shape.bbox.w, shape.bbox.h
        );
    }
    println!(
        "limits: rectangles are the primary target; bars and circles are heuristic best-effort; no ML and no pie-slice parsing."
    );

    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Debug, Default)]
struct KindCounts {
    rect: usize,
    bar: usize,
    circle: usize,
    line: usize,
    unknown: usize,
}

#[cfg(target_os = "macos")]
fn count_kinds(shapes: &[dunst_vision::shapes::Shape]) -> KindCounts {
    let mut counts = KindCounts::default();
    for shape in shapes {
        match shape.kind {
            dunst_vision::shapes::ShapeKind::Rect => counts.rect += 1,
            dunst_vision::shapes::ShapeKind::Bar => counts.bar += 1,
            dunst_vision::shapes::ShapeKind::Circle => counts.circle += 1,
            dunst_vision::shapes::ShapeKind::Line => counts.line += 1,
            dunst_vision::shapes::ShapeKind::Unknown => counts.unknown += 1,
        }
    }
    counts
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy)]
struct Stats {
    p50: f64,
    p95: f64,
}

#[cfg(target_os = "macos")]
fn stats_ms(mut samples: Vec<std::time::Duration>) -> Stats {
    samples.sort_unstable();
    let p50 = percentile_ms(&samples, 0.50);
    let p95 = percentile_ms(&samples, 0.95);
    Stats { p50, p95 }
}

#[cfg(target_os = "macos")]
fn percentile_ms(samples: &[std::time::Duration], percentile: f64) -> f64 {
    let idx = ((samples.len() as f64 * percentile).ceil() as usize).saturating_sub(1);
    samples[idx.min(samples.len() - 1)].as_secs_f64() * 1000.0
}
