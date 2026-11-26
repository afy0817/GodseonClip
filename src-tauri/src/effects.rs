//! 화면 효과 (Phase 4): what the editor does to the picture — 크롭, 90° 회전 · 뒤집기, 출력 크기 —
//! as a layer over the keep list (time stays in `plan.rs`). The UI sends the user's choice
//! ([`Effects`]) in pixels of the upright source: FFmpeg turns phone video upright (autorotate)
//! before any filter runs, and the webview shows it upright too. [`plan`] checks it and works out
//! every number the filters need ([`EffectsPlan`]); the filter strings are built in `command.rs`
//! (rule 1). The UI's mirror of [`plan`] is `src/features/editor/effects.ts` (same test cases).
//!
//! Order: crop (source pixels) → quarter turns · flips → scale → pad (맞춤) → `setsar=1`. Cropping
//! first keeps the box where it was drawn, whatever the turn; the size is of the turned picture.

use crate::msg::Msg;
use crate::plan;
use crate::probe::{OverlayKind, OverlaySource, VideoStream};
use serde::{Deserialize, Serialize};

/// The smallest crop side and output side, in pixels (encoders refuse very small pictures).
pub const MIN_SIDE: u32 = 64;
/// The largest output side (H.265 · AV1 levels and the hardware encoders stop around 8K).
pub const MAX_SIDE: u32 = 8192;
/// 모자이크 · 블러: at most this many boxes, each at least this many pixels a side.
pub const MAX_MASKS: usize = 20;
pub const MIN_MASK_SIDE: u32 = 16;
/// PiP: at most this many files over the picture, each at least this many pixels a side.
pub const MAX_OVERLAYS: usize = 4;
pub const MIN_OVERLAY_SIDE: u32 = 16;

/// A rectangle in pixels of the upright source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// 크롭: the box, and the 비율 it was locked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// 16:9 · 9:16 · 1:1 · 4:3 as seen (after the turn) as [w, h]; None for 자유. A preset size
    /// then lands on the ratio exactly (9:16 → 1080 × 1920), though the even box is a pixel off.
    #[serde(default)]
    pub ratio: Option<[u32; 2]>,
}

/// 비율이 다를 때 (출력 크기 직접 입력).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Fit {
    /// 맞춤: the whole picture, black bars around it.
    Contain,
    /// 채움: the frame filled, the picture's edges cut off (centered).
    Cover,
    /// 늘림: the picture stretched to the frame.
    Stretch,
}

/// 출력 크기.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OutputSize {
    /// 원본: the (cropped, turned) picture's own size.
    #[default]
    Source,
    /// 1080p · 720p · 480p: the short side down to `short`, never up (일괄 변환's rule).
    Short { short: u32 },
    /// 직접: exactly `width` × `height` (even), up or down.
    Exact { width: u32, height: u32, fit: Fit },
}

/// 모자이크 (square cells) or 블러 (Gaussian blur).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MaskKind {
    Mosaic,
    Blur,
}

/// 부분 모자이크 · 블러: a box (upright source pixels, like the crop) hidden from `start` to `end`
/// (original timeline, seconds) with a strength of 1–100.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mask {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub start: f64,
    pub end: f64,
    pub kind: MaskKind,
    pub strength: u32,
}

fn full_volume() -> u32 {
    100
}

/// PiP: a video or an image (`path`) laid over the picture in a box (upright source pixels) from
/// `start` to `end` (original timeline); a video plays from `offset` in it and goes when it ends.
/// `opacity` 0–100; a video's sound is mixed in at `volume` 0–100 when `audio`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overlay {
    pub path: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub offset: f64,
    #[serde(default = "full_volume")]
    pub opacity: u32,
    #[serde(default)]
    pub audio: bool,
    #[serde(default = "full_volume")]
    pub volume: u32,
}

/// The editor's 화면 효과. The default asks for nothing.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Effects {
    pub crop: Option<Crop>,
    /// Quarter turns clockwise (0–3; larger values wrap).
    pub turn: u8,
    /// Mirror left-right · upside down, after the turn (as seen).
    pub flip_h: bool,
    pub flip_v: bool,
    pub size: OutputSize,
    /// 모자이크 · 블러 boxes, applied before the crop (they are in the source picture).
    pub masks: Vec<Mask>,
    /// PiP files, over the masks and under the crop, the later on top.
    pub overlays: Vec<Overlay>,
}

impl Effects {
    /// Nothing asked: no crop, turn, flip, size or mask.
    pub fn is_none(&self) -> bool {
        self.crop.is_none()
            && self.turn % 4 == 0
            && !self.flip_h
            && !self.flip_v
            && self.size == OutputSize::Source
            && self.masks.is_empty()
            && self.overlays.is_empty()
    }
}

/// What a mask does to its box: 모자이크 shrinks it to `cells` (area average) and blows it back up
/// (nearest neighbour), 블러 blurs it by `sigma` pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MaskFilter {
    Mosaic { cells: [u32; 2] },
    Blur { sigma: u32 },
}

/// One mask, planned: its box (even), the filter and when it shows — the original timeline from
/// [`plan`], the result's after [`EffectsPlan::in_result`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MaskPlan {
    pub rect: Rect,
    pub filter: MaskFilter,
    pub times: Vec<plan::Range>,
}

/// One stretch of a PiP video in the result: shown from `start` for `len` seconds, played from
/// `source_offset` in the put-on file (a cut makes several, since the video follows the original).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayPiece {
    pub start: f64,
    pub len: f64,
    pub source_offset: f64,
}

/// One PiP, planned: the file, its box (even), opacity, the sound's volume if mixed, and when it
/// shows — the original span from [`plan`], the result's after [`EffectsPlan::in_result`] (an
/// image: `times`; a video: `pieces`, each its own input).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayPlan {
    pub path: String,
    pub rect: Rect,
    pub opacity: u32,
    /// Mix the video's sound at this volume (0–100); None: silent.
    pub volume: Option<u32>,
    pub span: plan::Range,
    pub offset: f64,
    /// Set by [`EffectsPlan::attach_sources`] (the file's probe).
    pub source: Option<OverlaySource>,
    pub times: Vec<plan::Range>,
    pub pieces: Vec<OverlayPiece>,
}

