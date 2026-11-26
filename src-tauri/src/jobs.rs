//! Runs FFmpeg jobs: progress from `-progress pipe:1`, cancel with `q`, `.part` file until success.
//! A job may have several attempts (a hardware encoder, then software): the next one runs only
//! when the previous one failed. Or several steps (a lossless cut: one stream copy per kept piece,
//! then the join): the next one runs only when the previous one succeeded.

use crate::command::{self, LosslessCutPlan};
use crate::msg::Msg;
use crate::probe::{self, VideoStream};
use crate::util::{last_line, new_command, run_capture};
use serde::Serialize;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    pub id: u64,
    pub percent: f64,
    pub out_time: f64,
    pub speed: Option<f64>,
    pub fps: Option<f64>,
    /// Set once an attempt failed and the job started over with the next one.
    pub note: Option<Msg>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobFinished {
    pub id: u64,
    pub ok: bool,
    pub canceled: bool,
    pub output: Option<String>,
    pub error: Option<Msg>,
    pub elapsed_ms: u64,
    /// As in [`JobProgress::note`]: an earlier attempt failed.
    pub note: Option<Msg>,
    /// The end of what FFmpeg wrote to stderr, for the log view (every attempt or step that wrote
    /// something, the last [`LOG_LINES`] lines). None: nothing was written.
    pub log: Option<String>,
}

impl JobFinished {
    fn new(id: u64) -> Self {
        JobFinished { id, ok: false, canceled: false, output: None, error: None, elapsed_ms: 0, note: None, log: None }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    Progress(JobProgress),
    Finished(JobFinished),
}

/// Parses FFmpeg's `-progress` key=value stream. One block ends with `progress=continue|end`.
#[derive(Debug, Default)]
pub struct ProgressParser {
    out_time_us: Option<i64>,
    speed: Option<f64>,
    fps: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressSnapshot {
    pub out_time: f64,
    pub speed: Option<f64>,
    pub fps: Option<f64>,
    pub end: bool,
}

impl ProgressParser {
    pub fn feed(&mut self, line: &str) -> Option<ProgressSnapshot> {
        let (key, value) = line.trim().split_once('=')?;
        let value = value.trim();
        match key {
            // out_time_ms is also microseconds (a long-standing FFmpeg quirk); prefer out_time_us.
            "out_time_us" | "out_time_ms" => {
                if let Ok(v) = value.parse::<i64>() {
                    if key == "out_time_us" || self.out_time_us.is_none() {
                        self.out_time_us = Some(v.max(0));
                    }
                }
            }
            "speed" => self.speed = value.trim_end_matches('x').parse().ok(),
            "fps" => self.fps = value.parse().ok().filter(|f: &f64| *f > 0.0),
            "progress" => {
                return Some(ProgressSnapshot {
                    out_time: self.out_time_us.unwrap_or(0) as f64 / 1_000_000.0,
                    speed: self.speed,
                    fps: self.fps,
                    end: value == "end",
                });
            }
            _ => {}
        }
        None
    }
}

pub fn percent(out_time: f64, total: f64, end: bool) -> f64 {
    if end {
        return 100.0;
    }
    if total <= 0.0 {
        return 0.0;
    }
    (out_time / total * 100.0).clamp(0.0, 99.9)
}

/// The stderr line worth showing when FFmpeg fails. Encoder libraries print their own banners
/// there whatever `-loglevel` says (SVT-AV1 `Svt[info]: …`, x265 `x265 [info]: …`).
pub fn error_line(stderr: &str) -> Option<String> {
    let noise = |l: &&str| {
        let l = l.trim_start();
        ["Svt[info]", "Svt[warn]", "x265 [info]", "x265 [warning]"].iter().any(|p| l.starts_with(p))
    };
    let kept: Vec<&str> = stderr.lines().filter(|l| !noise(l)).collect();
    last_line(&kept.join("\n"), 400)
}

/// Lines of stderr a finished job keeps for the log view.
pub const LOG_LINES: usize = 40;

/// The last `max_lines` non-blank lines of `text`, each cut at 400 characters; None if there are none.
pub fn stderr_tail(text: &str, max_lines: usize) -> Option<String> {
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    let keep = &lines[lines.len().saturating_sub(max_lines)..];
    let cut = |l: &&str| if l.chars().count() > 400 { format!("{}…", l.chars().take(400).collect::<String>()) } else { l.to_string() };
    Some(keep.iter().map(cut).collect::<Vec<_>>().join("\n"))
}

/// Adds one run's stderr to the job's log (`label` names the attempt when there are several).
fn push_log(log: &mut Vec<String>, label: Option<&str>, stderr: &str) {
    if let Some(tail) = stderr_tail(stderr, LOG_LINES) {
        log.push(match label {
            Some(l) if !l.is_empty() => format!("[{l}]\n{tail}"),
            _ => tail,
        });
    }
}

fn joined_log(log: &[String]) -> Option<String> {
    let text = log.join("\n");
    stderr_tail(&text, LOG_LINES)
}

/// "VideoToolbox 인코딩이 실패해 libx265(으)로 다시 시작했습니다: …"
pub fn retry_note(failed: &str, next: &str, reason: Option<&str>) -> Msg {
    Msg::RetriedWith { failed: failed.to_string(), next: next.to_string(), reason: reason.map(String::from) }
}

/// One way to run a job.
#[derive(Debug, Clone)]
pub struct Attempt {
    pub args: Vec<OsString>,
    /// Names the attempt in the retry note ("VideoToolbox"); unused for a job with one attempt.
    pub label: String,
}

/// One FFmpeg run of a job made of several steps. `args` is called right before the step runs, so it
/// can use what the steps before wrote (the concat list after the pieces); its Err ends the job.
pub struct Step {
    pub args: Box<dyn FnOnce() -> Result<Vec<OsString>, Msg> + Send>,
    /// Share of the progress bar, in seconds of media the step goes through.
    pub weight: f64,
}

struct Running {
    /// FFmpeg of the current attempt or step; None between two steps.
    child: Option<Arc<Mutex<Child>>>,
    stdin: Option<ChildStdin>,
    canceled: Arc<AtomicBool>,
}

struct Spawned {
    child: Arc<Mutex<Child>>,
    stdin: Option<ChildStdin>,
    stdout: ChildStdout,
    stderr: ChildStderr,
}

fn spawn(ffmpeg: &Path, args: &[OsString]) -> std::io::Result<Spawned> {
    let mut child = new_command(ffmpeg)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let stdin = child.stdin.take();
    Ok(Spawned { child: Arc::new(Mutex::new(child)), stdin, stdout, stderr })
}

#[derive(Default)]
pub struct JobManager {
    next_id: AtomicU64,
    running: Arc<Mutex<HashMap<u64, Running>>>,
}

impl JobManager {
    /// Starts FFmpeg writing to `part`; on success renames it to `final_path`.
    pub fn start<F>(
        &self,
        ffmpeg: &Path,
        args: Vec<OsString>,
        total_secs: f64,
        part: PathBuf,
        final_path: PathBuf,
        on_event: F,
    ) -> std::io::Result<u64>
    where
        F: Fn(JobEvent) + Send + Sync + 'static,
    {
        let attempts = vec![Attempt { args, label: String::new() }];
        self.start_attempts(ffmpeg, attempts, total_secs, part, final_path, on_event)
    }

