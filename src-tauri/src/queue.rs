//! 작업 큐: the outputs the user asked for, run a few at a time (1–4) through [`JobManager`].
//!
//! The queue keeps, for every item, how to run it ([`Work`]) — so a canceled or failed item can run
//! again — and the names it writes. No two items share a final or `.part` name: [`Queue::add`]
//! hands the names in use to the code that picks them (two exports of the same file would otherwise
//! write the same `.part`). Every change is sent as a whole snapshot and progress as its own event,
//! both while the queue is locked, so they arrive in the order they happened.

use crate::jobs::{Attempt, StepsJob, JobEvent, JobFinished, JobManager, JobProgress};
use crate::msg::Msg;
use crate::settings::Lang;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// Jobs that may run at once (the queue screen offers 1–4; 1 by default).
pub const MAX_CONCURRENCY: usize = 4;

pub fn clamp_concurrency(n: usize) -> usize {
    n.clamp(1, MAX_CONCURRENCY)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemState {
    Waiting,
    Running,
    Done,
    Failed,
    Canceled,
}

impl ItemState {
    pub fn finished(self) -> bool {
        matches!(self, ItemState::Done | ItemState::Failed | ItemState::Canceled)
    }
}

/// How an item runs; kept so it can run again after a cancel or a failure.
#[derive(Clone)]
pub enum Work {
    /// One FFmpeg run, or attempts in order until one succeeds (hardware, then software).
    Attempts { attempts: Vec<Attempt>, total_secs: f64 },
    /// Several steps (무손실 구간 제거). Built each time the item starts, since steps run once.
    Steps { build: Arc<dyn Fn() -> Result<StepsJob, Msg> + Send + Sync>, length: f64 },
}

/// What [`Queue::add`] gets from the code that planned the item.
pub struct Task<D> {
    /// What the UI shows about the item (the export's request and summary).
    pub info: D,
    pub program: PathBuf,
    pub work: Work,
    pub final_path: PathBuf,
    pub part: PathBuf,
    /// Every FFmpeg command, one per line, for the log view.
    pub command_line: String,
}

impl<D> Task<D> {
    /// The same task with other UI info (the app's queue holds exports and conversions alike).
    pub fn map_info<E>(self, f: impl FnOnce(D) -> E) -> Task<E> {
        let Task { info, program, work, final_path, part, command_line } = self;
        Task { info: f(info), program, work, final_path, part, command_line }
    }
}

/// One item as the UI sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem<D> {
    pub id: u64,
    pub info: D,
    pub state: ItemState,
    /// Running, and asked to stop: FFmpeg is stopping.
    pub canceling: bool,
    /// The name the result gets (held for this item since it was added); once done, the file made.
    pub output: String,
    /// Size of the result once done.
    pub output_bytes: Option<u64>,
    pub progress: Option<JobProgress>,
    /// An earlier attempt failed and the next one ran (why).
    pub note: Option<Msg>,
    pub error: Option<Msg>,
    pub command_line: String,
    /// End of FFmpeg's stderr ([`JobFinished::log`]).
    pub log: Option<String>,
    /// Unix time (ms) the item last started; None if it never ran.
    pub started_at: Option<u64>,
    pub elapsed_ms: Option<u64>,
    /// Larger = finished (done, failed or canceled) later.
    pub finished_order: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueSnapshot<D> {
    /// Grows with every change: a reply or event older than what the UI has can be dropped.
    pub version: u64,
    pub concurrency: usize,
    /// In queue order: waiting items run in this order.
    pub items: Vec<QueueItem<D>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemProgress {
    pub id: u64,
    pub progress: JobProgress,
}

#[derive(Debug, Clone)]
pub enum QueueEvent<D> {
    /// Anything but progress changed: items, states, order, concurrency.
    Changed(QueueSnapshot<D>),
    Progress(ItemProgress),
}


struct Entry<D> {
    item: QueueItem<D>,
    program: PathBuf,
    work: Work,
    final_path: PathBuf,
    part: PathBuf,
    /// JobManager id of the current run.
    job: Option<u64>,
}

struct Inner<D> {
    entries: Vec<Entry<D>>,
    concurrency: usize,
    next_id: u64,
    next_finish: u64,
    version: u64,
    closed: bool,
}

impl<D: Clone> Inner<D> {
    fn index(&self, id: u64) -> Result<usize, Msg> {
        self.entries.iter().position(|e| e.item.id == id).ok_or(Msg::QueueItemNotFound)
    }

    fn count(&self, state: ItemState) -> usize {
        self.entries.iter().filter(|e| e.item.state == state).count()
    }

    fn finish(&mut self, index: usize, state: ItemState) {
        self.next_finish += 1;
        let entry = &mut self.entries[index];
        entry.item.state = state;
        entry.item.canceling = false;
        entry.item.finished_order = Some(self.next_finish);
        entry.job = None;
    }

    fn snapshot(&self) -> QueueSnapshot<D> {
        QueueSnapshot {
            version: self.version,
            concurrency: self.concurrency,
            items: self.entries.iter().map(|e| e.item.clone()).collect(),
        }
    }
}

type Emit<D> = Box<dyn Fn(QueueEvent<D>) + Send + Sync>;

struct Shared<D> {
    inner: Mutex<Inner<D>>,
    jobs: Arc<JobManager>,
    emit: Emit<D>,
}

/// The queue; clones share it.
pub struct Queue<D>(Arc<Shared<D>>);

impl<D> Clone for Queue<D> {
    fn clone(&self) -> Self {
        Queue(self.0.clone())
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl<D: Clone + Serialize + Send + 'static> Queue<D> {
    /// `emit` gets every change and progress, called while the queue is locked (it must not call
    /// back into the queue).
    pub fn new(jobs: Arc<JobManager>, concurrency: usize, emit: impl Fn(QueueEvent<D>) + Send + Sync + 'static) -> Self {
        Queue(Arc::new(Shared {
            inner: Mutex::new(Inner {
                entries: Vec::new(),
                concurrency: clamp_concurrency(concurrency),
                next_id: 0,
                next_finish: 0,
                version: 0,
                closed: false,
            }),
            jobs,
            emit: Box::new(emit),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, Inner<D>> {
        // A panic in an event callback must not take the queue down with it.
        self.0.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn snapshot(&self) -> QueueSnapshot<D> {
        self.lock().snapshot()
    }

    /// (running, waiting): what quitting would cancel.
    pub fn active(&self) -> (usize, usize) {
        let inner = self.lock();
        (inner.count(ItemState::Running), inner.count(ItemState::Waiting))
    }

    /// Adds an item at the end. `make` plans it; it gets a test for the names other items hold
    /// (`command::output_paths_avoiding`) and runs while the queue is locked, so two adds never
    /// pick the same name. The item may already be running when this returns.
    pub fn add(&self, make: impl FnOnce(&dyn Fn(&Path) -> bool) -> Result<Task<D>, Msg>) -> Result<QueueItem<D>, Msg> {
        let mut inner = self.lock();
        if inner.closed {
            return Err(Msg::QueueClosed);
        }
        let task = {
            let entries = &inner.entries;
            let taken = |p: &Path| entries.iter().any(|e| e.final_path == p || e.part == p);
            make(&taken)?
        };
        inner.next_id += 1;
        let id = inner.next_id;
        inner.entries.push(Entry {
            item: QueueItem {
                id,
                info: task.info,
                state: ItemState::Waiting,
                canceling: false,
                output: task.final_path.display().to_string(),
                output_bytes: None,
                progress: None,
                note: None,
                error: None,
                command_line: task.command_line,
                log: None,
                started_at: None,
                elapsed_ms: None,
                finished_order: None,
            },
            program: task.program,
            work: task.work,
            final_path: task.final_path,
            part: task.part,
            job: None,
        });
        self.pump(&mut inner);
        self.changed(&mut inner);
        let index = inner.index(id)?;
        Ok(inner.entries[index].item.clone())
    }

    /// Waiting: canceled before it starts. Running: FFmpeg is asked to stop; the item turns canceled
    /// once its `.part` (and scratch folder) are gone.
    pub fn cancel(&self, id: u64) -> Result<(), Msg> {
        let mut inner = self.lock();
        let index = inner.index(id)?;
        match inner.entries[index].item.state {
            ItemState::Waiting => inner.finish(index, ItemState::Canceled),
            ItemState::Running => {
                let entry = &mut inner.entries[index];
                if entry.item.canceling {
                    return Ok(());
                }
                if let Some(job) = entry.job {
                    self.0.jobs.cancel(job);
                }
                entry.item.canceling = true;
            }
            _ => return Err(Msg::ItemFinished),
        }
        self.changed(&mut inner);
        Ok(())
    }

    /// "대기 작업 모두 취소": returns how many were waiting.
    pub fn cancel_waiting(&self) -> usize {
        let mut inner = self.lock();
        let waiting: Vec<usize> =
            (0..inner.entries.len()).filter(|&i| inner.entries[i].item.state == ItemState::Waiting).collect();
        for &i in &waiting {
            inner.finish(i, ItemState::Canceled);
        }
        if !waiting.is_empty() {
            self.changed(&mut inner);
        }
        waiting.len()
    }

    /// "대기열에서 빼기" (a waiting item) or takes a finished one off the record. A running item
    /// has to be canceled first. Its names are free again.
    pub fn remove(&self, id: u64) -> Result<(), Msg> {
        let mut inner = self.lock();
        let index = inner.index(id)?;
        if inner.entries[index].item.state == ItemState::Running {
            return Err(Msg::CancelRunningFirst);
        }
        inner.entries.remove(index);
        self.changed(&mut inner);
        Ok(())
    }

    /// "기록 지우기": takes every done, failed and canceled item off; returns how many.
    pub fn clear_finished(&self) -> usize {
        let mut inner = self.lock();
        let before = inner.entries.len();
        inner.entries.retain(|e| !e.item.state.finished());
        let removed = before - inner.entries.len();
        if removed > 0 {
            self.changed(&mut inner);
        }
        removed
    }

    /// Moves a waiting item to `position` among the waiting items (0 runs next; past the end: last).
    pub fn move_to(&self, id: u64, position: usize) -> Result<(), Msg> {
        let mut inner = self.lock();
        let from = inner.index(id)?;
        if inner.entries[from].item.state != ItemState::Waiting {
            return Err(Msg::OnlyWaitingMoves);
        }
        let entry = inner.entries.remove(from);
        let waiting: Vec<usize> =
            (0..inner.entries.len()).filter(|&i| inner.entries[i].item.state == ItemState::Waiting).collect();
        let to = match waiting.get(position) {
            Some(&i) => i,
            None => waiting.last().map_or(inner.entries.len(), |&i| i + 1),
        };
        inner.entries.insert(to, entry);
        self.changed(&mut inner);
        Ok(())
    }

    /// "다시 시작": a canceled or failed item waits again, at the end, with the same names.
    pub fn restart(&self, id: u64) -> Result<(), Msg> {
        let mut inner = self.lock();
        if inner.closed {
            return Err(Msg::QueueClosed);
        }
        let index = inner.index(id)?;
        if !matches!(inner.entries[index].item.state, ItemState::Canceled | ItemState::Failed) {
            return Err(Msg::OnlyStoppedRestart);
        }
        let mut entry = inner.entries.remove(index);
        let item = &mut entry.item;
        item.state = ItemState::Waiting;
        item.canceling = false;
        item.output = entry.final_path.display().to_string();
        item.output_bytes = None;
        item.progress = None;
        item.note = None;
        item.error = None;
        item.log = None;
        item.started_at = None;
        item.elapsed_ms = None;
        item.finished_order = None;
        inner.entries.push(entry);
        self.pump(&mut inner);
        self.changed(&mut inner);
        Ok(())
    }

    /// "동시 작업": 1–4. More waiting items start at once; fewer lets the running ones finish.
    pub fn set_concurrency(&self, n: usize) -> usize {
        let mut inner = self.lock();
        inner.concurrency = clamp_concurrency(n);
        self.pump(&mut inner);
        self.changed(&mut inner);
        inner.concurrency
    }

    /// App exit: nothing starts any more, waiting items are canceled, running ones are asked to stop.
    /// Returns how many were running; [`JobManager::wait_idle`] waits until they cleaned up.
    pub fn close(&self) -> usize {
        let mut inner = self.lock();
        inner.closed = true;
        let mut running = 0;
        for i in 0..inner.entries.len() {
            match inner.entries[i].item.state {
                ItemState::Waiting => inner.finish(i, ItemState::Canceled),
                ItemState::Running => {
                    running += 1;
                    let entry = &mut inner.entries[i];
                    if let (false, Some(job)) = (entry.item.canceling, entry.job) {
                        self.0.jobs.cancel(job);
                    }
                    entry.item.canceling = true;
                }
                _ => {}
            }
        }
        self.changed(&mut inner);
        running
    }

    fn changed(&self, inner: &mut Inner<D>) {
        inner.version += 1;
        (self.0.emit)(QueueEvent::Changed(inner.snapshot()));
    }

    /// Starts waiting items, in order, while fewer than the concurrency run.
    fn pump(&self, inner: &mut Inner<D>) {
        if inner.closed {
            return;
        }
        while inner.count(ItemState::Running) < inner.concurrency {
            let Some(index) = inner.entries.iter().position(|e| e.item.state == ItemState::Waiting) else { break };
            self.start(inner, index);
        }
    }

    /// Runs the item through the JobManager. Its events come back while the queue is still locked
    /// here at the earliest, so the job id is recorded before any event of that job is handled.
    fn start(&self, inner: &mut Inner<D>, index: usize) {
        let entry = &mut inner.entries[index];
        let id = entry.item.id;
        let queue = self.clone();
        let on_event = move |event: JobEvent| queue.on_job_event(id, event);
        entry.item.state = ItemState::Running;
        entry.item.started_at = Some(now_ms());
        let jobs = &self.0.jobs;
        let (part, final_path) = (entry.part.clone(), entry.final_path.clone());
        let started = match &entry.work {
            Work::Attempts { attempts, total_secs } => jobs
                .start_attempts(&entry.program, attempts.clone(), *total_secs, part, final_path, on_event)
                .map_err(Msg::launch),
            Work::Steps { build, length } => {
                build().map(|job| jobs.start_steps(&entry.program, job.steps, *length, job.scratch, part, final_path, on_event))
            }
        };
        match started {
            Ok(job) => entry.job = Some(job),
            Err(e) => {
                entry.item.error = Some(e);
                inner.finish(index, ItemState::Failed);
            }
        }
    }

    fn on_job_event(&self, id: u64, event: JobEvent) {
        let mut inner = self.lock();
        let Ok(index) = inner.index(id) else { return };
        match event {
            JobEvent::Progress(p) => {
                let entry = &mut inner.entries[index];
                if entry.job != Some(p.id) {
                    return;
                }
                if p.note.is_some() {
                    entry.item.note = p.note.clone();
                }
                entry.item.progress = Some(p.clone());
                (self.0.emit)(QueueEvent::Progress(ItemProgress { id, progress: p }));
            }
            JobEvent::Finished(f) => {
                if inner.entries[index].job != Some(f.id) {
                    return;
                }
                let state = apply_finished(&mut inner.entries[index].item, f);
                inner.finish(index, state);
                self.pump(&mut inner);
                self.changed(&mut inner);
            }
        }
    }
}

/// Copies a finished job's result into its item; returns the item's new state.
fn apply_finished<D>(item: &mut QueueItem<D>, f: JobFinished) -> ItemState {
    if f.note.is_some() {
        item.note = f.note;
    }
    item.log = f.log;
    item.elapsed_ms = Some(f.elapsed_ms);
    if f.ok {
        if let Some(out) = f.output {
            item.output_bytes = std::fs::metadata(&out).ok().map(|m| m.len());
            item.output = out;
        }
        ItemState::Done
    } else if f.canceled {
        ItemState::Canceled
    } else {
        item.error = Some(f.error.unwrap_or(Msg::FfmpegFailed));
        ItemState::Failed
    }
}

/// The native dialog before quitting while items run or wait — the one text Rust words itself (the
/// UI's dictionaries cannot reach a native dialog).
#[derive(Debug, Clone, PartialEq)]
pub struct QuitDialog {
    pub title: String,
    pub message: String,
    pub quit: String,
    pub keep: String,
}

pub fn quit_dialog(lang: Lang, running: usize, waiting: usize) -> QuitDialog {
    match lang {
        Lang::Ko => {
            let what = match (running, waiting) {
                (r, 0) => format!("진행 중인 작업 {r}개가"),
                (0, w) => format!("대기 중인 작업 {w}개가"),
                (r, w) => format!("진행 중인 작업 {r}개와 대기 중인 작업 {w}개가"),
            };
            QuitDialog {
                title: "작업이 진행 중입니다".into(),
                message: format!("{what} 취소됩니다. 만들던 파일은 지우고, 원본은 그대로 둡니다. 종료할까요?"),
                quit: "종료".into(),
                keep: "계속 작업".into(),
            }
        }
        Lang::En => {
            let jobs = |n: usize, kind: &str| format!("{n} {kind} job{}", if n == 1 { "" } else { "s" });
            let what = match (running, waiting) {
                (r, 0) => jobs(r, "running"),
                (0, w) => jobs(w, "waiting"),
                (r, w) => format!("{} and {}", jobs(r, "running"), jobs(w, "waiting")),
            };
            QuitDialog {
                title: "Jobs are in progress".into(),
                message: format!(
                    "{what} will be canceled. Unfinished files are deleted, and your originals stay as they are. Quit anyway?"
                ),
                quit: "Quit".into(),
                keep: "Keep Working".into(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command;
    use crate::jobs::Step;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;

    type Events = Receiver<QueueEvent<String>>;

    fn queue(concurrency: usize) -> (Queue<String>, Arc<JobManager>, Events) {
        let jobs = Arc::new(JobManager::default());
        let (tx, rx) = channel();
        let q = Queue::new(jobs.clone(), concurrency, move |e| {
            let _ = tx.send(e);
        });
        (q, jobs, rx)
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("godseon-queue-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A `/bin/sh` script stands in for FFmpeg; `script` gets the `.part` path it should write.
    fn sh_task(dir: &Path, name: &str, taken: &dyn Fn(&Path) -> bool, script: impl FnOnce(&Path) -> String) -> Task<String> {
        let input = dir.join(format!("{name}.mp4"));
        let (final_path, part) = command::output_paths_avoiding(&input, "_trim", None, taken);
        let args = vec!["-c".into(), script(&part).into()];
        Task {
            info: name.to_string(),
            program: "/bin/sh".into(),
            work: Work::Attempts { attempts: vec![Attempt { args, label: String::new() }], total_secs: 1.0 },
            final_path,
            part,
            command_line: format!("sh {name}"),
        }
    }

    /// Writes the result once `gate` exists.
    fn gated(gate: &Path) -> impl FnOnce(&Path) -> String {
        let gate = gate.display().to_string();
        move |part| format!("while [ ! -e '{gate}' ]; do sleep 0.02; done; printf ok > '{}'", part.display())
    }

    /// Writes some of the `.part`, then waits for `q` (a cancel) and fails, as FFmpeg does.
    fn until_canceled(part: &Path) -> String {
        format!("printf x > '{}'; read line; exit 1", part.display())
    }

    fn add(q: &Queue<String>, dir: &Path, name: &str, script: impl FnOnce(&Path) -> String) -> QueueItem<String> {
        q.add(|taken| Ok(sh_task(dir, name, taken, script))).unwrap()
    }

    /// The first snapshot that satisfies `done`; also returns every snapshot seen until then.
    fn wait_for(rx: &Events, done: impl Fn(&QueueSnapshot<String>) -> bool) -> (QueueSnapshot<String>, Vec<QueueSnapshot<String>>) {
        let mut seen = Vec::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(10)).expect("queue event") {
                QueueEvent::Changed(s) => {
                    seen.push(s.clone());
                    if done(&s) {
                        return (s, seen);
                    }
                }
                QueueEvent::Progress(_) => {}
            }
        }
    }

    fn state(s: &QueueSnapshot<String>, name: &str) -> ItemState {
        s.items.iter().find(|i| i.info == name).unwrap_or_else(|| panic!("no {name}")).state
    }

    /// For `wait_for`: early snapshots may not have the item yet.
    fn has(s: &QueueSnapshot<String>, name: &str, want: ItemState) -> bool {
        s.items.iter().any(|i| i.info == name && i.state == want)
    }

    fn states(s: &QueueSnapshot<String>) -> Vec<(String, ItemState)> {
        s.items.iter().map(|i| (i.info.clone(), i.state)).collect()
    }

    fn all_finished(s: &QueueSnapshot<String>) -> bool {
        s.items.iter().all(|i| i.state.finished())
    }

    fn part_files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".part.") || n.starts_with(".godseon-cut-"))
            .collect()
    }

    fn touch(p: &Path) {
        std::fs::write(p, b"").unwrap();
    }

    #[test]
    fn runs_waiting_items_in_order_up_to_the_concurrency() {
        if cfg!(windows) {
            return;
        }
        let dir = temp("order");
        let (q, _jobs, rx) = queue(1);
        let gates: Vec<PathBuf> = (0..3).map(|i| dir.join(format!("gate {i}"))).collect();
        for (name, gate) in ["제주 여행 A", "제주 여행 B", "제주 여행 C"].iter().zip(&gates) {
            add(&q, &dir, name, gated(gate));
        }
        let s = q.snapshot();
        assert_eq!(states(&s).iter().map(|x| x.1).collect::<Vec<_>>(), [ItemState::Running, ItemState::Waiting, ItemState::Waiting]);
        assert_eq!(q.active(), (1, 2));

        assert_eq!(q.set_concurrency(2), 2);
        assert_eq!((state(&q.snapshot(), "제주 여행 B"), state(&q.snapshot(), "제주 여행 C")), (ItemState::Running, ItemState::Waiting));
        touch(&gates[1]);
        let (s, _) = wait_for(&rx, |s| has(s, "제주 여행 C", ItemState::Running));
        assert_eq!(state(&s, "제주 여행 B"), ItemState::Done, "C starts once B is done");
        touch(&gates[0]);
        let (_, mut seen) = wait_for(&rx, |s| has(s, "제주 여행 A", ItemState::Done));
        touch(&gates[2]);
        let (s, rest) = wait_for(&rx, all_finished);
        seen.extend(rest);
        assert!(seen.iter().all(|s| s.items.iter().filter(|i| i.state == ItemState::Running).count() <= 2));
        for item in &s.items {
            assert_eq!(item.state, ItemState::Done, "{item:?}");
            assert_eq!(std::fs::read(&item.output).unwrap(), b"ok");
            assert_eq!(item.output_bytes, Some(2));
            assert!(item.started_at.is_some() && item.elapsed_ms.is_some() && item.error.is_none());
        }
        let order: Vec<&str> = {
            let mut done: Vec<&QueueItem<String>> = s.items.iter().collect();
            done.sort_by_key(|i| i.finished_order);
            done.iter().map(|i| i.info.as_str()).collect()
        };
        assert_eq!(order, ["제주 여행 B", "제주 여행 A", "제주 여행 C"]);
        assert!(part_files(&dir).is_empty());
        assert!(s.version > 0 && s.concurrency == 2);
        assert_eq!(clamp_concurrency(0), 1);
        assert_eq!(q.set_concurrency(9), MAX_CONCURRENCY);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancels_failures_and_restarts_end_in_the_right_state() {
        if cfg!(windows) {
            return;
        }
        let dir = temp("states");
        let (q, _jobs, rx) = queue(1);
        let gate = dir.join("gate");
        let marker = dir.join("C ran");
        let fails = add(&q, &dir, "고장난 클립", |part| {
            format!("printf x > '{}'; echo 'moov atom not found' >&2; exit 1", part.display())
        });
        let blocks = add(&q, &dir, "긴 인코딩", until_canceled);
        let m = marker.display().to_string();
        let g = gate.display().to_string();
        let waits = add(&q, &dir, "대기 클립", move |part| {
            format!("printf x > '{m}'; while [ ! -e '{g}' ]; do sleep 0.02; done; printf ok > '{}'", part.display())
        });

        let (s, _) = wait_for(&rx, |s| has(s, "긴 인코딩", ItemState::Running));
        let failed = s.items.iter().find(|i| i.id == fails.id).unwrap();
        assert_eq!(failed.state, ItemState::Failed);
        assert_eq!(failed.error, Some(Msg::raw("moov atom not found")));
        assert_eq!(failed.log.as_deref(), Some("moov atom not found"));
        assert!(!Path::new(&failed.output).exists());

        // Canceled while waiting: it never runs.
        q.cancel(waits.id).unwrap();
        let s = q.snapshot();
        let c = s.items.iter().find(|i| i.id == waits.id).unwrap();
        assert!(c.state == ItemState::Canceled && c.started_at.is_none());
        // Canceled while running: canceling, then canceled once FFmpeg (here sh) stopped.
        q.cancel(blocks.id).unwrap();
        assert!(q.snapshot().items.iter().find(|i| i.id == blocks.id).unwrap().canceling);
        let (s, _) = wait_for(&rx, all_finished);
        assert_eq!(state(&s, "긴 인코딩"), ItemState::Canceled);
        assert!(!marker.exists(), "an item canceled before it started never runs");
        assert!(part_files(&dir).is_empty(), "{:?}", part_files(&dir));

        assert!(q.cancel(fails.id).is_err(), "finished items cannot be canceled");
        q.restart(waits.id).unwrap();
        let (s, _) = wait_for(&rx, |s| has(s, "대기 클립", ItemState::Running));
        assert_eq!(s.items.last().unwrap().id, waits.id, "a restarted item goes to the end");
        assert!(q.remove(waits.id).is_err(), "a running item is canceled before it is removed");
        touch(&gate);
        let (s, _) = wait_for(&rx, all_finished);
        let c = s.items.iter().find(|i| i.id == waits.id).unwrap();
        assert!(c.state == ItemState::Done && c.output.ends_with("대기 클립_trim.mp4"), "the same name as before");

        q.restart(fails.id).unwrap();
        let (s, _) = wait_for(&rx, |s| s.items.iter().any(|i| i.id == fails.id && i.state == ItemState::Failed));
        assert_eq!(s.items.iter().find(|i| i.id == fails.id).unwrap().error, Some(Msg::raw("moov atom not found")));
        assert!(q.restart(waits.id).is_err(), "done items do not restart");
        assert!(part_files(&dir).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn retry_notes_and_logs_reach_the_item() {
        if cfg!(windows) {
            return;
        }
        let dir = temp("retry");
        let (q, _jobs, rx) = queue(1);
        q.add(|taken| {
            let mut t = sh_task(&dir, "하드웨어 실패", taken, |_| String::new());
            let p = t.part.display().to_string();
            t.work = Work::Attempts {
                attempts: vec![
                    Attempt { args: vec!["-c".into(), "echo 'Error while opening encoder' >&2; exit 1".into()], label: "VideoToolbox".into() },
                    Attempt { args: vec!["-c".into(), format!("printf 'out_time_us=500000\\nprogress=end\\n'; printf ok > '{p}'").into()], label: "libx265".into() },
                ],
                total_secs: 1.0,
            };
            Ok(t)
        })
        .unwrap();
        let mut progress = 0;
        let item = loop {
            match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                QueueEvent::Progress(p) => {
                    progress += 1;
                    assert!(p.progress.note.is_some());
                }
                QueueEvent::Changed(s) if all_finished(&s) => break s.items[0].clone(),
                QueueEvent::Changed(_) => {}
            }
        };
        assert_eq!(progress, 1);
        assert_eq!(item.state, ItemState::Done);
        assert_eq!(item.note, Some(crate::jobs::retry_note("VideoToolbox", "libx265", Some("Error while opening encoder"))));
        assert_eq!(item.log.as_deref(), Some("[VideoToolbox]\nError while opening encoder"));
        assert_eq!(item.progress.as_ref().map(|p| p.percent), Some(100.0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn waiting_items_reorder_and_leave_and_the_record_clears() {
        if cfg!(windows) {
            return;
        }
        let dir = temp("reorder");
        let (q, _jobs, rx) = queue(1);
        let gate = dir.join("gate");
        let names = ["첫 클립", "둘째", "셋째", "넷째"];
        let ids: Vec<u64> = names.iter().map(|n| add(&q, &dir, n, gated(&gate)).id).collect();
        let waiting = |q: &Queue<String>| -> Vec<String> {
            q.snapshot().items.into_iter().filter(|i| i.state == ItemState::Waiting).map(|i| i.info).collect()
        };
        q.move_to(ids[3], 0).unwrap();
        assert_eq!(waiting(&q), ["넷째", "둘째", "셋째"]);
        q.move_to(ids[1], 9).unwrap();
        assert_eq!(waiting(&q), ["넷째", "셋째", "둘째"]);
        q.move_to(ids[1], 1).unwrap();
        assert_eq!(waiting(&q), ["넷째", "둘째", "셋째"]);
        assert!(q.move_to(ids[0], 0).is_err(), "only waiting items move");
        q.remove(ids[2]).unwrap();
        assert_eq!(waiting(&q), ["넷째", "둘째"]);
        assert!(q.remove(ids[0]).is_err());
        assert!(q.remove(999).is_err());

        touch(&gate);
        let (s, seen) = wait_for(&rx, |s| s.items.len() == 3 && all_finished(s));
        let started: Vec<String> = {
            let mut order = Vec::new();
            for snap in &seen {
                for i in &snap.items {
                    if i.state == ItemState::Running && !order.contains(&i.info) {
                        order.push(i.info.clone());
                    }
                }
            }
            order
        };
        assert_eq!(started, ["첫 클립", "넷째", "둘째"], "waiting items run in the order they were moved to");
        assert!(s.items.iter().all(|i| i.state == ItemState::Done));
        assert_eq!(q.clear_finished(), 3);
        assert!(q.snapshot().items.is_empty());
        assert_eq!(q.cancel_waiting(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn items_in_the_queue_never_share_a_name() {
        if cfg!(windows) {
            return;
        }
        let dir = temp("names");
        let (q, _jobs, _rx) = queue(1);
        let gate = dir.join("gate");
        let a = add(&q, &dir, "클립 01", gated(&gate));
        let b = add(&q, &dir, "클립 01", gated(&gate));
        let c = add(&q, &dir, "클립 01", gated(&gate));
        let names: Vec<String> = [&a, &b, &c].iter().map(|i| Path::new(&i.output).file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["클립 01_trim.mp4", "클립 01_trim (1).mp4", "클립 01_trim (2).mp4"]);
        // A name is free again once its item leaves the queue.
        q.remove(b.id).unwrap();
        let d = add(&q, &dir, "클립 01", gated(&gate));
        assert!(d.output.ends_with("클립 01_trim (1).mp4"));
        assert_eq!(q.cancel_waiting(), 2);
        assert!(q.snapshot().items.iter().filter(|i| i.state == ItemState::Canceled).all(|i| i.started_at.is_none()));
        touch(&gate);
        let (s, _) = wait_for(&_rx, all_finished);
        assert_eq!(s.items.iter().find(|i| i.id == a.id).unwrap().state, ItemState::Done);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A lossless cut's steps are built (and its scratch folder made) only when the item starts,
    /// again for every restart, and the folder is gone once it ends.
    #[test]
    fn step_items_make_their_scratch_folder_only_while_they_run() {
        if cfg!(windows) {
            return;
        }
        let dir = temp("steps");
        let (q, _jobs, rx) = queue(1);
        let first_gate = dir.join("first gate");
        add(&q, &dir, "앞 작업", gated(&first_gate));
        let scratch = dir.join(".godseon-cut-test");
        // Absent: the piece waits for a cancel (`q`) as FFmpeg would; present: it is copied at once.
        let piece_gate = dir.join("piece gate");
        let builds = Arc::new(AtomicUsize::new(0));
        let item = q
            .add(|taken| {
                let input = dir.join("구간 제거 클립.mp4");
                let (final_path, part) = command::output_paths_avoiding(&input, "_cut", None, taken);
                let (scratch, gate, builds) = (scratch.clone(), piece_gate.clone(), builds.clone());
                let p = part.clone();
                let build = move || {
                    builds.fetch_add(1, Ordering::SeqCst);
                    let piece = scratch.join("piece-000.mp4");
                    let (dir_to_make, piece2, p2) = (scratch.clone(), piece.clone(), p.clone());
                    let g = gate.display().to_string();
                    let steps = vec![
                        Step {
                            args: Box::new(move || {
                                std::fs::create_dir(&dir_to_make).map_err(|e| Msg::TempFolderFailed { detail: e.to_string() })?;
                                Ok(vec!["-c".into(), format!("if [ -e '{g}' ]; then printf A > '{}'; else read line; exit 1; fi", piece.display()).into()])
                            }),
                            weight: 1.0,
                        },
                        Step { args: Box::new(move || Ok(vec!["-c".into(), format!("cat '{}' > '{}'", piece2.display(), p2.display()).into()])), weight: 1.0 },
                    ];
                    Ok(StepsJob { steps, scratch: scratch.clone(), command_line: String::new() })
                };
                Ok(Task {
                    info: "구간 제거".to_string(),
                    program: "/bin/sh".into(),
                    work: Work::Steps { build: Arc::new(build), length: 1.0 },
                    final_path,
                    part,
                    command_line: "sh pieces\nsh join".into(),
                })
            })
            .unwrap();
        assert_eq!(item.state, ItemState::Waiting);
        assert!(builds.load(Ordering::SeqCst) == 0 && !scratch.exists(), "nothing on disk while it waits");

        touch(&first_gate);
        let (_, _) = wait_for(&rx, |s| has(s, "구간 제거", ItemState::Running));
        while !scratch.exists() {
            std::thread::sleep(Duration::from_millis(10));
        }
        q.cancel(item.id).unwrap();
        let (_, _) = wait_for(&rx, |s| has(s, "구간 제거", ItemState::Canceled));
        assert!(!scratch.exists() && part_files(&dir).is_empty());

        touch(&piece_gate);
        q.restart(item.id).unwrap();
        let (s, _) = wait_for(&rx, |s| has(s, "구간 제거", ItemState::Done));
        assert_eq!(builds.load(Ordering::SeqCst), 2);
        let done = s.items.iter().find(|i| i.id == item.id).unwrap();
        assert_eq!(std::fs::read(&done.output).unwrap(), b"A");
        assert!(!scratch.exists() && part_files(&dir).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn closing_cancels_everything_and_cleans_up() {
        if cfg!(windows) {
            return;
        }
        let dir = temp("close");
        let (q, jobs, rx) = queue(2);
        for name in ["하나", "둘", "셋"] {
            add(&q, &dir, name, until_canceled);
        }
        let (_, _) = wait_for(&rx, |s| s.items.iter().filter(|i| i.state == ItemState::Running).count() == 2);
        while part_files(&dir).len() < 2 {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(q.active(), (2, 1));
        assert_eq!(q.close(), 2);
        assert!(jobs.wait_idle(Duration::from_secs(5)));
        let (s, _) = wait_for(&rx, all_finished);
        assert!(s.items.iter().all(|i| i.state == ItemState::Canceled), "{:?}", states(&s));
        assert!(part_files(&dir).is_empty(), "{:?}", part_files(&dir));
        assert_eq!(jobs.running_count(), 0);
        assert!(q.add(|taken| Ok(sh_task(&dir, "늦은 작업", taken, until_canceled))).is_err());
        assert!(q.restart(s.items[0].id).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn quit_dialog_names_what_is_lost_in_both_languages() {
        let ko = |r, w| quit_dialog(Lang::Ko, r, w).message;
        assert_eq!(ko(1, 0), "진행 중인 작업 1개가 취소됩니다. 만들던 파일은 지우고, 원본은 그대로 둡니다. 종료할까요?");
        assert!(ko(0, 3).starts_with("대기 중인 작업 3개가 취소됩니다."));
        assert!(ko(2, 5).starts_with("진행 중인 작업 2개와 대기 중인 작업 5개가 취소됩니다."));
        assert_eq!(
            quit_dialog(Lang::Ko, 1, 0),
            QuitDialog { title: "작업이 진행 중입니다".into(), message: ko(1, 0), quit: "종료".into(), keep: "계속 작업".into() }
        );
        let en = |r, w| quit_dialog(Lang::En, r, w).message;
        assert_eq!(
            en(1, 0),
            "1 running job will be canceled. Unfinished files are deleted, and your originals stay as they are. Quit anyway?"
        );
        assert!(en(0, 3).starts_with("3 waiting jobs will be canceled."));
        assert!(en(2, 1).starts_with("2 running jobs and 1 waiting job will be canceled."));
        assert_eq!((quit_dialog(Lang::En, 1, 1).quit.as_str(), quit_dialog(Lang::En, 1, 1).keep.as_str()), ("Quit", "Keep Working"));
    }
}