fn plan_overlay(o: &Overlay, sw: u32, sh: u32) -> Result<OverlayPlan, Msg> {
    let rect = Rect { x: even_floor(o.x), y: even_floor(o.y), width: even_floor(o.width), height: even_floor(o.height) };
    if rect.width < MIN_OVERLAY_SIDE || rect.height < MIN_OVERLAY_SIDE {
        return Err(Msg::OverlayTooSmall { min: MIN_OVERLAY_SIDE });
    }
    if rect.x as u64 + rect.width as u64 > sw as u64 || rect.y as u64 + rect.height as u64 > sh as u64 {
        return Err(Msg::OverlayOutsidePicture { width: sw, height: sh });
    }
    let ok = o.start.is_finite() && o.end.is_finite() && o.offset.is_finite() && o.start >= 0.0 && o.end > o.start && o.offset >= 0.0;
    if !ok {
        return Err(Msg::OverlayTimeInvalid);
    }
    Ok(OverlayPlan {
        path: o.path.clone(),
        rect,
        opacity: o.opacity.min(100),
        volume: o.audio.then_some(o.volume.min(100)),
        span: plan::Range::new(o.start, o.end),
        offset: o.offset,
        source: None,
        times: vec![plan::Range::new(o.start, o.end)],
        pieces: Vec::new(),
    })
}

/// 모자이크 cell size in pixels for `strength` (1–100) on a picture whose short side is `short`:
/// 0.5 % → 6 % of the short side (1080p: 5 → 65 px, 4K: 11 → 130 px), so a strength looks the
/// same at any resolution. The editor's preview uses the same numbers (`effects.ts`).
pub fn mosaic_cell(strength: u32, short: u32) -> u32 {
    let s = strength.clamp(1, 100) as f64;
    ((short as f64 * (0.005 + (s - 1.0) / 99.0 * 0.055)).round() as u32).max(2)
}

/// 블러 sigma in pixels for `strength` on a picture whose short side is `short`: 0.2 % → 3 %.
pub fn blur_sigma(strength: u32, short: u32) -> u32 {
    let s = strength.clamp(1, 100) as f64;
    ((short as f64 * (0.002 + (s - 1.0) / 99.0 * 0.028)).round() as u32).max(1)
}

fn plan_mask(m: &Mask, sw: u32, sh: u32) -> Result<MaskPlan, Msg> {
    let rect = Rect { x: even_floor(m.x), y: even_floor(m.y), width: even_floor(m.width), height: even_floor(m.height) };
    if rect.width < MIN_MASK_SIDE || rect.height < MIN_MASK_SIDE {
        return Err(Msg::MaskTooSmall { min: MIN_MASK_SIDE });
    }
    if rect.x as u64 + rect.width as u64 > sw as u64 || rect.y as u64 + rect.height as u64 > sh as u64 {
        return Err(Msg::MaskOutsidePicture { width: sw, height: sh });
    }
    if !(m.start.is_finite() && m.end.is_finite() && m.start >= 0.0 && m.end > m.start) {
        return Err(Msg::MaskTimeInvalid);
    }
    let short = sw.min(sh);
    let filter = match m.kind {
        MaskKind::Mosaic => {
            let cell = mosaic_cell(m.strength, short) as f64;
            let cells = |side: u32| ((side as f64 / cell).round() as u32).max(1);
            MaskFilter::Mosaic { cells: [cells(rect.width), cells(rect.height)] }
        }
        MaskKind::Blur => MaskFilter::Blur { sigma: blur_sigma(m.strength, short) },
    };
    Ok(MaskPlan { rect, filter, times: vec![plan::Range::new(m.start, m.end)] })
}

/// 맞춤's frame: the scaled picture sits at (x, y) on `width` × `height` of black.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pad {
    pub width: u32,
    pub height: u32,
    pub x: u32,
    pub y: u32,
}

/// The numbers of the filters, in order. Every position and size is even (4:2:0 chroma).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectsPlan {
    /// 모자이크 · 블러 first, in the source picture.
    pub masks: Vec<MaskPlan>,
    /// Then the PiP files, the later on top.
    pub overlays: Vec<OverlayPlan>,
    /// `crop` in the upright source (채움 included); None: the whole picture.
    pub crop: Option<Rect>,
    /// Quarter turns clockwise after the crop: 0, 1 or 3 (a half turn is both flips).
    pub turn: u8,
    pub flip_h: bool,
    pub flip_v: bool,
    /// `scale` to this picture size (then `setsar=1`); None: no scaling.
    pub scale: Option<[u32; 2]>,
    pub pad: Option<Pad>,
    /// The result's width × height.
    pub size: [u32; 2],
}

impl EffectsPlan {
    /// The PiP files' probes, in order (before [`EffectsPlan::in_result`]); a silent video, or an
    /// image, mixes no sound.
    pub fn attach_sources(&mut self, sources: Vec<OverlaySource>) {
        for (o, s) in self.overlays.iter_mut().zip(sources) {
            if !(s.kind == OverlayKind::Video && s.has_audio) {
                o.volume = None;
            }
            o.source = Some(s);
        }
    }

    /// The masks' times moved to the result made of `keep` (the kept ranges, original timeline —
    /// `[start, end]` for a 자르기), where the filters run; masks that fall in what was cut away are
    /// dropped. None when nothing is left to do.
    pub fn in_result(mut self, keep: &[plan::Range]) -> Option<Self> {
        for m in &mut self.masks {
            m.times = m.times.iter().flat_map(|t| plan::to_result(*t, keep)).collect();
        }
        self.masks.retain(|m| !m.times.is_empty());
        for o in &mut self.overlays {
            let kind = o.source.as_ref().map_or(OverlayKind::Image, |s| s.kind);
            if kind == OverlayKind::Video {
                // The video plays along the original: each kept piece starts where the original is.
                let length = o.source.as_ref().and_then(|s| s.duration).unwrap_or(f64::INFINITY);
                o.pieces = plan::to_result_pieces(o.span, keep)
                    .into_iter()
                    .filter_map(|(r, from)| {
                        let source_offset = o.offset + (from - o.span.start);
                        let len = r.len().min(length - source_offset);
                        (len > 1e-3).then_some(OverlayPiece { start: r.start, len, source_offset })
                    })
                    .collect();
                o.times = o.pieces.iter().map(|p| plan::Range::new(p.start, p.start + p.len)).collect();
            } else {
                o.times = plan::to_result(o.span, keep);
            }
        }
        self.overlays.retain(|o| !o.times.is_empty());
        let nothing = self.masks.is_empty() && self.overlays.is_empty() && self.crop.is_none() && self.turn == 0 && !self.flip_h && !self.flip_v && self.scale.is_none() && self.pad.is_none();
        (!nothing).then_some(self)
    }