    /// Like [`start`](Self::start), trying `attempts` in order until one succeeds. Every attempt
    /// writes to `part`; a failed attempt's file is removed before the next one starts, and a
    /// canceled job runs no further attempt. The job keeps one id; after a failure its progress
    /// starts over and carries a `note` saying why.
    pub fn start_attempts<F>(
        &self,
        ffmpeg: &Path,
        attempts: Vec<Attempt>,
        total_secs: f64,
        part: PathBuf,
        final_path: PathBuf,
        on_event: F,
    ) -> std::io::Result<u64>
    where
        F: Fn(JobEvent) + Send + Sync + 'static,
    {
        assert!(!attempts.is_empty(), "a job needs at least one attempt");
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let first = spawn(ffmpeg, &attempts[0].args)?;
        let canceled = Arc::new(AtomicBool::new(false));
        self.running
            .lock()
            .unwrap()
            .insert(id, Running { child: Some(first.child.clone()), stdin: first.stdin, canceled: canceled.clone() });

        let on_event = Arc::new(on_event);
        let started = Instant::now();
        let running = self.running.clone();
        let ffmpeg = ffmpeg.to_path_buf();
        thread::spawn(move || {
            let mut finished = JobFinished::new(id);
            let mut log = Vec::new();
            let several = attempts.len() > 1;
            let mut current = Some((first.child, first.stdout, first.stderr));
            let mut index = 0;
            while let Some((child, stdout, stderr)) = current.take() {
                let note = finished.note.clone();
                let progress = move |s: ProgressSnapshot| JobProgress {
                    id,
                    percent: percent(s.out_time, total_secs, s.end),
                    out_time: s.out_time,
                    speed: s.speed,
                    fps: s.fps,
                    note: note.clone(),
                };
                let (success, stderr_text) = run_to_end(child, stdout, stderr, progress, &on_event, &canceled);
                push_log(&mut log, several.then_some(attempts[index].label.as_str()), &stderr_text);
                let was_canceled = canceled.load(Ordering::SeqCst);
                if success && !was_canceled {
                    match finalize(&part, &final_path) {
                        Ok(p) => {
                            finished.ok = true;
                            finished.output = Some(p.display().to_string());
                        }
                        Err(e) => finished.error = Some(Msg::RenameFailed { detail: e.to_string() }),
                    }
                    break;
                }
                let _ = std::fs::remove_file(&part);
                if was_canceled {
                    finished.canceled = true;
                    break;
                }
                let reason = error_line(&stderr_text);
                let Some(next) = attempts.get(index + 1) else {
                    finished.error = Some(Msg::line_or(reason, Msg::FfmpegFailed));
                    break;
                };
                finished.note = Some(retry_note(&attempts[index].label, &next.label, reason.as_deref()));
                // Under the lock `cancel` takes, so a cancel between two attempts is never lost.
                let mut map = running.lock().unwrap();
                if canceled.load(Ordering::SeqCst) {
                    finished.canceled = true;
                    break;
                }
                match spawn(&ffmpeg, &next.args) {
                    Ok(s) => {
                        if let Some(job) = map.get_mut(&id) {
                            job.child = Some(s.child.clone());
                            job.stdin = s.stdin;
                        }
                        current = Some((s.child, s.stdout, s.stderr));
                        index += 1;
                    }
                    Err(e) => finished.error = Some(Msg::FfmpegRelaunchFailed { detail: e.to_string() }),
                }
            }
            running.lock().unwrap().remove(&id);
            finished.elapsed_ms = started.elapsed().as_millis() as u64;
            finished.log = joined_log(&log);
            on_event(JobEvent::Finished(finished));
        });
        Ok(id)
    }

