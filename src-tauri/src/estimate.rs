//! 예상 크기 (Phase 3 step 3, canvas Convert.dc.html): how big a file of the 일괄 변환 list will be,
//! from a test encode with the conversion's own settings — not from the bitrate.
//!
//! The test converts three stretches of 10 s (around 1/6, 1/2 and 5/6 of the file, so an intro or a
//! still opening does not decide alone; a file of [`WHOLE_FILE_UP_TO`] s or less is tried whole) and
//! scales their size to the whole length. One file is tried at a time, never while the 작업 큐 runs
//! an item (a test that is running then is canceled and tried again later), and a test that is no
//! longer wanted (the settings or the list changed) is canceled. Results are kept per file and
//! settings while the app runs. The stretches are written as `.part` files to a scratch folder of
//! the app cache, measured and removed; the folder is emptied when the app starts and quits.

use crate::convert::Prepared;
use crate::jobs::{Attempt, JobEvent, JobManager};
use crate::msg::Msg;
use crate::plan::Range;
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

/// Stretches tried in a longer file.
pub const SAMPLE_COUNT: usize = 3;
/// Length of each stretch, in seconds.
pub const SAMPLE_SECS: f64 = 10.0;
/// Files up to this long are tried whole: three stretches would cost about as much, and the
/// result is then the conversion's own size.
pub const WHOLE_FILE_UP_TO: f64 = 45.0;

/// The stretches to try for a file `duration` s long: the whole file when short, else
/// [`SAMPLE_COUNT`] × [`SAMPLE_SECS`] centered on the middles of its thirds.
pub fn sample_windows(duration: f64) -> Vec<Range> {
    if !(duration.is_finite() && duration > 0.0) {
        return Vec::new();
    }
    if duration <= WHOLE_FILE_UP_TO {
        return vec![Range::new(0.0, duration)];
    }
    (0..SAMPLE_COUNT)
        .map(|k| {
            let center = duration * (2 * k + 1) as f64 / (2 * SAMPLE_COUNT) as f64;
            let start = (center - SAMPLE_SECS / 2.0).clamp(0.0, duration - SAMPLE_SECS);
            Range::new(start, start + SAMPLE_SECS)
        })
        .collect()
}

/// (seconds tried, bytes made) per stretch → the whole file's size: all the bytes over all the
/// seconds, times the length. Also the size if the whole file were like its smallest and its
/// largest stretch, for how much the content varies. None without anything tried.
pub fn estimate_bytes(tried: &[(f64, u64)], duration: f64) -> Option<(u64, u64, u64)> {
    let secs: f64 = tried.iter().map(|t| t.0).sum();
    if tried.is_empty() || secs <= 0.0 || !(duration > 0.0) {
        return None;
    }
    let bytes: u64 = tried.iter().map(|t| t.1).sum();
    let at = |rate: f64| (rate * duration).round() as u64;
    let rates = tried.iter().filter(|t| t.0 > 0.0).map(|t| t.1 as f64 / t.0);
    let low = rates.clone().fold(f64::INFINITY, f64::min);
    let high = rates.fold(0.0, f64::max);
    Some((at(bytes as f64 / secs), at(low), at(high)))
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum EstimateState {
    /// Waits for its turn (or for the queue to finish).
    Waiting,
    /// Being tried: share of all its stretches.
    Running { percent: f64 },
    Done {
        bytes: u64,
        /// The size if the whole file were like the smallest · largest stretch.
        low: u64,
        high: u64,
        /// Encoder the test ran with (the software one when the hardware one failed).
        encoder: String,
        /// The whole file was tried (a short file): the conversion's size.
        whole: bool,
    },
    Failed { error: Msg },
}

/// One file of the list with the settings asked for.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EstimateEntry {
    pub path: String,
    /// Names the file's state and settings; events for an older key are stale. Empty when the file
    /// could not be planned.
    pub key: String,
    #[serde(flatten)]
    pub state: EstimateState,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EstimateSnapshot {
    /// The queue runs an item: nothing is tried until it is done.
    pub paused: bool,
    /// One per path asked for, in that order.
    pub entries: Vec<EstimateEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EstimateEvent {
    Entry(EstimateEntry),
    Paused(bool),
}

/// The attempts for one stretch written to the path given; `true` = the software encoder alone.
pub type AttemptsFn = Box<dyn Fn(Range, &Path, bool) -> Result<Vec<Attempt>, Msg> + Send>;

/// One file to try.
pub struct Want {
    pub path: String,
    pub key: String,
    pub program: PathBuf,
    pub duration: f64,
    /// Of the files the test writes (the conversion's container).
    pub extension: String,
    /// The first encoder's name, and the software one (used once the first failed).
    pub labels: (String, Option<String>),
    pub attempts: AttemptsFn,
}

/// File size and modification time: a file changed or replaced is tried again.
fn file_stamp(path: &str) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("{}:{modified}", meta.len()))
}