    /// The masks and PiP files of the result's stretch [`start`, `start + len`) (after
    /// [`EffectsPlan::in_result`]), on that stretch's own timeline from 0 — what one kept piece of a
    /// 구간 제거 shows. Their filters run on each piece before the join: `concat` places a piece by
    /// its frames' average spacing (and its sound's length), which for a variable frame rate is not
    /// quite where the plan puts it (measured, FFmpeg 9.0.2: 1 ms early with sound, 23 ms without),
    /// so times on the joined timeline could land a frame off at a join. Crop · turn · size stay.
    pub fn in_piece(&self, start: f64, len: f64) -> EffectsPlan {
        let end = start + len;
        let clip = |times: &[plan::Range]| -> Vec<plan::Range> {
            times
                .iter()
                .filter_map(|r| {
                    let (a, b) = (r.start.max(start), r.end.min(end));
                    (b - a > 1e-6).then(|| plan::Range::new(a - start, b - start))
                })
                .collect()
        };
        let mut p = self.clone();
        for m in &mut p.masks {
            m.times = clip(&m.times);
        }
        p.masks.retain(|m| !m.times.is_empty());
        for o in &mut p.overlays {
            if o.source.as_ref().is_some_and(|s| s.kind == OverlayKind::Video) {
                // A video's pieces never cross a join (each is one kept piece's stretch).
                o.pieces = o
                    .pieces
                    .iter()
                    .filter(|q| q.start >= start - 1e-6 && q.start < end - 1e-6)
                    .map(|q| OverlayPiece { start: q.start - start, ..*q })
                    .collect();
                o.times = o.pieces.iter().map(|q| plan::Range::new(q.start, q.start + q.len)).collect();
            } else {
                o.times = clip(&o.times);
            }
        }
        p.overlays.retain(|o| !o.times.is_empty());
        p
    }

    /// The picture's size before 맞춤's bars: the scaled size, or the cropped, turned picture's own
    /// (맞춤 can add bars without scaling). `src`: the upright source size.
    pub fn picture_size(&self, src: (u32, u32)) -> [u32; 2] {
        if let Some(s) = self.scale {
            return s;
        }
        let (w, h) = self.crop.map_or(src, |c| (c.width, c.height));
        if self.turn % 2 == 1 { [h, w] } else { [w, h] }
    }
}

/// `num / den` to the nearest even number (halves up) — what `scale=-2:…` rounds to.
fn even_round(num: u64, den: u64) -> u32 {
    ((num + den) / (2 * den)) as u32 * 2
}

fn even_floor(n: u32) -> u32 {
    n & !1
}

/// 해상도 · 출력 크기 1080p / 720p / 480p: `w` × `h` with the short side brought down to `short`,
/// the other side to the nearest even number (the same numbers as `scale=-2:{short}`). None when
/// the short side is `short` or less (never made bigger) or a side is 0.
pub fn short_side_size(w: u32, h: u32, short: u32) -> Option<[u32; 2]> {
    if w == 0 || h == 0 || w.min(h) <= short {
        return None;
    }
    let other = |long: u32, small: u32| even_round(long as u64 * short as u64, small as u64);
    Some(if w >= h { [other(w, h), short] } else { [short, other(h, w)] })
}

/// [`short_side_size`] for a box locked to `ratio` ([w, h] as seen): when the box is within 1 % of
/// it, the long side follows the ratio (9:16 at 1080 → 1080 × 1920 even from a 1216 × 2160 box).
fn short_side_at_ratio(w: u32, h: u32, short: u32, ratio: Option<[u32; 2]>) -> Option<[u32; 2]> {
    let size = short_side_size(w, h, short)?;
    let Some([a, b]) = ratio.filter(|[a, b]| *a > 0 && *b > 0) else { return Some(size) };
    let (box_ratio, wanted) = (w as f64 / h as f64, a as f64 / b as f64);
    if ((box_ratio - wanted) / wanted).abs() > 0.01 {
        return Some(size);
    }
    let (long, small) = (a.max(b) as u64, a.min(b) as u64);
    let other = even_round(short as u64 * long, small);
    Some(if w >= h { [other, short] } else { [short, other] })
}

/// The crop box, even and inside the `sw` × `sh` picture.
fn check_crop(c: &Crop, sw: u32, sh: u32) -> Result<Rect, Msg> {
    let r = Rect { x: even_floor(c.x), y: even_floor(c.y), width: even_floor(c.width), height: even_floor(c.height) };
    if r.width < MIN_SIDE || r.height < MIN_SIDE {
        return Err(Msg::CropTooSmall { min: MIN_SIDE });
    }
    if r.x as u64 + r.width as u64 > sw as u64 || r.y as u64 + r.height as u64 > sh as u64 {
        return Err(Msg::CropOutsidePicture { width: sw, height: sh });
    }
    Ok(r)
}

fn check_side(n: u32) -> Result<u32, Msg> {
    let n = even_floor(n);
    if (MIN_SIDE..=MAX_SIDE).contains(&n) { Ok(n) } else { Err(Msg::OutputSizeInvalid { min: MIN_SIDE, max: MAX_SIDE }) }
}

/// 맞춤: the size of `cw` × `ch` scaled to fit inside `w` × `h` (even, never past the frame).
fn contain_size(cw: u32, ch: u32, w: u32, h: u32) -> [u32; 2] {
    if w as u64 * ch as u64 <= h as u64 * cw as u64 {
        [w, even_round(w as u64 * ch as u64, cw as u64).clamp(2, h)]
    } else {
        [even_round(h as u64 * cw as u64, ch as u64).clamp(2, w), h]
    }
}