    /// Runs `steps` one after another, each only once the one before succeeded; the last one writes
    /// `part`, renamed to `final_path` when it succeeds. `scratch` (the folder the steps write their
    /// intermediate files to) is removed when the job ends, whether it succeeded, failed or was
    /// canceled, and so is `part` unless it became the result. Progress adds up the steps by their
    /// `weight`; its `out_time` is that share of `length` (the result's length).
    pub fn start_steps<F>(
        &self,
        ffmpeg: &Path,
        steps: Vec<Step>,
        length: f64,
        scratch: PathBuf,
        part: PathBuf,
        final_path: PathBuf,
        on_event: F,
    ) -> u64
    where
        F: Fn(JobEvent) + Send + Sync + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let canceled = Arc::new(AtomicBool::new(false));
        self.running.lock().unwrap().insert(id, Running { child: None, stdin: None, canceled: canceled.clone() });
        let on_event = Arc::new(on_event);
        let started = Instant::now();
        let running = self.running.clone();
        let ffmpeg = ffmpeg.to_path_buf();
        thread::spawn(move || {
            let mut finished = JobFinished::new(id);
            let mut log = Vec::new();
            let total: f64 = steps.iter().map(|s| s.weight.max(0.0)).sum();
            let count = steps.len();
            let mut done = 0.0;
            let mut all_ran = true;
            for (index, step) in steps.into_iter().enumerate() {
                let weight = step.weight.max(0.0);
                if canceled.load(Ordering::SeqCst) {
                    finished.canceled = true;
                    all_ran = false;
                    break;
                }
                let args = match (step.args)() {
                    Ok(a) => a,
                    Err(e) => {
                        finished.error = Some(e);
                        all_ran = false;
                        break;
                    }
                };
                // Spawned under the lock `cancel` takes, so a cancel right now is never lost.
                let spawned = {
                    let mut map = running.lock().unwrap();
                    if canceled.load(Ordering::SeqCst) {
                        finished.canceled = true;
                        all_ran = false;
                        break;
                    }
                    match spawn(&ffmpeg, &args) {
                        Ok(s) => {
                            if let Some(job) = map.get_mut(&id) {
                                job.child = Some(s.child.clone());
                                job.stdin = s.stdin;
                            }
                            (s.child, s.stdout, s.stderr)
                        }
                        Err(e) => {
                            finished.error = Some(Msg::launch(e));
                            all_ran = false;
                            break;
                        }
                    }
                };
                let last = index + 1 == count;
                let progress = move |s: ProgressSnapshot| {
                    let current = if s.end { weight } else { s.out_time.clamp(0.0, weight) };
                    let share = if total > 0.0 { (done + current) / total } else { 0.0 };
                    JobProgress {
                        id,
                        percent: if last && s.end { 100.0 } else { (share * 100.0).min(99.9) },
                        out_time: share * length,
                        speed: s.speed,
                        fps: s.fps,
                        note: None,
                    }
                };
                let (child, stdout, stderr) = spawned;
                let (success, stderr_text) = run_to_end(child, stdout, stderr, progress, &on_event, &canceled);
                push_log(&mut log, None, &stderr_text);
                if let Some(job) = running.lock().unwrap().get_mut(&id) {
                    job.child = None;
                    job.stdin = None;
                }
                if canceled.load(Ordering::SeqCst) {
                    finished.canceled = true;
                    all_ran = false;
                    break;
                }
                if !success {
                    finished.error = Some(Msg::line_or(error_line(&stderr_text), Msg::FfmpegFailed));
                    all_ran = false;
                    break;
                }
                done += weight;
            }
            if all_ran {
                match finalize(&part, &final_path) {
                    Ok(p) => {
                        finished.ok = true;
                        finished.output = Some(p.display().to_string());
                    }
                    Err(e) => finished.error = Some(Msg::RenameFailed { detail: e.to_string() }),
                }
            }
            if !finished.ok {
                let _ = std::fs::remove_file(&part);
            }
            let _ = std::fs::remove_dir_all(&scratch);
            running.lock().unwrap().remove(&id);
            finished.elapsed_ms = started.elapsed().as_millis() as u64;
            finished.log = joined_log(&log);
            on_event(JobEvent::Finished(finished));
        });
        id
    }