/// The key of a planned file: the file as it is now and the conversion's settings.
pub fn key_for(path: &str, prepared: &Prepared) -> Option<String> {
    Some(format!("{}|{}", file_stamp(path)?, prepared.settings_key()))
}

/// What the estimator needs to try `prepared` (planned by `convert::prepare`).
pub fn want_for(path: &str, prepared: Prepared) -> Result<Want, Msg> {
    let key = key_for(path, &prepared).ok_or(Msg::FileNotFound { path: None })?;
    Ok(Want {
        path: path.to_string(),
        key,
        program: prepared.program().to_path_buf(),
        duration: prepared.duration(),
        extension: prepared.container().extension().to_string(),
        labels: prepared.encoder_labels(),
        attempts: Box::new(move |sample, output, software_only| prepared.sample_attempts(sample, output, software_only)),
    })
}

/// Empties the scratch folder (app start and exit: a crash may have left a stretch behind).
pub fn clean(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

struct Current {
    want: Want,
    windows: Vec<Range>,
    /// The stretch being tried.
    index: usize,
    /// (seconds, bytes) of the stretches done.
    tried: Vec<(f64, u64)>,
    /// The hardware encoder failed on an earlier stretch.
    software_only: bool,
    job: Option<u64>,
    percent: f64,
    /// Canceled because no one wants it any more: dropped once FFmpeg stopped.
    dropped: bool,
    /// Canceled for the queue: waits again, first in line.
    paused: bool,
}

#[derive(Default)]
struct State {
    /// Waiting, in the order asked for.
    waiting: VecDeque<Want>,
    /// Done or failed, by (path, key).
    results: HashMap<(String, String), EstimateState>,
    current: Option<Current>,
    busy: bool,
    closed: bool,
    next_file: u64,
}

type Emit = Box<dyn Fn(EstimateEvent) + Send + Sync>;

struct Shared {
    state: Mutex<State>,
    jobs: Arc<JobManager>,
    dir: PathBuf,
    emit: Emit,
}

/// The test encodes; clones share it.
#[derive(Clone)]
pub struct Estimator(Arc<Shared>);

impl Estimator {
    /// `dir`: the scratch folder (made when the first test runs). `emit` is called while the
    /// estimator is locked (it must not call back into it).
    pub fn new(jobs: Arc<JobManager>, dir: PathBuf, emit: impl Fn(EstimateEvent) + Send + Sync + 'static) -> Self {
        Estimator(Arc::new(Shared { state: Mutex::default(), jobs, dir, emit: Box::new(emit) }))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn entry(path: &str, key: &str, state: EstimateState) -> EstimateEntry {
        EstimateEntry { path: path.to_string(), key: key.to_string(), state }
    }

    fn send(&self, path: &str, key: &str, state: EstimateState) {
        (self.0.emit)(EstimateEvent::Entry(Self::entry(path, key, state)));
    }

    /// The files to try now, in order (an Err: the file cannot be planned, and why). Results already
    /// known come back as they are; a test no file here needs any more is canceled. The reply is
    /// the state once the next test started, so an event sent meanwhile is never newer than it.
    pub fn want(&self, files: Vec<Result<Want, (String, Msg)>>) -> EstimateSnapshot {
        let mut st = self.lock();
        // (path, key, error): the order of the reply.
        let mut order: Vec<(String, String, Option<Msg>)> = Vec::with_capacity(files.len());
        let mut waiting = VecDeque::new();
        let mut wanted: HashSet<(String, String)> = HashSet::new();
        let running = st.current.as_ref().filter(|c| !c.dropped).map(|c| (c.want.path.clone(), c.want.key.clone()));
        for file in files {
            match file {
                Err((path, error)) => order.push((path, String::new(), Some(error))),
                Ok(w) => {
                    let id = (w.path.clone(), w.key.clone());
                    if !wanted.insert(id.clone()) {
                        continue;
                    }
                    order.push((w.path.clone(), w.key.clone(), None));
                    if !st.results.contains_key(&id) && running.as_ref() != Some(&id) {
                        waiting.push_back(w);
                    }
                }
            }
        }
        st.waiting = waiting;
        if let Some(c) = st.current.as_mut() {
            if !c.dropped && !wanted.contains(&(c.want.path.clone(), c.want.key.clone())) {
                c.dropped = true;
                if let Some(job) = c.job {
                    self.0.jobs.cancel(job);
                }
            }
        }
        self.pump(&mut st);
        let entries = order
            .into_iter()
            .map(|(path, key, error)| {
                let state = if let Some(error) = error {
                    EstimateState::Failed { error }
                } else if let Some(known) = st.results.get(&(path.clone(), key.clone())) {
                    known.clone()
                } else {
                    match st.current.as_ref().filter(|c| !c.dropped && !c.paused && c.want.path == path && c.want.key == key) {
                        Some(c) => EstimateState::Running { percent: c.percent },
                        None => EstimateState::Waiting,
                    }
                };
                Self::entry(&path, &key, state)
            })
            .collect();
        EstimateSnapshot { paused: st.busy, entries }
    }

    /// The 예상 크기 measured for `path` with `key`, if done.
    pub fn bytes(&self, path: &str, key: &str) -> Option<u64> {
        match self.lock().results.get(&(path.to_string(), key.to_string())) {
            Some(EstimateState::Done { bytes, .. }) => Some(*bytes),
            _ => None,
        }
    }

    /// The 작업 큐 runs an item (`true`) or none: tests pause, and a running one is canceled to be
    /// tried again once the queue is done.
    pub fn set_busy(&self, busy: bool) {
        let mut st = self.lock();
        if st.busy == busy {
            return;
        }
        st.busy = busy;
        (self.0.emit)(EstimateEvent::Paused(busy));
        if busy {
            if let Some(c) = st.current.as_mut().filter(|c| !c.dropped && !c.paused) {
                c.paused = true;
                if let Some(job) = c.job {
                    self.0.jobs.cancel(job);
                }
                let (path, key) = (c.want.path.clone(), c.want.key.clone());
                self.send(&path, &key, EstimateState::Waiting);
            }
        } else {
            self.pump(&mut st);
        }
    }

    /// App exit: nothing more starts and the running test is canceled (the JobManager waits for it).
    pub fn close(&self) {
        let mut st = self.lock();
        st.closed = true;
        st.waiting.clear();
        if let Some(c) = st.current.as_mut() {
            c.dropped = true;
            if let Some(job) = c.job {
                self.0.jobs.cancel(job);
            }
        }
    }

    /// Starts the next file while nothing runs and the queue is idle.
    fn pump(&self, st: &mut State) {
        while !st.busy && !st.closed && st.current.is_none() {
            let Some(want) = st.waiting.pop_front() else { return };
            let windows = sample_windows(want.duration);
            if windows.is_empty() {
                self.fail(st, &want, Msg::EstimateNeedsDuration);
                continue;
            }
            let current = Current { want, windows, index: 0, tried: Vec::new(), software_only: false, job: None, percent: 0.0, dropped: false, paused: false };
            st.current = Some(current);
            self.start_stretch(st);
        }
    }

    fn fail(&self, st: &mut State, want: &Want, error: Msg) {
        let state = EstimateState::Failed { error };
        st.results.insert((want.path.clone(), want.key.clone()), state.clone());
        self.send(&want.path, &want.key, state);
    }

    /// Runs the current file's next stretch; a file whose stretch cannot start fails.
    fn start_stretch(&self, st: &mut State) {
        let Some(c) = st.current.as_mut() else { return };
        st.next_file += 1;
        let n = st.next_file;
        let ext = &c.want.extension;
        let part = self.0.dir.join(format!("sample-{n}.part.{ext}"));
        let done = self.0.dir.join(format!("sample-{n}.{ext}"));
        let window = c.windows[c.index];
        let started = std::fs::create_dir_all(&self.0.dir)
            .map_err(|e| Msg::EstimateFolderFailed { detail: e.to_string() })
            .and_then(|()| (c.want.attempts)(window, &part, c.software_only))
            .and_then(|attempts| {
                let me = self.clone();
                self.0
                    .jobs
                    .start_attempts(&c.want.program, attempts, window.len(), part, done, move |e| me.on_job_event(e))
                    .map_err(Msg::launch)
            });
        match started {
            Ok(job) => {
                c.job = Some(job);
                let (path, key, percent) = (c.want.path.clone(), c.want.key.clone(), c.percent);
                self.send(&path, &key, EstimateState::Running { percent });
            }
            Err(e) => {
                let c = st.current.take().expect("current");
                self.fail(st, &c.want, e);
            }
        }
    }

    fn on_job_event(&self, event: JobEvent) {
        let mut st = self.lock();
        let id = match &event {
            JobEvent::Progress(p) => p.id,
            JobEvent::Finished(f) => f.id,
        };
        let Some(c) = st.current.as_mut().filter(|c| c.job == Some(id)) else {
            // A stretch of a test dropped meanwhile: only its file to remove.
            if let JobEvent::Finished(f) = &event {
                if let Some(out) = &f.output {
                    let _ = std::fs::remove_file(out);
                }
            }
            return;
        };
        match event {
            JobEvent::Progress(p) => {
                if c.dropped || c.paused {
                    return;
                }
                c.percent = ((c.index as f64 + p.percent / 100.0) / c.windows.len() as f64 * 100.0).clamp(0.0, 99.9);
                let (path, key, percent) = (c.want.path.clone(), c.want.key.clone(), c.percent);
                self.send(&path, &key, EstimateState::Running { percent });
            }
            JobEvent::Finished(f) => {
                let bytes = f.output.as_ref().and_then(|out| {
                    let len = std::fs::metadata(out).ok().map(|m| m.len());
                    let _ = std::fs::remove_file(out);
                    len
                });
                let mut c = st.current.take().expect("current");
                if c.dropped || st.closed {
                    self.pump(&mut st);
                    return;
                }
                if c.paused || f.canceled {
                    // Tried again from its first stretch once the queue is done.
                    let (path, key) = (c.want.path.clone(), c.want.key.clone());
                    st.waiting.push_front(c.want);
                    self.send(&path, &key, EstimateState::Waiting);
                    self.pump(&mut st);
                    return;
                }
                let Some(bytes) = bytes.filter(|_| f.ok) else {
                    let error = Msg::SampleFailed { why: f.error.map(Box::new) };
                    self.fail(&mut st, &c.want, error);
                    self.pump(&mut st);
                    return;
                };
                if f.note.is_some() {
                    c.software_only = true;
                }
                c.tried.push((c.windows[c.index].len(), bytes));
                c.index += 1;
                if c.index < c.windows.len() {
                    st.current = Some(c);
                    self.start_stretch(&mut st);
                    self.pump(&mut st);
                    return;
                }
                let whole = c.windows.len() == 1 && c.windows[0].start <= 0.0 && c.windows[0].end >= c.want.duration;
                let encoder = match (&c.want.labels, c.software_only) {
                    ((_, Some(software)), true) => software.clone(),
                    ((first, _), _) => first.clone(),
                };
                let state = match estimate_bytes(&c.tried, c.want.duration) {
                    Some((bytes, low, high)) => EstimateState::Done { bytes, low, high, encoder, whole },
                    None => EstimateState::Failed { error: Msg::SampleEmpty },
                };
                st.results.insert((c.want.path.clone(), c.want.key.clone()), state.clone());
                self.send(&c.want.path, &c.want.key, state);
                self.pump(&mut st);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::mpsc::{channel, Receiver};
    use std::time::Duration;

    #[test]
    fn stretches_spread_over_the_file_or_the_whole_short_file() {
        assert!(sample_windows(0.0).is_empty() && sample_windows(f64::NAN).is_empty());
        assert_eq!(sample_windows(12.5), vec![Range::new(0.0, 12.5)]);
        assert_eq!(sample_windows(45.0), vec![Range::new(0.0, 45.0)]);
        // 10 minutes: around 1:40, 5:00 and 8:20.
        assert_eq!(sample_windows(600.0), vec![Range::new(95.0, 105.0), Range::new(295.0, 305.0), Range::new(495.0, 505.0)]);
        // Just over the limit: inside the file, not overlapping.
        let w = sample_windows(46.0);
        assert_eq!(w.len(), 3);
        assert!(w.iter().all(|r| r.start >= 0.0 && r.end <= 46.0 && (r.len() - SAMPLE_SECS).abs() < 1e-9), "{w:?}");
        assert!(w.windows(2).all(|p| p[0].end <= p[1].start), "{w:?}");
    }

    #[test]
    fn the_size_is_the_stretches_rate_over_the_whole_length() {
        assert_eq!(estimate_bytes(&[], 600.0), None);
        assert_eq!(estimate_bytes(&[(10.0, 1_000)], 0.0), None);
        // 1 MB, 2 MB and 3 MB per 10 s → 200 kB/s on average, 100–300 kB/s per stretch.
        assert_eq!(estimate_bytes(&[(10.0, 1_000_000), (10.0, 2_000_000), (10.0, 3_000_000)], 600.0), Some((120_000_000, 60_000_000, 180_000_000)));
        // A short file tried whole: its own size.
        assert_eq!(estimate_bytes(&[(12.5, 4_321)], 12.5), Some((4_321, 4_321, 4_321)));
    }

    /// A shell stands in for FFmpeg (as in jobs.rs): each stretch writes `bytes` bytes to its `.part`,
    /// after `delay` seconds; `fail_first` makes the hardware attempt fail once; `block` waits for `q`.
    #[derive(Clone, Copy)]
    struct Fake {
        bytes: u64,
        delay: f64,
        fail_first: bool,
        block: bool,
    }

    fn sh(script: String) -> Vec<OsString> {
        vec!["-c".into(), script.into()]
    }

    fn want(path: &str, key: &str, duration: f64, fake: Fake, calls: Arc<Mutex<Vec<(Range, bool)>>>) -> Want {
        Want {
            path: path.into(),
            key: key.into(),
            program: PathBuf::from("/bin/sh"),
            duration,
            extension: "mp4".into(),
            labels: ("VideoToolbox".into(), Some("libx265".into())),
            attempts: Box::new(move |range, out, software_only| {
                calls.lock().unwrap().push((range, software_only));
                let o = out.display();
                let write = if fake.block {
                    format!("printf x > '{o}'; read line; exit 1")
                } else {
                    format!("sleep {}; printf 'out_time_us=5000000\\nprogress=continue\\n'; head -c {} /dev/zero > '{o}'; printf 'progress=end\\n'", fake.delay, fake.bytes)
                };
                let software = Attempt { args: sh(write), label: "libx265".into() };
                if software_only {
                    return Ok(vec![software]);
                }
                let hardware = if fake.fail_first {
                    Attempt { args: sh(format!("printf x > '{o}'; echo 'Error while opening encoder' >&2; exit 1")), label: "VideoToolbox".into() }
                } else {
                    Attempt { label: "VideoToolbox".into(), ..software.clone() }
                };
                Ok(vec![hardware, software])
            }),
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("godseon 예상 크기 {tag} {}", std::process::id()));
        clean(&dir);
        dir
    }

    fn estimator(dir: &Path) -> (Estimator, Receiver<EstimateEvent>) {
        let (tx, rx) = channel();
        let tx = Mutex::new(tx);
        let e = Estimator::new(Arc::new(JobManager::default()), dir.to_path_buf(), move |ev| {
            let _ = tx.lock().unwrap().send(ev);
        });
        (e, rx)
    }

    /// Waits for `path`'s next done or failed state.
    fn result_of(rx: &Receiver<EstimateEvent>, path: &str) -> EstimateState {
        loop {
            match rx.recv_timeout(Duration::from_secs(10)).expect("estimate event") {
                EstimateEvent::Entry(e) if e.path == path && matches!(e.state, EstimateState::Done { .. } | EstimateState::Failed { .. }) => return e.state,
                _ => {}
            }
        }
    }

    fn left_in(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
    }

    #[test]
    fn tries_three_stretches_one_file_at_a_time_and_keeps_the_result() {
        if cfg!(windows) {
            return;
        }
        let dir = scratch("세 곳");
        let (e, rx) = estimator(&dir);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let fake = Fake { bytes: 1000, delay: 0.0, fail_first: false, block: false };
        let snap = e.want(vec![
            Ok(want("/영상/긴 클립.mp4", "k1", 600.0, fake, calls.clone())),
            Ok(want("/영상/짧은 클립.mp4", "k1", 20.0, Fake { bytes: 4321, ..fake }, calls.clone())),
            Err(("/영상/소리만.m4a".into(), Msg::ConvertNeedsVideo)),
        ]);
        assert!(!snap.paused);
        assert_eq!(snap.entries.len(), 3);
        assert_eq!(snap.entries[2], Estimator::entry("/영상/소리만.m4a", "", EstimateState::Failed { error: Msg::ConvertNeedsVideo }));
        let long = result_of(&rx, "/영상/긴 클립.mp4");
        assert_eq!(long, EstimateState::Done { bytes: 60_000, low: 60_000, high: 60_000, encoder: "VideoToolbox".into(), whole: false });
        let short = result_of(&rx, "/영상/짧은 클립.mp4");
        assert_eq!(short, EstimateState::Done { bytes: 4321, low: 4321, high: 4321, encoder: "VideoToolbox".into(), whole: true });
        let ranges: Vec<Range> = calls.lock().unwrap().iter().map(|c| c.0).collect();
        assert_eq!(ranges, [Range::new(95.0, 105.0), Range::new(295.0, 305.0), Range::new(495.0, 505.0), Range::new(0.0, 20.0)]);
        assert_eq!(e.bytes("/영상/긴 클립.mp4", "k1"), Some(60_000));
        assert_eq!(e.bytes("/영상/긴 클립.mp4", "k2"), None, "other settings");
        assert!(left_in(&dir).is_empty(), "every stretch removed: {:?}", left_in(&dir));

        // Asked again: known at once, nothing runs.
        let again = e.want(vec![Ok(want("/영상/긴 클립.mp4", "k1", 600.0, fake, calls.clone()))]);
        assert_eq!(again.entries[0].state, long);
        assert_eq!(calls.lock().unwrap().len(), 4);
        clean(&dir);
    }

    #[test]
    fn after_the_hardware_encoder_fails_the_other_stretches_use_software() {
        if cfg!(windows) {
            return;
        }
        let dir = scratch("하드웨어 실패");
        let (e, rx) = estimator(&dir);
        let calls = Arc::new(Mutex::new(Vec::new()));
        e.want(vec![Ok(want("/영상/a.mov", "k", 120.0, Fake { bytes: 500, delay: 0.0, fail_first: true, block: false }, calls.clone()))]);
        let state = result_of(&rx, "/영상/a.mov");
        assert!(matches!(&state, EstimateState::Done { encoder, bytes: 6000, .. } if encoder == "libx265"), "{state:?}");
        let software: Vec<bool> = calls.lock().unwrap().iter().map(|c| c.1).collect();
        assert_eq!(software, [false, true, true], "hardware → software on the first stretch, software after");
        assert!(left_in(&dir).is_empty(), "{:?}", left_in(&dir));
        clean(&dir);
    }

    #[test]
    fn waits_while_the_queue_runs_and_tries_again_after_it() {
        if cfg!(windows) {
            return;
        }
        let dir = scratch("큐 쉼");
        let (e, rx) = estimator(&dir);
        let calls = Arc::new(Mutex::new(Vec::new()));
        e.set_busy(true);
        let fake = Fake { bytes: 100, delay: 0.3, fail_first: false, block: false };
        let snap = e.want(vec![Ok(want("/영상/b.mkv", "k", 60.0, fake, calls.clone()))]);
        assert!(snap.paused);
        assert_eq!(snap.entries[0].state, EstimateState::Waiting);
        std::thread::sleep(Duration::from_millis(300));
        assert!(calls.lock().unwrap().is_empty(), "nothing tried while the queue runs");

        // The queue is done: the test runs; the queue starts again during it → canceled, later again.
        e.set_busy(false);
        while calls.lock().unwrap().is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        e.set_busy(true);
        std::thread::sleep(Duration::from_millis(800));
        let tried = calls.lock().unwrap().len();
        e.set_busy(false);
        // 100 bytes per 10 s stretch → 600 bytes for 60 s.
        assert_eq!(result_of(&rx, "/영상/b.mkv"), EstimateState::Done { bytes: 600, low: 600, high: 600, encoder: "VideoToolbox".into(), whole: false });
        assert!(calls.lock().unwrap().len() >= tried + 3, "all three stretches again after the pause");
        assert_eq!(calls.lock().unwrap()[tried].0, Range::new(5.0, 15.0), "from the first stretch");
        assert!(left_in(&dir).is_empty(), "{:?}", left_in(&dir));
        clean(&dir);
    }

    #[test]
    fn a_test_no_longer_wanted_is_canceled_and_leaves_nothing() {
        if cfg!(windows) {
            return;
        }
        let dir = scratch("취소");
        let (e, rx) = estimator(&dir);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let blocking = Fake { bytes: 0, delay: 0.0, fail_first: false, block: true };
        e.want(vec![Ok(want("/영상/c.mp4", "high", 300.0, blocking, calls.clone()))]);
        while calls.lock().unwrap().is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(200));
        // The quality changed: the old test stops, the new one runs.
        let quick = Fake { bytes: 10, delay: 0.0, fail_first: false, block: false };
        let snap = e.want(vec![Ok(want("/영상/c.mp4", "small", 300.0, quick, calls.clone()))]);
        assert_eq!(snap.entries[0].state, EstimateState::Waiting);
        assert!(matches!(result_of(&rx, "/영상/c.mp4"), EstimateState::Done { bytes: 300, .. }));
        assert_eq!(e.bytes("/영상/c.mp4", "high"), None, "the canceled test keeps no result");

        // The list cleared while a test runs, then the app quits: nothing left in the folder.
        e.want(vec![Ok(want("/영상/d.mp4", "k", 300.0, blocking, calls.clone()))]);
        std::thread::sleep(Duration::from_millis(300));
        e.want(Vec::new());
        e.close();
        assert!(e.0.jobs.wait_idle(Duration::from_secs(5)));
        assert!(left_in(&dir).is_empty(), "{:?}", left_in(&dir));
        assert_eq!(e.bytes("/영상/d.mp4", "k"), None);
        clean(&dir);
    }
}
