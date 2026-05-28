//! Filesystem watcher — keeps Termory's session / memory / skill view
//! in sync with on-disk changes so the user never has to click
//! "refresh". When a watched path changes, we coalesce a burst of
//! events into a single re-scan and emit the result to the frontend
//! via a Tauri event.
//!
//! Two watch tiers:
//!   * Static — each platform's top-level config dir under HOME
//!     (`~/.codex/`, `~/.claude/`, …). Set up once at startup.
//!   * Dynamic — project cwds discovered from session metadata, plus
//!     their git-root ancestors. Reconfigured after every scan (both
//!     watcher-triggered and `scan_all_sessions` IPC). Lets us catch
//!     edits to `<cwd>/CLAUDE.md`, `<cwd>/.claude/skills/...`, etc.
//!     without recursively watching every cwd the user might be in.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
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

struct WatcherInner {
    watcher: RecommendedWatcher,
    /// Project cwds we're currently dynamically watching. Diffed
    /// against the new set on every reconfigure so we only add/remove
    /// the delta — avoids tearing down and rebuilding the whole tree.
    dynamic_paths: HashSet<PathBuf>,
}

/// Handle to the running watcher. Cheap to clone (Arc).
#[derive(Clone)]
pub struct WatcherHandle {
    inner: Arc<Mutex<WatcherInner>>,
}

impl WatcherHandle {
    /// Update the set of dynamically-watched project cwds. Paths in
    /// `new_paths` that aren't already watched get added; paths that
    /// disappeared from `new_paths` get removed. Paths that overlap a
    /// static target (e.g. someone has `~/.codex/foo` as a session
    /// project — vanishingly rare) are skipped to avoid double events.
    pub fn reconfigure_dynamic(&self, new_paths: HashSet<PathBuf>) {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            // Worker thread panicked while holding the lock; recover
            // the inner state so we can still mutate the watcher.
            Err(p) => p.into_inner(),
        };

        // Remove watches no longer present.
        let to_remove: Vec<PathBuf> = inner
            .dynamic_paths
            .iter()
            .filter(|p| !new_paths.contains(*p))
            .cloned()
            .collect();
        for path in &to_remove {
            let _ = inner.watcher.unwatch(path);
        }

        // Add new watches. Skip non-existent paths (project was deleted)
        // and paths already covered by a static target.
        let static_targets = watch_targets();
        let to_add: Vec<PathBuf> = new_paths
            .iter()
            .filter(|p| !inner.dynamic_paths.contains(*p))
            .cloned()
            .collect();
        for path in &to_add {
            if !path.exists() {
                continue;
            }
            if static_targets.iter().any(|t| path.starts_with(t)) {
                continue;
            }
            if let Err(err) = inner.watcher.watch(path, RecursiveMode::Recursive) {
                eprintln!("termory watcher: skip dynamic {path:?}: {err}");
            }
        }

        inner.dynamic_paths = new_paths;
    }
}

/// Compute the project-cwd set Termory should be dynamically watching,
/// from a freshly-scanned `Vec<AppSession>`.
pub fn dynamic_paths_from_sessions<S: AsRef<str>>(
    project_paths: impl IntoIterator<Item = S>
) -> HashSet<PathBuf> {
    project_paths
        .into_iter()
        .filter_map(|p| {
            let s = p.as_ref();
            if s.is_empty() {
                return None;
            }
            let path = PathBuf::from(s);
            if path.is_absolute() {
                Some(path)
            } else {
                None
            }
        })
        .collect()
}

/// Start the filesystem watcher in a background thread. Returns the
/// handle once static watches are registered; the event loop runs
/// forever in the spawned thread.
pub fn start(app_handle: AppHandle) -> notify::Result<WatcherHandle> {
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

    let inner = Arc::new(Mutex::new(WatcherInner {
        watcher,
        dynamic_paths: HashSet::new(),
    }));
    let inner_for_thread = inner.clone();

    thread::spawn(move || {
        loop {
            // Block until the first event of a burst arrives.
            let mut events: Vec<notify::Event> = Vec::new();
            match rx.recv() {
                Ok(Ok(event)) => events.push(event),
                Ok(Err(_)) => {}  // watcher-level error, ignore
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
                    // Reconfigure dynamic watches based on the project
                    // cwds discovered in this scan. Sessions that have
                    // been opened in new projects pick up coverage;
                    // disappeared projects get unwatched.
                    let new_cwds = dynamic_paths_from_sessions(
                        sessions.iter().map(|s| s.project.as_str())
                    );
                    let handle = WatcherHandle {
                        inner: inner_for_thread.clone(),
                    };
                    handle.reconfigure_dynamic(new_cwds);

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

    Ok(WatcherHandle { inner })
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

/// The list of top-level directories we watch statically. Each is the
/// canonical root for one platform's records:
///   * `~/.codex/` — sessions DB, memories, skills, AGENTS.md
///   * `$CLAUDE_CONFIG_DIR` or `~/.claude/` — projects, rules, skills,
///     global CLAUDE.md
///   * `~/.gemini/` — chats / memory / skills under tmp/
///   * `~/.config/opencode/` — AGENTS.md, skills
///   * `~/.local/share/opencode/` — sqlite DB, storage compat layout
///   * `~/.agents/` — tool-neutral global skills
///
/// Dynamic watches (project cwds derived from session metadata) are
/// layered on top via `WatcherHandle::reconfigure_dynamic`.
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
