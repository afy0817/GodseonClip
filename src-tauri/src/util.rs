//! Small process helpers shared by the FFmpeg modules.

use std::ffi::OsStr;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Builds a `Command` that never opens a console window on Windows.
pub fn new_command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[derive(Debug, Clone)]
pub struct Captured {
    pub success: bool,
    pub timed_out: bool,
    pub stdout: String,
    /// Exact stdout bytes, for binary output such as raw PCM.
    pub stdout_raw: Vec<u8>,
    pub stderr: String,
}

/// Runs a program to completion (or until `timeout`) and captures its output.
/// Arguments go straight to the process — no shell, so quoting and non-ASCII paths are safe.
pub fn run_capture<S: AsRef<OsStr>>(
    program: &Path,
    args: &[S],
    timeout: Duration,
) -> std::io::Result<Captured> {
    let mut raw = Vec::new();
    let mut captured = run_streaming(program, args, timeout, |chunk| raw.extend_from_slice(chunk))?;
    captured.stdout = String::from_utf8_lossy(&raw).into_owned();
    captured.stdout_raw = raw;
    Ok(captured)
}

/// Like [`run_capture`], but hands stdout to `on_chunk` as it arrives instead of keeping it
/// (the returned `stdout` fields stay empty). For long binary output such as decoded audio.
pub fn run_streaming<S: AsRef<OsStr>>(
    program: &Path,
    args: &[S],
    timeout: Duration,
    mut on_chunk: impl FnMut(&[u8]),
) -> std::io::Result<Captured> {
    let mut child = new_command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut out = child.stdout.take().expect("stdout is piped");
    let mut err = child.stderr.take().expect("stderr is piped");
    let err_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err.read_to_end(&mut buf);
        buf
    });

    // stdout is read on this thread (on_chunk need not be Send); a watchdog enforces the timeout.
    let child = Arc::new(Mutex::new(child));
    let done = Arc::new(AtomicBool::new(false));
    let timed_out = Arc::new(AtomicBool::new(false));
    let watchdog = {
        let (child, done, timed_out) = (child.clone(), done.clone(), timed_out.clone());
        thread::spawn(move || {
            let deadline = Instant::now() + timeout;
            while !done.load(Ordering::SeqCst) {
                if Instant::now() >= deadline {
                    timed_out.store(true, Ordering::SeqCst);
                    let _ = child.lock().unwrap().kill();
                    return;
                }
                thread::sleep(Duration::from_millis(15));
            }
        })
    };

    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match out.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => on_chunk(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    // Poll instead of wait(): the watchdog needs the lock to kill a process that closed stdout but hangs.
    let status = loop {
        match child.lock().unwrap().try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(e) => break Err(e),
        }
        thread::sleep(Duration::from_millis(15));
    };
    done.store(true, Ordering::SeqCst);
    let _ = watchdog.join();
    let status = status?;
    let timed_out = timed_out.load(Ordering::SeqCst);

    let stderr = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned();
    Ok(Captured {
        success: status.success() && !timed_out,
        timed_out,
        stdout: String::new(),
        stdout_raw: Vec::new(),
        stderr,
    })
}

/// Last non-empty line of a tool's stderr, shortened for display.
pub fn last_line(text: &str, max_chars: usize) -> Option<String> {
    let line = text.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
    let mut s: String = line.chars().take(max_chars).collect();
    if line.chars().count() > max_chars {
        s.push('…');
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_line_skips_blank_lines_and_truncates() {
        assert_eq!(last_line("a\nb\n\n  \n", 10).as_deref(), Some("b"));
        assert_eq!(last_line("", 10), None);
        assert_eq!(last_line("abcdefghij-k", 5).as_deref(), Some("abcde…"));
    }
}