/// 채움: the middle of `crop` with the frame's ratio `w` : `h` as seen (`quarter`: the picture is
/// turned a quarter, so the box's width is seen as its height).
fn cover_crop(crop: Rect, w: u32, h: u32, quarter: bool) -> Rect {
    let (cw, ch) = if quarter { (crop.height, crop.width) } else { (crop.width, crop.height) };
    let (sw, sh) = if cw as u64 * h as u64 > w as u64 * ch as u64 {
        (even_round(ch as u64 * w as u64, h as u64).clamp(2, cw), ch)
    } else {
        (cw, even_round(cw as u64 * h as u64, w as u64).clamp(2, ch))
    };
    let (nw, nh) = if quarter { (sh, sw) } else { (sw, sh) };
    Rect { x: crop.x + even_floor((crop.width - nw) / 2), y: crop.y + even_floor((crop.height - nh) / 2), width: nw, height: nh }
}

/// Checks `e` against the source picture and works out the filters' numbers. None when nothing
/// changes the picture (no effect, a box over the whole picture, a preset that would enlarge it).
pub fn plan(video: &VideoStream, e: &Effects) -> Result<Option<EffectsPlan>, Msg> {
    if e.is_none() {
        return Ok(None);
    }
    let (sw, sh) = video.upright_size();
    if sw == 0 || sh == 0 {
        return Err(Msg::PictureSizeUnknown);
    }
    let whole = Rect { x: 0, y: 0, width: sw, height: sh };
    if e.masks.len() > MAX_MASKS {
        return Err(Msg::TooManyMasks { max: MAX_MASKS });
    }
    let masks = e.masks.iter().map(|m| plan_mask(m, sw, sh)).collect::<Result<Vec<_>, Msg>>()?;
    if e.overlays.len() > MAX_OVERLAYS {
        return Err(Msg::TooManyOverlays { max: MAX_OVERLAYS });
    }
    let overlays = e.overlays.iter().map(|o| plan_overlay(o, sw, sh)).collect::<Result<Vec<_>, Msg>>()?;
    let mut crop = match &e.crop {
        Some(c) => check_crop(c, sw, sh)?,
        None => whole,
    };
    let quarter = e.turn % 2 == 1;
    let seen = |r: Rect| if quarter { (r.height, r.width) } else { (r.width, r.height) };
    let (cw, ch) = seen(crop);
    let ratio = e.crop.and_then(|c| c.ratio);
    let (scale, pad, size) = match e.size {
        OutputSize::Source => (None, None, [cw, ch]),
        OutputSize::Short { short } => {
            let short = check_side(short)?;
            match short_side_at_ratio(cw, ch, short, ratio) {
                Some(s) => (Some(s), None, s),
                None => (None, None, [cw, ch]),
            }
        }
        OutputSize::Exact { width, height, fit } => {
            let (w, h) = (check_side(width)?, check_side(height)?);
            match fit {
                Fit::Stretch => (Some([w, h]), None, [w, h]),
                Fit::Cover => {
                    crop = cover_crop(crop, w, h, quarter);
                    (Some([w, h]), None, [w, h])
                }
                Fit::Contain => {
                    let inner = contain_size(cw, ch, w, h);
                    let pad = (inner != [w, h]).then(|| Pad {
                        width: w,
                        height: h,
                        x: even_floor((w - inner[0]) / 2),
                        y: even_floor((h - inner[1]) / 2),
                    });
                    (Some(inner), pad, [w, h])
                }
            }
        }
    };
    let (cw, ch) = seen(crop);
    let scale = scale.filter(|s| *s != [cw, ch]);
    // A half turn is the two flips.
    let (turn, flip_h, flip_v) = match e.turn % 4 {
        2 => (0, !e.flip_h, !e.flip_v),
        t => (t, e.flip_h, e.flip_v),
    };
    let crop = (crop != whole).then_some(crop);
    if masks.is_empty() && overlays.is_empty() && crop.is_none() && turn == 0 && !flip_h && !flip_v && scale.is_none() && pad.is_none() {
        return Ok(None);
    }
    Ok(Some(EffectsPlan { masks, overlays, crop, turn, flip_h, flip_v, scale, pad, size }))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn video(width: u32, height: u32, rotation: Option<f64>) -> VideoStream {
        VideoStream {
            index: 0,
            codec: "h264".into(),
            profile: None,
            width,
            height,
            start_time: Some(0.0),
            has_b_frames: 0,
            fps: Some(30.0),
            avg_fps: Some(30.0),
            pix_fmt: Some("yuv420p".into()),
            color_primaries: None,
            color_transfer: None,
            color_space: None,
            hdr: false,
            rotation,
            vfr_suspect: false,
            frame_times: None,
        }
    }

    fn crop(x: u32, y: u32, width: u32, height: u32, ratio: Option<[u32; 2]>) -> Option<Crop> {
        Some(Crop { x, y, width, height, ratio })
    }

    const UHD: (u32, u32) = (3840, 2160);

    #[allow(clippy::too_many_arguments)]
    fn mask(x: u32, y: u32, width: u32, height: u32, start: f64, end: f64, kind: MaskKind, strength: u32) -> Mask {
        Mask { x, y, width, height, start, end, kind, strength }
    }

    /// The shape `src/lib/ipc.ts` sends for masks.
    #[test]
    fn masks_from_the_ui() {
        let e: Effects = serde_json::from_str(
            r#"{"masks":[{"x":101,"y":50,"width":200,"height":120,"start":1.5,"end":4,"kind":"mosaic","strength":50},
                         {"x":0,"y":0,"width":64,"height":64,"start":0,"end":10,"kind":"blur","strength":1}]}"#,
        )
        .unwrap();
        assert_eq!(e.masks[0], mask(101, 50, 200, 120, 1.5, 4.0, MaskKind::Mosaic, 50));
        assert_eq!(e.masks[1].kind, MaskKind::Blur);
        assert!(serde_json::from_str::<Effects>(r#"{"crop":null}"#).unwrap().masks.is_empty());
    }

    #[test]
    fn strength_scales_with_the_picture() {
        assert_eq!((mosaic_cell(1, 1080), mosaic_cell(50, 1080), mosaic_cell(100, 1080)), (5, 35, 65));
        assert_eq!((mosaic_cell(50, 2160), mosaic_cell(50, 240)), (70, 8));
        assert_eq!(mosaic_cell(0, 1080), 5, "clamped to 1");
        assert_eq!(mosaic_cell(1, 100), 2, "at least 2 px");
        assert_eq!((blur_sigma(1, 1080), blur_sigma(50, 1080), blur_sigma(100, 1080)), (2, 17, 32));
        assert_eq!(blur_sigma(1, 240), 1);
    }

    #[test]
    fn masks_plan_even_boxes_cells_and_times() {
        let v = video(1920, 1080, None);
        let e = Effects {
            masks: vec![mask(101, 51, 351, 211, 1.5, 4.0, MaskKind::Mosaic, 50), mask(0, 0, 64, 64, 0.0, 10.0, MaskKind::Blur, 100)],
            ..Effects::default()
        };
        let p = plan(&v, &e).unwrap().unwrap();
        assert_eq!((p.crop, p.size), (None, [1920, 1080]), "masks alone change the picture");
        assert_eq!(p.masks[0].rect, Rect { x: 100, y: 50, width: 350, height: 210 });
        // 35 px cells: 350 / 35 = 10, 210 / 35 = 6.
        assert_eq!(p.masks[0].filter, MaskFilter::Mosaic { cells: [10, 6] });
        assert_eq!(p.masks[0].times, vec![plan::Range::new(1.5, 4.0)]);
        assert_eq!(p.masks[1].filter, MaskFilter::Blur { sigma: 32 });
        // A phone video: the box is in the upright picture, the strength follows its short side.
        let phone = video(1920, 1080, Some(90.0));
        let up = Effects { masks: vec![mask(0, 1700, 1080, 200, 0.0, 1.0, MaskKind::Mosaic, 50)], ..Effects::default() };
        assert_eq!(plan(&phone, &up).unwrap().unwrap().masks[0].filter, MaskFilter::Mosaic { cells: [31, 6] });
    }

    #[test]
    fn bad_masks_are_refused() {
        let v = video(1920, 1080, None);
        let one = |m: Mask| plan(&v, &Effects { masks: vec![m], ..Effects::default() }).err();
        assert_eq!(one(mask(1900, 0, 64, 64, 0.0, 1.0, MaskKind::Blur, 50)), Some(Msg::MaskOutsidePicture { width: 1920, height: 1080 }));
        assert_eq!(one(mask(0, 0, 14, 64, 0.0, 1.0, MaskKind::Blur, 50)), Some(Msg::MaskTooSmall { min: MIN_MASK_SIDE }));
        assert_eq!(one(mask(0, 0, 64, 64, 2.0, 2.0, MaskKind::Blur, 50)), Some(Msg::MaskTimeInvalid));
        assert_eq!(one(mask(0, 0, 64, 64, f64::NAN, 2.0, MaskKind::Blur, 50)), Some(Msg::MaskTimeInvalid));
        let many = Effects { masks: vec![mask(0, 0, 64, 64, 0.0, 1.0, MaskKind::Blur, 50); MAX_MASKS + 1], ..Effects::default() };
        assert_eq!(plan(&v, &many).err(), Some(Msg::TooManyMasks { max: MAX_MASKS }));
    }

    pub(crate) fn source(kind: OverlayKind, duration: Option<f64>, has_audio: bool) -> OverlaySource {
        OverlaySource { path: "/v/올린 파일".into(), file_name: "올린 파일".into(), kind, width: 640, height: 360, duration, has_audio }
    }

    fn overlay(x: u32, y: u32, width: u32, height: u32, start: f64, end: f64, offset: f64) -> Overlay {
        Overlay { path: "/v/카메라 1.mp4".into(), x, y, width, height, start, end, offset, opacity: 100, audio: true, volume: 80 }
    }

    #[test]
    fn overlays_from_the_ui() {
        let e: Effects = serde_json::from_str(
            r#"{"overlays":[{"path":"/v/카메라 1.mp4","x":1280,"y":720,"width":577,"height":325,"start":2,"end":9.5,"offset":12,"opacity":70,"audio":true,"volume":80},
                            {"path":"/로고/채널 로고.png","x":40,"y":40,"width":200,"height":60,"start":0,"end":10}]}"#,
        )
        .unwrap();
        assert_eq!(e.overlays[0], Overlay { path: "/v/카메라 1.mp4".into(), x: 1280, y: 720, width: 577, height: 325, start: 2.0, end: 9.5, offset: 12.0, opacity: 70, audio: true, volume: 80 });
        let logo = &e.overlays[1];
        assert_eq!((logo.offset, logo.opacity, logo.audio, logo.volume), (0.0, 100, false, 100), "defaults");
        assert!(!e.is_none());
    }

    #[test]
    fn overlays_plan_boxes_and_refuse_bad_ones() {
        let v = video(1920, 1080, None);
        let p = plan(&v, &Effects { overlays: vec![overlay(1281, 721, 577, 325, 2.0, 9.5, 12.0)], ..Effects::default() }).unwrap().unwrap();
        let o = &p.overlays[0];
        assert_eq!((o.rect, o.volume, o.span, o.offset), (Rect { x: 1280, y: 720, width: 576, height: 324 }, Some(80), plan::Range::new(2.0, 9.5), 12.0));
        let one = |o: Overlay| plan(&v, &Effects { overlays: vec![o], ..Effects::default() }).err();
        assert_eq!(one(overlay(1600, 0, 400, 200, 0.0, 1.0, 0.0)), Some(Msg::OverlayOutsidePicture { width: 1920, height: 1080 }));
        assert_eq!(one(overlay(0, 0, 14, 200, 0.0, 1.0, 0.0)), Some(Msg::OverlayTooSmall { min: MIN_OVERLAY_SIDE }));
        assert_eq!(one(overlay(0, 0, 64, 64, 3.0, 2.0, 0.0)), Some(Msg::OverlayTimeInvalid));
        assert_eq!(one(overlay(0, 0, 64, 64, 0.0, 2.0, -1.0)), Some(Msg::OverlayTimeInvalid));
        let many = Effects { overlays: vec![overlay(0, 0, 64, 64, 0.0, 1.0, 0.0); MAX_OVERLAYS + 1], ..Effects::default() };
        assert_eq!(plan(&v, &many).err(), Some(Msg::TooManyOverlays { max: MAX_OVERLAYS }));
    }

    /// A PiP video follows the original: a cut makes a piece per kept stretch, each played from
    /// where the original is, and the video stops where it ends. An image shows over merged times.
    #[test]
    fn overlay_pieces_follow_the_original_and_stop_at_the_end() {
        let v = video(1920, 1080, None);
        let keep = [plan::Range::new(0.0, 2.0), plan::Range::new(3.5, 7.0), plan::Range::new(8.0, 10.0)];
        let mut p = plan(&v, &Effects { overlays: vec![overlay(0, 0, 64, 64, 1.0, 9.0, 10.0)], ..Effects::default() }).unwrap().unwrap();
        // The put-on video is 14 s long: from 10 s in, 4 s are left.
        p.attach_sources(vec![source(OverlayKind::Video, Some(14.0), true)]);
        let p = p.in_result(&keep).unwrap();
        let o = &p.overlays[0];
        // Original 1–2 → result 1–2 from 10 s; 3.5–7 → result 2–5.5 from 12.5 s but only 1.5 s left.
        assert_eq!(o.pieces, vec![OverlayPiece { start: 1.0, len: 1.0, source_offset: 10.0 }, OverlayPiece { start: 2.0, len: 1.5, source_offset: 12.5 }]);
        assert_eq!(o.times, vec![plan::Range::new(1.0, 2.0), plan::Range::new(2.0, 3.5)]);
        assert_eq!(o.volume, Some(80));
        // An image: no pieces, the merged times; and it mixes no sound.
        let mut p = plan(&v, &Effects { overlays: vec![overlay(0, 0, 64, 64, 1.0, 9.0, 0.0)], ..Effects::default() }).unwrap().unwrap();
        p.attach_sources(vec![source(OverlayKind::Image, None, false)]);
        let o = &p.in_result(&keep).unwrap().overlays[0];
        assert_eq!((o.pieces.len(), o.times.clone(), o.volume), (0, vec![plan::Range::new(1.0, 6.5)], None));
        // A silent video mixes no sound either; a video that is over before its span starts shows nothing.
        let mut p = plan(&v, &Effects { overlays: vec![overlay(0, 0, 64, 64, 1.0, 9.0, 20.0)], ..Effects::default() }).unwrap().unwrap();
        p.attach_sources(vec![source(OverlayKind::Video, Some(14.0), false)]);
        assert_eq!(p.overlays[0].volume, None);
        assert_eq!(p.in_result(&keep), None);
    }

    #[test]
    fn mask_times_move_to_the_result() {
        let v = video(1920, 1080, None);
        let e = Effects {
            masks: vec![mask(0, 0, 64, 64, 1.0, 9.0, MaskKind::Blur, 50), mask(0, 0, 64, 64, 2.2, 3.0, MaskKind::Mosaic, 50)],
            ..Effects::default()
        };
        // 구간 제거 of 2–3.5 and 7–8: the first mask plays 1–6.5 in the result, the second is cut away.
        let keep = [plan::Range::new(0.0, 2.0), plan::Range::new(3.5, 7.0), plan::Range::new(8.0, 10.0)];
        let p = plan(&v, &e).unwrap().unwrap().in_result(&keep).unwrap();
        assert_eq!(p.masks.len(), 1);
        assert_eq!(p.masks[0].times, vec![plan::Range::new(1.0, 6.5)]);
        // Only masks, all cut away: nothing to do.
        let gone = Effects { masks: vec![mask(0, 0, 64, 64, 2.2, 3.0, MaskKind::Mosaic, 50)], ..Effects::default() };
        assert_eq!(plan(&v, &gone).unwrap().unwrap().in_result(&keep), None);
        // A crop stays when its masks go.
        let with_crop = Effects { crop: crop(0, 0, 640, 360, None), ..gone };
        assert!(plan(&v, &with_crop).unwrap().unwrap().in_result(&keep).is_some_and(|p| p.masks.is_empty()));
    }

    /// A 구간 제거 runs masks and PiP on each kept piece before the join: each piece gets the part of
    /// every result time inside it, on its own clock from 0 — a mask across a join is split, a video's
    /// piece moves with its kept stretch, an image is cut like a mask — and a piece they miss gets none.
    #[test]
    fn each_piece_gets_its_own_masks_and_pip_on_its_own_clock() {
        let v = video(1920, 1080, None);
        let keep = [plan::Range::new(0.0, 2.0), plan::Range::new(3.5, 7.0), plan::Range::new(8.0, 10.0)];
        let e = Effects {
            crop: crop(0, 0, 1280, 720, None),
            masks: vec![mask(0, 0, 64, 64, 1.0, 9.0, MaskKind::Blur, 50)],
            overlays: vec![overlay(0, 0, 64, 64, 1.0, 9.0, 10.0), overlay(100, 100, 64, 64, 1.5, 5.0, 0.0)],
            ..Effects::default()
        };
        let mut p = plan(&v, &e).unwrap().unwrap();
        p.attach_sources(vec![source(OverlayKind::Video, Some(14.0), true), source(OverlayKind::Image, None, false)]);
        let p = p.in_result(&keep).unwrap();
        // Result: piece 0 is 0–2, piece 1 is 2–5.5, piece 2 is 5.5–7.5. The mask plays 1–6.5, the
        // video 1–2 (from 10 s) and 2–3.5 (from 12.5 s), the image 1.5–3.5.
        let first = p.in_piece(0.0, 2.0);
        assert_eq!(first.masks[0].times, vec![plan::Range::new(1.0, 2.0)]);
        assert_eq!(first.overlays[0].pieces, vec![OverlayPiece { start: 1.0, len: 1.0, source_offset: 10.0 }]);
        assert_eq!(first.overlays[0].times, vec![plan::Range::new(1.0, 2.0)]);
        assert_eq!(first.overlays[1].times, vec![plan::Range::new(1.5, 2.0)]);
        let second = p.in_piece(2.0, 3.5);
        assert_eq!(second.masks[0].times, vec![plan::Range::new(0.0, 3.5)]);
        assert_eq!(second.overlays[0].pieces, vec![OverlayPiece { start: 0.0, len: 1.5, source_offset: 12.5 }]);
        assert_eq!(second.overlays[1].times, vec![plan::Range::new(0.0, 1.5)]);
        // The last piece: only the mask's last second; the video and the image are over.
        let last = p.in_piece(5.5, 2.0);
        assert_eq!(last.masks[0].times, vec![plan::Range::new(0.0, 1.0)]);
        assert!(last.overlays.is_empty());
        // Crop · turn · size are the joined picture's, so they stay as they were.
        assert_eq!((last.crop, last.size), (p.crop, p.size));
        let none = p.in_piece(7.5, 1.0);
        assert!(none.masks.is_empty() && none.overlays.is_empty());
    }

    /// The shapes `src/lib/ipc.ts` sends.
    #[test]
    fn effects_from_the_ui() {
        let e: Effects = serde_json::from_str(
            r#"{"crop":{"x":1312,"y":0,"width":1216,"height":2160,"ratio":[9,16]},"turn":1,"flipH":true,"flipV":false,
                "size":{"kind":"exact","width":1080,"height":1350,"fit":"cover"}}"#,
        )
        .unwrap();
        assert_eq!(e.crop, crop(1312, 0, 1216, 2160, Some([9, 16])));
        assert_eq!((e.turn, e.flip_h, e.flip_v), (1, true, false));
        assert_eq!(e.size, OutputSize::Exact { width: 1080, height: 1350, fit: Fit::Cover });
        let short: Effects = serde_json::from_str(r#"{"size":{"kind":"short","short":720}}"#).unwrap();
        assert_eq!(short, Effects { size: OutputSize::Short { short: 720 }, ..Effects::default() });
        let none: Effects = serde_json::from_str("{}").unwrap();
        assert!(none.is_none());
        let free: Effects = serde_json::from_str(r#"{"crop":{"x":0,"y":0,"width":64,"height":64}}"#).unwrap();
        assert_eq!(free.crop.unwrap().ratio, None);
    }

    #[test]
    fn nothing_asked_or_nothing_changed_plans_no_filters() {
        let v = video(UHD.0, UHD.1, None);
        assert_eq!(plan(&v, &Effects::default()).unwrap(), None);
        let whole = Effects { crop: crop(0, 0, 3840, 2160, None), ..Effects::default() };
        assert_eq!(plan(&v, &whole).unwrap(), None, "a box over the whole picture");
        let up = Effects { size: OutputSize::Short { short: 2160 }, ..Effects::default() };
        assert_eq!(plan(&v, &up).unwrap(), None, "a preset never enlarges");
        let four = Effects { turn: 4, ..Effects::default() };
        assert!(four.is_none() && plan(&v, &four).unwrap().is_none());
        assert!(!Effects { masks: vec![mask(0, 0, 64, 64, 0.0, 1.0, MaskKind::Blur, 50)], ..Effects::default() }.is_none());
        let same = Effects { size: OutputSize::Exact { width: 3840, height: 2160, fit: Fit::Stretch }, ..Effects::default() };
        assert_eq!(plan(&v, &same).unwrap(), None, "직접 at the source size");
    }

    #[test]
    fn crop_is_kept_even_and_inside_the_picture() {
        let v = video(1920, 1080, None);
        let p = plan(&v, &Effects { crop: crop(101, 51, 641, 361, None), ..Effects::default() }).unwrap().unwrap();
        assert_eq!(p.crop, Some(Rect { x: 100, y: 50, width: 640, height: 360 }));
        assert_eq!((p.scale, p.pad, p.size), (None, None, [640, 360]));
        let outside = Effects { crop: crop(1300, 0, 640, 360, None), ..Effects::default() };
        assert_eq!(plan(&v, &outside).err(), Some(Msg::CropOutsidePicture { width: 1920, height: 1080 }));
        let small = Effects { crop: crop(0, 0, 62, 400, None), ..Effects::default() };
        assert_eq!(plan(&v, &small).err(), Some(Msg::CropTooSmall { min: MIN_SIDE }));
    }

    /// A phone video recorded upright is stored sideways (rotation −90): the box is in the upright
    /// picture, 1080 wide and 1920 tall.
    #[test]
    fn crop_is_in_the_upright_picture() {
        let v = video(1920, 1080, Some(-90.0));
        assert_eq!(v.upright_size(), (1080, 1920));
        let p = plan(&v, &Effects { crop: crop(0, 420, 1080, 1080, Some([1, 1])), ..Effects::default() }).unwrap().unwrap();
        assert_eq!((p.crop, p.size), (Some(Rect { x: 0, y: 420, width: 1080, height: 1080 }), [1080, 1080]));
        let wide = Effects { crop: crop(0, 0, 1920, 1080, None), ..Effects::default() };
        assert!(matches!(plan(&v, &wide), Err(Msg::CropOutsidePicture { width: 1080, height: 1920 })));
    }

    #[test]
    fn turns_and_flips_fold_and_turn_the_size() {
        let v = video(1920, 1080, None);
        let p = |turn: u8, flip_h: bool, flip_v: bool| plan(&v, &Effects { turn, flip_h, flip_v, ..Effects::default() }).unwrap().unwrap();
        let right = p(1, false, false);
        assert_eq!((right.turn, right.flip_h, right.flip_v, right.size, right.crop), (1, false, false, [1080, 1920], None));
        assert_eq!(p(3, true, false).turn, 3);
        assert_eq!(p(7, false, false).turn, 3, "wraps");
        let half = p(2, false, false);
        assert_eq!((half.turn, half.flip_h, half.flip_v, half.size), (0, true, true, [1920, 1080]), "a half turn is both flips");
        assert_eq!(plan(&v, &Effects { turn: 2, flip_h: true, flip_v: true, ..Effects::default() }).unwrap(), None, "and undoes them");
        let mirror = p(0, true, false);
        assert_eq!((mirror.turn, mirror.flip_h, mirror.flip_v), (0, true, false));
    }

    /// 해상도 / 출력 크기 presets: the short side, as `scale=-2:N` would — 일괄 변환 uses the same.
    #[test]
    fn short_side_presets_match_scale_minus_two_and_never_enlarge() {
        assert_eq!(short_side_size(3840, 2160, 1080), Some([1920, 1080]));
        assert_eq!(short_side_size(1080, 1920, 720), Some([720, 1280]));
        assert_eq!(short_side_size(1920, 1080, 480), Some([854, 480]), "853.3 → 854");
        assert_eq!(short_side_size(1918, 1080, 720), Some([1278, 720]), "1278.67 → 1278");
        assert_eq!(short_side_size(1920, 1080, 1080), None);
        assert_eq!(short_side_size(1280, 720, 1080), None);
        assert_eq!(short_side_size(0, 720, 480), None);
    }

    #[test]
    fn a_locked_ratio_lands_presets_on_the_ratio() {
        let v = video(UHD.0, UHD.1, None);
        let at = |c: Option<Crop>, short: u32| plan(&v, &Effects { crop: c, size: OutputSize::Short { short }, ..Effects::default() }).unwrap().unwrap();
        // 9:16 from 4K: 1215 is odd, the box is 1216 wide; 1080p is still 1080 × 1920.
        let p = at(crop(1312, 0, 1216, 2160, Some([9, 16])), 1080);
        assert_eq!((p.scale, p.size), (Some([1080, 1920]), [1080, 1920]));
        assert_eq!(at(crop(1312, 0, 1216, 2160, None), 1080).size, [1080, 1918], "자유: the box's own ratio");
        assert_eq!(at(crop(1312, 0, 1216, 2160, Some([9, 16])), 480).size, [480, 854]);
        // A ratio the box no longer has (more than 1 % off) is not forced on it.
        assert_eq!(at(crop(0, 0, 1600, 2160, Some([9, 16])), 1080).size, [1080, 1458]);
        // The ratio is as seen: a 16:9 box drawn on the turned picture is 1216 × 2160 in the source.
        let turned = Effects { crop: crop(1312, 0, 1216, 2160, Some([16, 9])), turn: 1, size: OutputSize::Short { short: 720 }, ..Effects::default() };
        let p = plan(&v, &turned).unwrap().unwrap();
        assert_eq!((p.turn, p.scale, p.size), (1, Some([1280, 720]), [1280, 720]));
    }

    #[test]
    fn exact_size_contain_cover_stretch() {
        let v = video(1920, 1080, None);
        let at = |c: Option<Crop>, turn: u8, width: u32, height: u32, fit: Fit| {
            plan(&v, &Effects { crop: c, turn, size: OutputSize::Exact { width, height, fit }, ..Effects::default() }).unwrap().unwrap()
        };
        // 맞춤: 1920 × 1080 into a square → 1080 × 608 (607.5) with bars above and below.
        let p = at(None, 0, 1080, 1080, Fit::Contain);
        assert_eq!((p.crop, p.scale, p.size), (None, Some([1080, 608]), [1080, 1080]));
        assert_eq!(p.pad, Some(Pad { width: 1080, height: 1080, x: 0, y: 236 }));
        // 맞춤 that only adds bars: the 608 × 1080 box needs no scaling in a square.
        let p = at(crop(656, 0, 608, 1080, None), 0, 1080, 1080, Fit::Contain);
        assert_eq!((p.scale, p.pad, p.size), (None, Some(Pad { width: 1080, height: 1080, x: 236, y: 0 }), [1080, 1080]));
        // 맞춤 without bars when the ratio is the same.
        let p = at(None, 0, 1280, 720, Fit::Contain);
        assert_eq!((p.scale, p.pad), (Some([1280, 720]), None));
        // 채움: the middle 1080 × 1080 of the picture, then scaled.
        let p = at(None, 0, 720, 720, Fit::Cover);
        assert_eq!((p.crop, p.scale, p.pad, p.size), (Some(Rect { x: 420, y: 0, width: 1080, height: 1080 }), Some([720, 720]), None, [720, 720]));
        // 채움 of a turned picture: 4:5 as seen is 5:4 in the source box.
        let p = at(crop(0, 0, 1920, 1080, None), 1, 1080, 1350, Fit::Cover);
        assert_eq!(p.crop, Some(Rect { x: 284, y: 0, width: 1350, height: 1080 }), "(1920 − 1350) / 2 = 285 → even");
        assert_eq!((p.scale, p.size), (None, [1080, 1350]), "1350 × 1080 turned is already the size");
        // 늘림.
        let p = at(None, 0, 1080, 1080, Fit::Stretch);
        assert_eq!((p.crop, p.scale, p.pad, p.size), (None, Some([1080, 1080]), None, [1080, 1080]));
        // 직접 may enlarge; odd sizes are made even; out of range is refused.
        assert_eq!(at(crop(0, 0, 640, 360, None), 0, 1921, 1081, Fit::Stretch).size, [1920, 1080]);
        let tiny = Effects { size: OutputSize::Exact { width: 32, height: 720, fit: Fit::Contain }, ..Effects::default() };
        assert_eq!(plan(&v, &tiny).err(), Some(Msg::OutputSizeInvalid { min: MIN_SIDE, max: MAX_SIDE }));
        let huge = Effects { size: OutputSize::Exact { width: 1280, height: 9000, fit: Fit::Contain }, ..Effects::default() };
        assert!(plan(&v, &huge).is_err());
        let bad_short = Effects { size: OutputSize::Short { short: 0 }, ..Effects::default() };
        assert!(plan(&v, &bad_short).is_err());
    }

    #[test]
    fn a_picture_of_unknown_size_takes_no_effects() {
        let v = video(0, 0, None);
        assert_eq!(plan(&v, &Effects { flip_h: true, ..Effects::default() }).err(), Some(Msg::PictureSizeUnknown));
        assert_eq!(plan(&v, &Effects::default()).unwrap(), None);
    }
}
