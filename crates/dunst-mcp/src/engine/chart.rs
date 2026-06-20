use dunst_core::Bbox;

use super::TextHit;

/// Parse a (possibly French-formatted) numeric label like `"8 220,00"` or
/// `"8161,84'"` into a value. Space = thousands, comma = decimal; trailing OCR
/// junk is dropped.
pub(super) fn parse_value(s: &str) -> Option<f64> {
    let kept: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == ',')
        .collect();
    if kept.is_empty() {
        return None;
    }
    kept.replacen(',', ".", 1)
        .replace(',', "")
        .parse::<f64>()
        .ok()
}

/// Heuristic: contains a clock time like `HH:MM`.
pub(super) fn looks_like_clock(s: &str) -> bool {
    let b = s.as_bytes();
    (1..b.len().saturating_sub(2)).any(|i| {
        b[i] == b':'
            && b[i - 1].is_ascii_digit()
            && b[i + 1].is_ascii_digit()
            && b[i + 2].is_ascii_digit()
    })
}

/// Linear map from a screen-y (pixels, down-positive) to a chart value, fit from
/// two OCR'd Y-axis price labels.
pub(super) struct YCalibration {
    y_ref: f64,
    v_ref: f64,
    slope: f64,
}

impl YCalibration {
    pub(super) fn value_at(&self, screen_y: f64) -> f64 {
        self.v_ref + (screen_y - self.y_ref) * self.slope
    }
}

/// Y-axis price labels `(screen_y, value)` right of `min_cx`, filtered to the
/// densest value cluster. Gridlines cluster tightly while header values and
/// performance percentages are spread out and dropped.
fn yaxis_points(hits: &[TextHit], min_cx: f64) -> Vec<(f64, f64)> {
    let cands: Vec<(f64, f64)> = hits
        .iter()
        .filter_map(|h| {
            let cx = h.bbox.x + h.bbox.w / 2.0;
            if cx < min_cx {
                return None;
            }
            let v = parse_value(&h.text)?;
            (v >= 1.0).then_some((h.bbox.y + h.bbox.h / 2.0, v))
        })
        .collect();
    if cands.len() < 2 {
        return cands;
    }
    let center = cands.iter().map(|&(_, v)| v).max_by_key(|&v| {
        cands
            .iter()
            .filter(|&&(_, u)| (u - v).abs() <= v * 0.05)
            .count()
    });
    match center {
        Some(c) => cands
            .into_iter()
            .filter(|&(_, v)| (v - c).abs() <= c * 0.05)
            .collect(),
        None => cands,
    }
}

/// Build a Y-axis calibration from the gridline price labels, using the two with
/// the largest vertical separation.
pub(super) fn build_y_calibration(hits: &[TextHit], region: &Bbox) -> Option<YCalibration> {
    let mut pts = yaxis_points(hits, region.x + region.w * 0.82);
    if pts.len() < 2 {
        return None;
    }
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (y_a, v_a) = pts[0];
    let (y_b, v_b) = pts[pts.len() - 1];
    if (y_b - y_a).abs() < 1.0 {
        return None;
    }
    Some(YCalibration {
        y_ref: y_a,
        v_ref: v_a,
        slope: (v_b - v_a) / (y_b - y_a),
    })
}

/// Derive the plot rectangle from OCR'd axis labels. Robust where a thin-curve
/// or pale-fill chart defeats blob detection.
pub(super) fn region_from_axis(hits: &[TextHit]) -> Option<Bbox> {
    let time_xs: Vec<f64> = hits
        .iter()
        .filter(|h| looks_like_clock(&h.text))
        .map(|h| h.bbox.x + h.bbox.w / 2.0)
        .collect();
    if time_xs.len() < 3 {
        return None;
    }
    let x_min = time_xs.iter().copied().fold(f64::MAX, f64::min);
    let x_max = time_xs.iter().copied().fold(f64::MIN, f64::max);
    let price_pts = yaxis_points(hits, x_max - 40.0);
    if price_pts.len() < 2 {
        return None;
    }
    let y_top = price_pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
    let y_bot = price_pts.iter().map(|p| p.0).fold(f64::MIN, f64::max);
    if x_max - x_min < 50.0 || y_bot - y_top < 30.0 {
        return None;
    }
    Some(Bbox {
        x: x_min,
        y: y_top,
        w: x_max - x_min,
        h: y_bot - y_top,
    })
}

/// The X-axis time/date label nearest `x`, restricted to the axis row at the
/// bottom of `region` so a header timestamp cannot masquerade as the axis.
pub(super) fn nearest_time_label(hits: &[TextHit], x: f64, region: &Bbox) -> Option<String> {
    let y_lo = region.y + region.h * 0.6;
    let y_hi = region.y + region.h + 60.0;
    hits.iter()
        .filter(|h| {
            let cy = h.bbox.y + h.bbox.h / 2.0;
            cy >= y_lo && cy <= y_hi && (looks_like_clock(&h.text) || is_axis_token(&h.text))
        })
        .min_by(|a, b| {
            let da = (a.bbox.x + a.bbox.w / 2.0 - x).abs();
            let db = (b.bbox.x + b.bbox.w / 2.0 - x).abs();
            da.total_cmp(&db)
        })
        .map(|h| h.text.trim().to_string())
}

/// A short axis tick token: a clock, a bare day-number, or a `<num> <month>` date.
pub(super) fn is_axis_token(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && t.len() <= 12
        && t.chars().next().is_some_and(|c| c.is_ascii_digit())
        && t.chars().filter(char::is_ascii_digit).count() <= 4
}