    /// Asks FFmpeg to stop (`q` on stdin); kills it if it is still running after 3 seconds.
    pub fn cancel(&self, id: u64) -> bool {
        let mut map = self.running.lock().unwrap();
        let Some(job) = map.get_mut(&id) else { return false };
        job.canceled.store(true, Ordering::SeqCst);
        if let Some(mut stdin) = job.stdin.take() {
            let _ = stdin.write_all(b"q\n");
            let _ = stdin.flush();
        }
        let Some(child) = job.child.clone() else { return true };
        drop(map);
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if matches!(child.lock().unwrap().try_wait(), Ok(Some(_))) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
            let _ = child.lock().unwrap().kill();
        });
        true
    }

    /// Cancels every running job (app exit); returns how many there were.
    pub fn cancel_all(&self) -> usize {
        let ids: Vec<u64> = self.running.lock().unwrap().keys().copied().collect();
        ids.into_iter().filter(|id| self.cancel(*id)).count()
    }

    /// Waits until no job runs (each job removes its `.part` and scratch folder before it counts as
    /// ended), at most `timeout`; false if some still run.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.running.lock().unwrap().is_empty() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Jobs running now.
    pub fn running_count(&self) -> usize {
        self.running.lock().unwrap().len()
    }
}

/// Waits for one FFmpeg run while sending its progress (`progress` turns FFmpeg's numbers into the
/// job's); returns whether it succeeded, and its stderr.
///
/// FFmpeg writes `progress=end` whenever it stops — also on `q` (a cancel) and on an error — so that
/// block is held until FFmpeg exited and only counts as the end (100 %) if it succeeded and was not
/// asked to stop (`stopping`); otherwise it goes out as an ordinary block (Phase 3 step 6: canceled
/// items showed 100 % while they stopped and kept it in the record).
fn run_to_end<F, P>(
    child: Arc<Mutex<Child>>,
    stdout: ChildStdout,
    mut stderr: ChildStderr,
    progress: P,
    on_event: &Arc<F>,
    stopping: &AtomicBool,
) -> (bool, String)
where
    F: Fn(JobEvent) + Send + Sync + 'static,
    P: Fn(ProgressSnapshot) -> JobProgress + Send + 'static,
{
    let progress_cb = on_event.clone();
    let progress_reader = thread::spawn(move || {
        let mut parser = ProgressParser::default();
        let mut end = None;
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            match parser.feed(&line) {
                Some(s) if s.end => end = Some(s),
                Some(s) => progress_cb(JobEvent::Progress(progress(s))),
                None => {}
            }
        }
        (end, progress)
    });
    let stderr_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    });
    let status = loop {
        match child.lock().unwrap().try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        thread::sleep(Duration::from_millis(100));
    };
    let success = status.is_some_and(|s| s.success());
    if let Ok((Some(mut last), progress)) = progress_reader.join() {
        last.end = success && !stopping.load(Ordering::SeqCst);
        on_event(JobEvent::Progress(progress(last)));
    }
    let stderr_text = stderr_reader.join().unwrap_or_default();
    (success, stderr_text)
}

