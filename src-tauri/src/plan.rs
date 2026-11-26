//! The edit model: everything becomes a list of ranges to keep from the original.
//! Trim is one kept range; cut-out is the complement of the removed ranges.

use serde::{Deserialize, Serialize};

/// Touching ranges closer than this are merged.
const EPS: f64 = 1e-6;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Range {
    pub start: f64,
    pub end: f64,
}

impl Range {
    pub fn new(start: f64, end: f64) -> Self {
        Self { start, end }
    }
    pub fn len(&self) -> f64 {
        self.end - self.start
    }
}

pub fn total(ranges: &[Range]) -> f64 {
    ranges.iter().map(Range::len).sum()
}

/// Clamps to [0, duration], drops empty or invalid ranges, sorts and merges overlapping or touching ones.
pub fn normalize(ranges: &[Range], duration: f64) -> Vec<Range> {
    let mut v: Vec<Range> = ranges
        .iter()
        .filter(|r| r.start.is_finite() && r.end.is_finite())
        .map(|r| Range::new(r.start.max(0.0), r.end.min(duration)))
        .filter(|r| r.len() > EPS)
        .collect();
    v.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut out: Vec<Range> = Vec::with_capacity(v.len());
    for r in v {
        match out.last_mut() {
            Some(last) if r.start <= last.end + EPS => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// Ranges to keep after removing `removed`; kept pieces shorter than `min_len` (about one frame) are dropped.
pub fn keep_from_removed(removed: &[Range], duration: f64, min_len: f64) -> Vec<Range> {
    // A gap must be longer than EPS too, or a cut starting at 0 (or ending at the end)
    // would leave an empty kept range when min_len is 0.
    let long_enough = |gap: f64| gap > EPS && gap >= min_len;
    let mut keep = Vec::new();
    let mut cursor = 0.0;
    for r in normalize(removed, duration) {
        if long_enough(r.start - cursor) {
            keep.push(Range::new(cursor, r.start));
        }
        cursor = r.end;
    }
    if long_enough(duration - cursor) {
        keep.push(Range::new(cursor, duration));
    }
    keep
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlanError {
    InvalidDuration,
    EmptyRange,
}

impl From<PlanError> for crate::msg::Msg {
    fn from(e: PlanError) -> Self {
        match e {
            PlanError::InvalidDuration => crate::msg::Msg::DurationUnknown,
            PlanError::EmptyRange => crate::msg::Msg::EmptyRange,
        }
    }
}

/// The single kept range of a trim, clamped to the file.
pub fn keep_from_trim(start: f64, end: f64, duration: f64) -> Result<Range, PlanError> {
    if !(duration.is_finite() && duration > 0.0) {
        return Err(PlanError::InvalidDuration);
    }
    let r = Range::new(start.max(0.0), end.min(duration));
    if !(r.start.is_finite() && r.end.is_finite()) || r.len() <= EPS {
        return Err(PlanError::EmptyRange);
    }
    Ok(r)
}

/// Where `r` (original timeline) plays in the result made of `keep` (kept ranges in order, joined
/// back to back): one range per kept piece it overlaps, on the result's timeline (0 = the result's
/// first frame), touching ranges merged. Empty when `r` falls in what was cut away. A 자르기 is
/// `keep = [start, end]` (result time = original − start). Used for time-limited 화면 효과
/// (모자이크 · 블러), whose filter runs on the joined picture.
pub fn to_result(r: Range, keep: &[Range]) -> Vec<Range> {
    let mut out: Vec<Range> = Vec::new();
    let mut offset = 0.0;
    for k in keep {
        let (s, e) = (r.start.max(k.start), r.end.min(k.end));
        if e - s > EPS {
            let piece = Range::new(offset + s - k.start, offset + e - k.start);
            match out.last_mut() {
                Some(last) if (piece.start - last.end).abs() <= EPS => last.end = piece.end,
                _ => out.push(piece),
            }
        }
        offset += k.len();
    }
    out
}

/// [`to_result`] without merging, each piece with where it starts on the original timeline: what a
/// layer that plays along the original (a PiP video) needs, since its own time jumps at each cut.
pub fn to_result_pieces(r: Range, keep: &[Range]) -> Vec<(Range, f64)> {
    let mut out = Vec::new();
    let mut offset = 0.0;
    for k in keep {
        let (s, e) = (r.start.max(k.start), r.end.min(k.end));
        if e - s > EPS {
            out.push((Range::new(offset + s - k.start, offset + e - k.start), s));
        }
        offset += k.len();
    }
    out
}

/// Where a stream-copy cut that asks for `t` really starts: the last keyframe at or before `t`.
pub fn keyframe_at_or_before(keyframes: &[f64], t: f64) -> Option<f64> {
    let i = keyframes.partition_point(|k| *k <= t + 1e-3);
    i.checked_sub(1).map(|i| keyframes[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(a: f64, b: f64) -> Range {
        Range::new(a, b)
    }

    #[test]
    fn plan_example_from_design() {
        // 10:00 file, remove 2:10–3:05 and 7:40–8:20 → keep 0–130, 185–460, 500–600 (8:25).
        let keep = keep_from_removed(&[r(460.0, 500.0), r(130.0, 185.0)], 600.0, 1.0 / 30.0);
        assert_eq!(keep, vec![r(0.0, 130.0), r(185.0, 460.0), r(500.0, 600.0)]);
        assert_eq!(total(&keep), 505.0);
    }

    #[test]
    fn merges_overlapping_and_touching() {
        assert_eq!(
            normalize(&[r(5.0, 8.0), r(1.0, 3.0), r(2.0, 4.0), r(4.0, 5.0)], 10.0),
            vec![r(1.0, 8.0)]
        );
    }

    #[test]
    fn clamps_and_drops_junk() {
        assert_eq!(
            normalize(&[r(-5.0, 1.0), r(9.0, 20.0), r(3.0, 3.0), r(f64::NAN, 2.0)], 10.0),
            vec![r(0.0, 1.0), r(9.0, 10.0)]
        );
    }

    #[test]
    fn drops_slivers_shorter_than_a_frame() {
        let keep = keep_from_removed(&[r(0.0, 4.99), r(5.0, 10.0)], 10.0, 1.0 / 30.0);
        assert!(keep.is_empty(), "the 0.01 s gap is shorter than one frame");
    }

    #[test]
    fn cuts_at_the_edges_leave_no_empty_pieces() {
        assert_eq!(keep_from_removed(&[r(0.0, 2.0), r(8.0, 10.0)], 10.0, 0.0), vec![r(2.0, 8.0)]);
    }

    #[test]
    fn removing_nothing_keeps_everything() {
        assert_eq!(keep_from_removed(&[], 42.0, 0.04), vec![r(0.0, 42.0)]);
    }

    /// Property check with a tiny deterministic PRNG: output is sorted, disjoint, inside the file,
    /// and kept + removed always adds up to the duration.
    #[test]
    fn keep_list_properties() {
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for _ in 0..2000 {
            let duration = 1.0 + rnd() * 600.0;
            let n = (rnd() * 8.0) as usize;
            let removed: Vec<Range> = (0..n)
                .map(|_| {
                    let a = rnd() * duration * 1.2 - duration * 0.1;
                    r(a, a + rnd() * duration * 0.3)
                })
                .collect();
            let merged = normalize(&removed, duration);
            let keep = keep_from_removed(&removed, duration, 0.0);
            for w in keep.windows(2) {
                assert!(w[0].end < w[1].start);
            }
            for k in &keep {
                assert!(k.start >= 0.0 && k.end <= duration && k.len() > 0.0);
            }
            assert!((total(&keep) + total(&merged) - duration).abs() < 1e-6);
        }
    }

    #[test]
    fn to_result_follows_the_kept_pieces() {
        // 자르기 5–15: the result starts at 5.
        assert_eq!(to_result(r(7.0, 9.0), &[r(5.0, 15.0)]), vec![r(2.0, 4.0)]);
        assert_eq!(to_result(r(0.0, 6.0), &[r(5.0, 15.0)]), vec![r(0.0, 1.0)]);
        assert_eq!(to_result(r(15.0, 20.0), &[r(5.0, 15.0)]), vec![], "after OUT");
        // 구간 제거 of 2–3.5 and 7–8 from 10 s: keep 0–2, 3.5–7, 8–10 (result 0–2, 2–5.5, 5.5–7.5).
        let keep = [r(0.0, 2.0), r(3.5, 7.0), r(8.0, 10.0)];
        assert_eq!(to_result(r(1.0, 9.0), &keep), vec![r(1.0, 6.5)], "pieces back to back merge");
        assert_eq!(to_result(r(2.5, 3.0), &keep), vec![], "inside a removed range");
        assert_eq!(to_result(r(6.0, 7.5), &keep), vec![r(4.5, 5.5)]);
        // A gap kept between two ranges stays two ranges.
        assert_eq!(to_result(r(1.0, 4.0), &[r(0.0, 2.0), r(3.0, 5.0)]), vec![r(1.0, 3.0)]);
        assert_eq!(to_result(r(1.0, 1.5), &[r(0.0, 2.0), r(3.0, 5.0)]), vec![r(1.0, 1.5)]);
    }

    #[test]
    fn to_result_pieces_keep_where_each_piece_came_from() {
        let keep = [r(0.0, 2.0), r(3.5, 7.0), r(8.0, 10.0)];
        assert_eq!(to_result_pieces(r(1.0, 9.0), &keep), vec![(r(1.0, 2.0), 1.0), (r(2.0, 5.5), 3.5), (r(5.5, 6.5), 8.0)]);
        assert_eq!(to_result_pieces(r(7.2, 7.8), &keep), vec![]);
    }

    /// The mapped ranges are as long as the overlap with what is kept, sorted, inside the result.
    #[test]
    fn to_result_properties() {
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for _ in 0..2000 {
            let duration = 1.0 + rnd() * 300.0;
            let removed: Vec<Range> = (0..(rnd() * 6.0) as usize)
                .map(|_| {
                    let a = rnd() * duration;
                    r(a, a + rnd() * duration * 0.2)
                })
                .collect();
            let keep = keep_from_removed(&removed, duration, 0.0);
            let a = rnd() * duration;
            let m = r(a, a + rnd() * duration * 0.5);
            let out = to_result(m, &keep);
            let overlap: f64 = keep.iter().map(|k| (m.end.min(k.end) - m.start.max(k.start)).max(0.0)).sum();
            assert!((total(&out) - overlap).abs() < 1e-6);
            for w in out.windows(2) {
                assert!(w[0].end < w[1].start);
            }
            for o in &out {
                assert!(o.start >= -1e-9 && o.end <= total(&keep) + 1e-9 && o.len() > 0.0);
            }
        }
    }

    #[test]
    fn trim_validation() {
        assert_eq!(keep_from_trim(65.4, 500.0, 600.0), Ok(r(65.4, 500.0)));
        assert_eq!(keep_from_trim(-1.0, 700.0, 600.0), Ok(r(0.0, 600.0)));
        assert_eq!(keep_from_trim(10.0, 10.0, 600.0), Err(PlanError::EmptyRange));
        assert_eq!(keep_from_trim(10.0, 5.0, 600.0), Err(PlanError::EmptyRange));
        assert_eq!(keep_from_trim(0.0, 5.0, 0.0), Err(PlanError::InvalidDuration));
    }

    #[test]
    fn keyframe_snap() {
        let kf = [0.0, 1.001, 2.002, 65.0, 66.0];
        assert_eq!(keyframe_at_or_before(&kf, 65.4), Some(65.0));
        assert_eq!(keyframe_at_or_before(&kf, 65.0), Some(65.0));
        assert_eq!(keyframe_at_or_before(&kf, 1.0005), Some(1.001), "within 1 ms counts as on the keyframe");
        assert_eq!(keyframe_at_or_before(&kf, 0.5), Some(0.0));
        assert_eq!(keyframe_at_or_before(&[], 3.0), None);
    }
}
