//! Filesystem watcher — keeps Termory's session / memory / skill view
//! in sync with on-disk changes so the user never has to click
//! "refresh". When a watched path changes, we coalesce a burst of
//! events into a single re-scan and emit the result to the frontend
//! via a Tauri event.
//!
//! Scope is intentionally simple in this phase: watch each platform's
//! top-level config dir recursively, re-run the full `scan_sessions`,
//! and push the entire `Vec<AppSession>` back. A future optimization
//! would dispatch per-source incremental updates, but until that's
//! actually a bottleneck the whole-result push is simpler and correct.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter};

use crate::sessions::scan_sessions;

/// Event name fired at the frontend after a successful re-scan.
/// Payload is `Vec<AppSession>` (same shape as `scan_all_sessions`).
pub const SOURCES_CHANGED_EVENT: &str = "termory:sources-changed";

/// Coalesce changes that arrive within this window before triggering
/// a re-scan. Many editors / DB engines emit a flurry of intermediate
/// events on save (temp file → rename → mtime touch + WAL writes for
/// SQLite); 500ms collects the burst without making the UI feel laggy.
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(500);

/// After we've finished a re-scan, drain any events that arrive within
/// this settle window. Reading the SQLite databases (Codex's
/// `state_5.sqlite`, OpenCode's `opencode.db`) touches the `-wal` and
/// `-shm` sidecar files even on a pure read, which the watcher sees as
/// new modifications. Without this drain we'd immediately re-trigger
/// ourselves and loop indefinitely.
const SETTLE_WINDOW: Duration = Duration::from_millis(300);

/// Start the filesystem watcher in a background thread. Returns once
/// the watch handles are registered; the actual event loop runs
/// forever in the spawned thread.
pub fn start(app_handle: AppHandle) -> notify::Result<()> {
    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();

    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
        // Send may fail if the receiver thread has died; that's fine,
        // we'll silently stop forwarding.
        let _ = tx.send(res);
    })?;

    for path in watch_targets() {
        if !path.exists() {
            continue;
        }
        // Per-path failures are non-fatal — partial coverage beats no
        // coverage. A user might not have every CLI installed.
        if let Err(err) = watcher.watch(&path, RecursiveMode::Recursive) {
            eprintln!("termory watcher: skip {path:?}: {err}");
        }
    }

    thread::spawn(move || {
        // Hold the watcher alive in the worker thread. Dropping it
        // would unregister all watches.
        let _keep_alive = watcher;
        loop {
            // Block until the first event of a burst arrives.
            let mut events: Vec<notify::Event> = Vec::new();
            match rx.recv() {
                Ok(Ok(event)) => events.push(event),
                Ok(Err(_)) => {} // watcher-level error, ignore
                Err(_) => return, // channel closed → shutdown
            }
            // Then drain everything that lands within the debounce
            // window. Once we hit Timeout we know the burst is done.
            let deadline = Instant::now() + DEBOUNCE_WINDOW;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                match rx.recv_timeout(deadline - now) {
                    Ok(Ok(event)) => events.push(event),
                    Ok(Err(_)) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }

            // If every event in the burst touched only noise files
            // (SQLite WAL/SHM, OS metadata), there's nothing to re-scan
            // for. Skip without rescanning — otherwise we'd churn on
            // database internals after every read.
            if !events.iter().any(event_has_relevant_path) {
                continue;
            }

            match scan_sessions() {
                Ok(sessions) => {
                    if let Err(err) = app_handle.emit(SOURCES_CHANGED_EVENT, sessions) {
                        eprintln!("termory watcher: emit failed: {err}");
                    }
                }
                Err(err) => {
                    eprintln!("termory watcher: rescan failed: {err}");
                }
            }

            // Drain self-induced events so they don't immediately
            // trigger another rescan. The SQLite reads we just did
            // touch `-wal` / `-shm`; FSEvents reports those back to us.
            let settle_until = Instant::now() + SETTLE_WINDOW;
            loop {
                let now = Instant::now();
                if now >= settle_until {
                    break;
                }
                match rx.recv_timeout(settle_until - now) {
                    Ok(_) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        }
    });

    Ok(())
}

/// True if `event` touches at least one path that would actually
/// affect our scan output. Filters out SQLite's `-wal` / `-shm` /
/// `-journal` sidecars (they churn on every read, including our own)
/// and OS metadata noise (`.DS_Store`). If only filtered files
/// changed, the data we'd surface is identical to last scan, so a
/// re-scan would be pure cost.
fn event_has_relevant_path(event: &notify::Event) -> bool {
    event.paths.iter().any(|path| {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return false;
        };
        if name.ends_with("-wal")
            || name.ends_with("-shm")
            || name.ends_with("-journal")
            || name == ".DS_Store"
        {
            return false;
        }
        true
    })
}

/// The list of top-level directories we watch. Each is the canonical
/// root for one platform's records:
///   * `~/.codex/` — sessions DB, memories, skills, AGENTS.md
///   * `$CLAUDE_CONFIG_DIR` or `~/.claude/` — projects, rules, skills,
///     global CLAUDE.md
///   * `~/.gemini/` — chats / memory / skills under tmp/
///   * `~/.config/opencode/` — AGENTS.md, skills
///   * `~/.local/share/opencode/` — sqlite DB, storage compat layout
///   * `~/.agents/` — tool-neutral global skills
///
/// We deliberately do NOT watch the cwd — per-project files (CLAUDE.md
/// etc.) are read on launch and rarely change during a session, and
/// recursive watch on a working project dir would generate huge noise
/// from build artifacts, node_modules, etc.
fn watch_targets() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };

    let claude_config = std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));

    vec![
        home.join(".codex"),
        claude_config,
        home.join(".gemini"),
        home.join(".config").join("opencode"),
        home.join(".local").join("share").join("opencode"),
        home.join(".agents"),
    ]
}