/// Renames the finished `.part` file; if the final name was taken meanwhile, picks a free one.
fn finalize(part: &Path, final_path: &Path) -> std::io::Result<PathBuf> {
    let mut target = final_path.to_path_buf();
    let mut n = 1;
    while target.exists() {
        let stem = final_path.file_stem().unwrap_or_default().to_string_lossy();
        let ext = final_path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        target = final_path.with_file_name(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::rename(part, &target)?;
    Ok(target)
}

// ---------------------------------------------------------------------------------------------
// Lossless 구간 제거 (command::plan_lossless_cut): the checks and steps of the job.

/// Refuses a lossless cut whose pieces after a join do not start at an IDR frame (or a shown AV1 key
/// frame): the decoder would lose pictures after the join (command.rs). One packet per piece.
/// Codecs `command::starts_a_sequence` does not know, and checks that could not run, pass.
pub fn check_joins(ffmpeg: &Path, input: &Path, video: &VideoStream, plan: &LosslessCutPlan) -> Result<(), Msg> {
    for piece in plan.pieces.iter().skip(1) {
        let args = command::random_access_check_args(input, video.index, piece.seek);
        let Ok(out) = run_capture(ffmpeg, &args, Duration::from_secs(30)) else { continue };
        if command::starts_a_sequence(&video.codec, &out.stderr) == Some(false) {
            return Err(Msg::OpenGopJoin { at: piece.visible_start });
        }
    }
    Ok(())
}

/// The steps of a lossless cut of more than one piece, ready for [`JobManager::start_steps`].
pub struct CutJob {
    pub steps: Vec<Step>,
    /// Created by the first step, right before it runs (a queued job leaves nothing on disk while it
    /// waits); the job removes it when it ends.
    pub scratch: PathBuf,
    /// Every FFmpeg command, one per line, for display.
    pub command_line: String,
}

/// One stream copy per piece into a scratch folder next to the result (`tag` names it), then the
/// join: the concat list is written once the pieces are there, with each piece's duration from its
/// packets (`command::concat_durations`). Progress weighs the copies and the join alike. Nothing is
/// written until the job runs, so this can also be called only to show the commands.
pub fn lossless_cut_job(
    ffmpeg: &Path,
    ffprobe: &Path,
    input: &Path,
    video_index: u32,
    plan: &LosslessCutPlan,
    final_path: &Path,
    part: &Path,
    tag: &str,
) -> Result<CutJob, Msg> {
    let extension = final_path.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let names = (0..plan.pieces.len()).map(|i| command::piece_name(i, &extension)).collect::<Result<Vec<_>, _>>()?;
    let scratch = command::cut_scratch_dir(final_path, tag);
    let list = scratch.join("list.txt");
    let mut steps = Vec::with_capacity(plan.pieces.len() + 1);
    let mut lines = Vec::with_capacity(plan.pieces.len() + 1);
    // (piece file, its name in the list, its length)
    let mut pieces: Vec<(PathBuf, String, f64)> = Vec::with_capacity(plan.pieces.len());
    for (index, (piece, name)) in plan.pieces.iter().zip(names).enumerate() {
        let path = scratch.join(&name);
        let args = command::lossless_piece_args(input, Some(video_index), piece, &path);
        lines.push(command::display_command(ffmpeg, &args));
        let folder = (index == 0).then(|| scratch.clone());
        steps.push(Step {
            args: Box::new(move || {
                if let Some(dir) = folder {
                    std::fs::create_dir(&dir).map_err(|e| Msg::TempFolderFailed { detail: e.to_string() })?;
                }
                Ok(args)
            }),
            weight: piece.duration,
        });
        pieces.push((path, name, piece.seek + piece.duration - piece.visible_start));
    }
    let concat = command::lossless_concat_args(&list, input, part);
    lines.push(command::display_command(ffmpeg, &concat));
    let ffprobe = ffprobe.to_path_buf();
    steps.push(Step {
        weight: plan.pieces.iter().map(|p| p.duration).sum(),
        args: Box::new(move || {
            let bounds = pieces
                .iter()
                .map(|(path, _, length)| probe::piece_bounds(&ffprobe, path, *length))
                .collect::<Result<Vec<_>, _>>()?;
            let entries: Vec<(String, f64)> =
                pieces.iter().map(|p| p.1.clone()).zip(command::concat_durations(&bounds)).collect();
            std::fs::write(&list, command::concat_list(&entries)).map_err(|e| Msg::ConcatListFailed { detail: e.to_string() })?;
            Ok(concat)
        }),
    });
    Ok(CutJob { steps, scratch, command_line: lines.join("\n") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_blocks() {
        let mut p = ProgressParser::default();
        let block = "frame=11160\nfps=93.00\nout_time_us=372000000\nout_time_ms=372000000\nout_time=00:06:12.000000\nspeed=3.10x\nprogress=continue\n";
        let mut snaps = block.lines().filter_map(|l| p.feed(l));
        let s = snaps.next().unwrap();
        assert_eq!(s.out_time, 372.0);
        assert_eq!(s.speed, Some(3.1));
        assert_eq!(s.fps, Some(93.0));
        assert!(!s.end);
        assert!((percent(s.out_time, 600.0, s.end) - 62.0).abs() < 1e-9);
    }

    #[test]
    fn handles_na_values_and_end() {
        let mut p = ProgressParser::default();
        for l in ["out_time_us=N/A", "speed=N/A", "fps=0.00"] {
            assert!(p.feed(l).is_none());
        }
        let s = p.feed("progress=end").unwrap();
        assert_eq!((s.out_time, s.speed, s.fps, s.end), (0.0, None, None, true));
        assert_eq!(percent(0.0, 600.0, true), 100.0);
        assert_eq!(percent(700.0, 600.0, false), 99.9, "never shows 100% before FFmpeg says end");
        assert_eq!(percent(5.0, 0.0, false), 0.0);
    }

    #[test]
    fn error_line_skips_encoder_banners() {
        let stderr = "Svt[info]: SVT [config]: preset 8\n[vost#0:0 @ 0x1] Error while opening encoder\nSvt[info]: -------\n\n";
        assert_eq!(error_line(stderr).as_deref(), Some("[vost#0:0 @ 0x1] Error while opening encoder"));
        assert_eq!(error_line("x265 [info]: HEVC encoder version 4.1\n"), None);
        assert_eq!(
            retry_note("VideoToolbox", "libx265", Some("Error while opening encoder")),
            Msg::RetriedWith { failed: "VideoToolbox".into(), next: "libx265".into(), reason: Some("Error while opening encoder".into()) }
        );
        assert_eq!(
            retry_note("NVENC", "libx265", None),
            Msg::RetriedWith { failed: "NVENC".into(), next: "libx265".into(), reason: None }
        );
    }

    #[test]
    fn stderr_tail_keeps_the_last_lines() {
        assert_eq!(stderr_tail("\n  \n", 3), None);
        let text = (1..=50).map(|i| format!("줄 {i}  ")).collect::<Vec<_>>().join("\n\n");
        let tail = stderr_tail(&text, 3).unwrap();
        assert_eq!(tail, "줄 48\n줄 49\n줄 50", "blank lines dropped, trailing spaces trimmed");
        let long = "가".repeat(500);
        assert_eq!(stderr_tail(&long, 1).unwrap().chars().count(), 401);
    }

    fn sh(script: String) -> Vec<OsString> {
        vec!["-c".into(), script.into()]
    }

    fn wait_finished(rx: &std::sync::mpsc::Receiver<JobEvent>, progress: &mut Vec<JobProgress>) -> JobFinished {
        loop {
            match rx.recv_timeout(Duration::from_secs(10)).expect("job event") {
                JobEvent::Progress(p) => progress.push(p),
                JobEvent::Finished(f) => return f,
            }
        }
    }

    /// A shell stands in for FFmpeg: the first attempt fails after writing to `.part`, the second succeeds.
    #[test]
    fn falls_back_to_the_next_attempt_under_the_same_id() {
        if cfg!(windows) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("godseon-attempts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("결과 클립.part.mp4");
        let fin = dir.join("결과 클립.mp4");
        let p = part.display();
        let attempts = vec![
            Attempt {
                args: sh(format!("printf x > '{p}'; echo 'Error while opening encoder' >&2; exit 1")),
                label: "VideoToolbox".into(),
            },
            Attempt {
                args: sh(format!(
                    "printf 'out_time_us=500000\\nprogress=continue\\n'; printf ok > '{p}'; printf 'progress=end\\n'"
                )),
                label: "libx265".into(),
            },
        ];
        let jobs = JobManager::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let id = jobs
            .start_attempts(Path::new("/bin/sh"), attempts, 1.0, part.clone(), fin.clone(), move |e| {
                let _ = tx.send(e);
            })
            .unwrap();
        let mut progress = Vec::new();
        let finished = wait_finished(&rx, &mut progress);
        let expected = retry_note("VideoToolbox", "libx265", Some("Error while opening encoder"));
        assert!(finished.ok && finished.id == id, "{finished:?}");
        assert_eq!(finished.note.as_ref(), Some(&expected));
        assert_eq!(progress.len(), 2);
        assert!(progress.iter().all(|p| p.id == id && p.note.as_ref() == Some(&expected)));
        assert_eq!(std::fs::read(&fin).unwrap(), b"ok");
        assert!(!part.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_last_attempt_failing_fails_the_job() {
        if cfg!(windows) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("godseon-attempts-fail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("a.part.mp4");
        let fin = dir.join("a.mp4");
        let attempts = vec![
            Attempt { args: sh("echo first >&2; exit 1".into()), label: "NVENC".into() },
            Attempt {
                args: sh(format!("printf x > '{}'; echo 'No space left on device' >&2; exit 1", part.display())),
                label: "libx265".into(),
            },
        ];
        let jobs = JobManager::default();
        let (tx, rx) = std::sync::mpsc::channel();
        jobs.start_attempts(Path::new("/bin/sh"), attempts, 1.0, part.clone(), fin.clone(), move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        let finished = wait_finished(&rx, &mut Vec::new());
        assert!(!finished.ok && !finished.canceled);
        assert_eq!(finished.error, Some(Msg::raw("No space left on device")));
        assert!(matches!(finished.note, Some(Msg::RetriedWith { ref failed, ref next, .. }) if failed == "NVENC" && next == "libx265"));
        assert_eq!(finished.log.as_deref(), Some("[NVENC]\nfirst\n[libx265]\nNo space left on device"), "both attempts, by name");
        assert!(!part.exists() && !fin.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Canceled during the first attempt: the job ends canceled and the fallback never runs.
    #[test]
    fn cancel_skips_the_remaining_attempts() {
        if cfg!(windows) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("godseon-attempts-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("a.part.mp4");
        let marker = dir.join("fallback ran");
        let attempts = vec![
            // Ends (with an error, as FFmpeg does after `q`) once it reads a line from stdin.
            Attempt { args: sh("read line; exit 1".into()), label: "VideoToolbox".into() },
            Attempt { args: sh(format!("printf x > '{}'", marker.display())), label: "libx265".into() },
        ];
        let jobs = JobManager::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let id = jobs
            .start_attempts(Path::new("/bin/sh"), attempts, 1.0, part.clone(), dir.join("a.mp4"), move |e| {
                let _ = tx.send(e);
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(jobs.cancel(id));
        let finished = wait_finished(&rx, &mut Vec::new());
        assert!(finished.canceled && !finished.ok && finished.note.is_none(), "{finished:?}");
        assert!(!marker.exists(), "the fallback must not run after a cancel");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// FFmpeg says `progress=end` when it stops on `q` or on an error too: only a run that finished
    /// reaches 100 %.
    #[test]
    fn only_a_finished_run_reports_the_end() {
        if cfg!(windows) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("godseon-progress-end-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let block = |us: u64, end: bool| format!("printf 'out_time_us={us}\\nprogress={}\\n'", if end { "end" } else { "continue" });
        let run = |name: &str, script: String, cancel: bool| -> (JobFinished, Vec<f64>) {
            let part = dir.join(format!("{name} 클립.part.mp4"));
            let script = script.replace("PART", &part.display().to_string());
            let jobs = JobManager::default();
            let (tx, rx) = std::sync::mpsc::channel();
            let id = jobs
                .start(Path::new("/bin/sh"), sh(script), 4.0, part.clone(), dir.join(format!("{name} 클립.mp4")), move |e| {
                    let _ = tx.send(e);
                })
                .unwrap();
            let mut progress = Vec::new();
            if cancel {
                match rx.recv_timeout(Duration::from_secs(10)).expect("first block") {
                    JobEvent::Progress(p) => progress.push(p),
                    JobEvent::Finished(f) => panic!("ended before the cancel: {f:?}"),
                }
                assert!(jobs.cancel(id));
            }
            let finished = wait_finished(&rx, &mut progress);
            assert!(!part.exists());
            (finished, progress.iter().map(|p| p.percent).collect())
        };
        let (f, p) = run("끝까지", format!("{}; printf ok > 'PART'; {}", block(2_000_000, false), block(4_000_000, true)), false);
        assert!(f.ok, "{f:?}");
        assert_eq!(p, [50.0, 100.0], "finished: the last block is the end");
        // Stops on `q` at 1 s and says `end` (and exits 0, as FFmpeg does after `q`).
        let (f, p) = run("취소", format!("{}; read line; printf x > 'PART'; {}", block(500_000, false), block(1_000_000, true)), true);
        assert!(f.canceled && !f.ok, "{f:?}");
        assert_eq!(p, [12.5, 25.0], "canceled: never 100 %");
        let (f, p) = run("실패", format!("{}; echo 'Conversion failed!' >&2; exit 1", block(3_000_000, true)), false);
        assert!(!f.ok && !f.canceled, "{f:?}");
        assert_eq!(p, [75.0], "failed: never 100 %");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finalize_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("godseon-finalize-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("a_trim.part.mp4");
        let fin = dir.join("a_trim.mp4");
        std::fs::write(&fin, b"old").unwrap();
        std::fs::write(&part, b"new").unwrap();
        let out = finalize(&part, &fin).unwrap();
        assert_eq!(out.file_name().unwrap(), "a_trim (1).mp4");
        assert_eq!(std::fs::read(&fin).unwrap(), b"old");
        assert!(!part.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn step(script: String, weight: f64) -> Step {
        Step { args: Box::new(move || Ok(sh(script))), weight }
    }

    /// Two pieces, then a join that reads them: progress adds up, the scratch folder goes away.
    #[test]
    fn steps_run_in_order_and_clean_up() {
        if cfg!(windows) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("godseon-steps-{}", std::process::id()));
        let scratch = dir.join(".godseon-cut-1");
        std::fs::create_dir_all(&scratch).unwrap();
        let (part, fin) = (dir.join("제주 여행_cut.part.mp4"), dir.join("제주 여행_cut.mp4"));
        let (a, b) = (scratch.join("piece-000.mp4"), scratch.join("piece-001.mp4"));
        let progress = |us: u64| format!("printf 'out_time_us={us}\\nprogress=continue\\n'");
        let (a2, b2, p2) = (a.clone(), b.clone(), part.clone());
        let steps = vec![
            step(format!("{}; printf A > '{}'; printf 'progress=end\\n'", progress(1_000_000), a.display()), 2.0),
            step(format!("{}; printf B > '{}'", progress(5_000_000), b.display()), 2.0),
            // Built when it runs: both pieces are there by then.
            Step {
                args: Box::new(move || {
                    assert!(a2.exists() && b2.exists(), "the join runs after the pieces");
                    Ok(sh(format!("{}; cat '{}' '{}' > '{}'; printf 'progress=end\\n'", progress(2_000_000), a2.display(), b2.display(), p2.display())))
                }),
                weight: 4.0,
            },
        ];
        let jobs = JobManager::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let id = jobs.start_steps(Path::new("/bin/sh"), steps, 3.0, scratch.clone(), part.clone(), fin.clone(), move |e| {
            let _ = tx.send(e);
        });
        let mut progress = Vec::new();
        let finished = wait_finished(&rx, &mut progress);
        assert!(finished.ok && finished.id == id, "{finished:?}");
        assert_eq!(std::fs::read(&fin).unwrap(), b"AB");
        assert!(!part.exists() && !scratch.exists(), "the scratch folder is removed");
        let percents: Vec<f64> = progress.iter().map(|p| p.percent).collect();
        // 1 of 8 · step end 2 of 8 · second step capped at its weight 4 of 8 · 6 of 8 · the end.
        assert_eq!(percents, vec![12.5, 25.0, 50.0, 75.0, 100.0]);
        assert!((progress[2].out_time - 1.5).abs() < 1e-9, "out_time is the share of the result's length");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_or_refused_step_ends_the_job_and_cleans_up() {
        if cfg!(windows) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("godseon-steps-fail-{}", std::process::id()));
        let scratch = dir.join(".godseon-cut-2");
        std::fs::create_dir_all(&scratch).unwrap();
        let (part, fin) = (dir.join("a.part.mkv"), dir.join("a.mkv"));
        let marker = dir.join("join ran");
        let piece = scratch.join("piece-000.mkv");
        let run = |steps: Vec<Step>| {
            let jobs = JobManager::default();
            let (tx, rx) = std::sync::mpsc::channel();
            jobs.start_steps(Path::new("/bin/sh"), steps, 1.0, scratch.clone(), part.clone(), fin.clone(), move |e| {
                let _ = tx.send(e);
            });
            wait_finished(&rx, &mut Vec::new())
        };
        let f = run(vec![
            step(format!("printf x > '{}'; echo 'No space left on device' >&2; exit 1", piece.display()), 1.0),
            step(format!("printf x > '{}'", marker.display()), 1.0),
        ]);
        assert!(!f.ok && !f.canceled && f.error == Some(Msg::raw("No space left on device")), "{f:?}");
        assert!(!marker.exists() && !scratch.exists() && !part.exists() && !fin.exists());

        std::fs::create_dir_all(&scratch).unwrap();
        let f = run(vec![
            step(format!("printf x > '{}'", piece.display()), 1.0),
            Step { args: Box::new(|| Err(Msg::ConcatListFailed { detail: "테스트".into() })), weight: 1.0 },
        ]);
        assert_eq!(f.error, Some(Msg::ConcatListFailed { detail: "테스트".into() }));
        assert!(!scratch.exists() && !part.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Canceled during the first piece: nothing after it runs, and nothing is left behind.
    #[test]
    fn cancel_during_a_step_skips_the_rest() {
        if cfg!(windows) {
            return;
        }
        let dir = std::env::temp_dir().join(format!("godseon-steps-cancel-{}", std::process::id()));
        let scratch = dir.join(".godseon-cut-3");
        std::fs::create_dir_all(&scratch).unwrap();
        let (part, marker) = (dir.join("a.part.mp4"), dir.join("join ran"));
        let piece = scratch.join("piece-000.mp4");
        let steps = vec![
            step(format!("printf x > '{}'; read line; exit 1", piece.display()), 1.0),
            step(format!("printf x > '{}'", marker.display()), 1.0),
        ];
        let jobs = JobManager::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let id = jobs.start_steps(Path::new("/bin/sh"), steps, 1.0, scratch.clone(), part.clone(), dir.join("a.mp4"), move |e| {
            let _ = tx.send(e);
        });
        std::thread::sleep(Duration::from_millis(300));
        assert!(jobs.cancel(id));
        let f = wait_finished(&rx, &mut Vec::new());
        assert!(f.canceled && !f.ok, "{f:?}");
        assert!(!marker.exists() && !scratch.exists() && !part.exists());
        assert!(!jobs.cancel(id), "the job is gone");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
