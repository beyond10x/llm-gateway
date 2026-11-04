//! The heartbeat `serve` writes for `b10x-llm-gateway watchdog` (story:pod-watchdog).
//!
//! With a `[watchdog]` table, `serve` rewrites one small JSON file every `heartbeat_seconds`:
//! its process id, the time, its controller identity and, per route, when its last call ended
//! and how many calls are in flight. The file is written to a temporary name in the same
//! directory and renamed over the old one, so a reader sees one whole heartbeat or the previous
//! one. The watchdog takes a heartbeat older than `stale_minutes`, or none, for a gateway that
//! can no longer stop its pods.
//!
//! The route activity comes from an [`ActivitySource`] the composer hands in. A route the source
//! does not list has no known activity, and the watchdog then applies no idle limit to its pod.

use crate::config::WatchdogConfig;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs,
    io::{self, Read as _, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, PoisonError},
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// The format every heartbeat names.
pub const FORMAT: &str = "llm-gateway.heartbeat/1";

/// The largest heartbeat file the watchdog reads.
pub const MAX_HEARTBEAT_BYTES: u64 = 1 << 20;

/// One route's activity: its name, when its last call ended (Unix milliseconds) and how many of
/// its calls are in flight.
pub type RouteActivity = (String, Option<u64>, u64);

/// Where the heartbeat reads route activity from, at each beat.
pub type ActivitySource = Arc<dyn Fn() -> Vec<RouteActivity> + Send + Sync>;

/// A source that knows no route's activity.
pub fn no_activity() -> ActivitySource {
    Arc::new(Vec::new)
}

/// One heartbeat, as written and read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Heartbeat {
    pub format: String,
    pub pid: u32,
    /// When it was written, in milliseconds since the Unix epoch.
    pub time_ms: u64,
    /// The controller identity written onto every pod the gateway creates.
    pub instance: String,
    pub routes: Vec<RouteBeat>,
}

/// One route of a heartbeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteBeat {
    pub route: String,
    pub last_call_end_ms: Option<u64>,
    pub in_flight: u64,
}

impl Heartbeat {
    /// The heartbeat of this process at `time_ms` with `activity`.
    pub fn now(time_ms: u64, instance: &str, activity: Vec<RouteActivity>) -> Self {
        Self {
            format: FORMAT.to_owned(),
            pid: std::process::id(),
            time_ms,
            instance: instance.to_owned(),
            routes: activity
                .into_iter()
                .map(|(route, last_call_end_ms, in_flight)| RouteBeat {
                    route,
                    last_call_end_ms,
                    in_flight,
                })
                .collect(),
        }
    }
}

/// The default heartbeat file: `$XDG_STATE_HOME/llm-gateway/heartbeat.json` when
/// `XDG_STATE_HOME` is an absolute path, else `$HOME/.local/state/llm-gateway/heartbeat.json`;
/// `None` when neither is set.
pub fn default_path(xdg_state_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = xdg_state_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            home.map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".local").join("state"))
        })?;
    Some(base.join("llm-gateway").join("heartbeat.json"))
}

/// The heartbeat file `config` names, else [`default_path`] of this process's environment.
pub fn path_of(config: &WatchdogConfig) -> Option<PathBuf> {
    config
        .heartbeat_file
        .clone()
        .or_else(|| default_path(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME")))
}

/// Milliseconds since the Unix epoch now; 0 before it.
pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Writes `heartbeat` to `path` whole: to a temporary file beside it, mode 0600, then renamed
/// over it. A missing directory is created, mode 0700.
///
/// # Errors
/// The file system's error.
pub fn write(path: &Path, heartbeat: &Heartbeat) -> io::Result<()> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    let mut name = path.file_name().map(OsString::from).unwrap_or_default();
    name.push(format!(".{}.tmp", std::process::id()));
    let temporary = directory.join(name);
    let bytes = serde_json::to_vec(heartbeat).map_err(io::Error::other)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    drop(file);
    fs::rename(&temporary, path)
}

/// The heartbeat at `path`; `None` when there is none, it is larger than
/// [`MAX_HEARTBEAT_BYTES`], or it is not a heartbeat of [`FORMAT`].
pub fn read(path: &Path) -> Option<Heartbeat> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_HEARTBEAT_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_HEARTBEAT_BYTES {
        return None;
    }
    serde_json::from_slice::<Heartbeat>(&bytes)
        .ok()
        .filter(|heartbeat| heartbeat.format == FORMAT)
}

/// The running heartbeat thread. Dropping it, or [`Beating::stop`], ends it; the last
/// heartbeat stays, so it goes stale.
pub struct Beating {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Beating {
    /// Ends the thread and waits for it.
    pub fn stop(mut self) {
        self.end();
    }

    fn end(&mut self) {
        let (stopped, wake) = &*self.stop;
        *stopped.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

impl Drop for Beating {
    fn drop(&mut self) {
        self.end();
    }
}

/// Writes a heartbeat to `path` now and then every `interval` until stopped, reading
/// `activity` at each beat. A failed write is reported once per run of failures, without the
/// path, and the next beat tries again.
pub fn start(
    path: PathBuf,
    interval: Duration,
    instance: &str,
    activity: ActivitySource,
) -> Beating {
    let stop = Arc::new((Mutex::new(false), Condvar::new()));
    let flag = Arc::clone(&stop);
    let instance = instance.to_owned();
    let thread = thread::spawn(move || {
        let (stopped, wake) = &*flag;
        let mut failing = false;
        loop {
            let beat = Heartbeat::now(unix_ms(), &instance, activity());
            match write(&path, &beat) {
                Ok(()) => failing = false,
                Err(error) if !failing => {
                    failing = true;
                    tracing::warn!(kind = ?error.kind(), "the heartbeat could not be written");
                }
                Err(_) => {}
            }
            let guard = stopped.lock().unwrap_or_else(PoisonError::into_inner);
            let (guard, _) = wake
                .wait_timeout_while(guard, interval, |stopped| !*stopped)
                .unwrap_or_else(PoisonError::into_inner);
            if *guard {
                return;
            }
        }
    });
    Beating {
        stop,
        thread: Some(thread),
    }
}

#[cfg(test)]
mod tests {
    use super::{FORMAT, Heartbeat, default_path, read, start, write};
    use std::{
        ffi::OsString, fs, os::unix::fs::PermissionsExt as _, path::PathBuf, sync::Arc,
        time::Duration,
    };

    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("heartbeat-{name}-{}", std::process::id()));
        drop(fs::remove_dir_all(&dir));
        dir
    }

    #[test]
    fn the_default_heartbeat_follows_xdg_state_then_home() {
        let some = |text: &str| Some(OsString::from(text));
        assert_eq!(
            default_path(some("/s"), some("/h")),
            Some(PathBuf::from("/s/llm-gateway/heartbeat.json"))
        );
        assert_eq!(
            default_path(some("relative"), some("/h")),
            Some(PathBuf::from("/h/.local/state/llm-gateway/heartbeat.json"))
        );
        assert_eq!(default_path(None, None), None);
    }

    #[test]
    fn a_written_heartbeat_reads_back_whole_and_private() {
        let dir = scratch("roundtrip");
        let path = dir.join("state").join("heartbeat.json");
        let beat = Heartbeat::now(
            42,
            "b10x-llm-gateway",
            vec![("small".to_owned(), Some(7), 2)],
        );
        write(&path, &beat).unwrap();
        assert_eq!(read(&path), Some(beat));
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = fs::metadata(dir.join("state"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(
            fs::read_dir(dir.join("state")).unwrap().count(),
            1,
            "no temporary file left"
        );
        fs::write(&path, b"{\"format\":\"other/1\"}").unwrap();
        assert_eq!(read(&path), None);
        fs::write(&path, b"not json").unwrap();
        assert_eq!(read(&path), None);
        assert!(FORMAT.ends_with("/1"));
    }

    #[test]
    fn the_thread_beats_at_once_with_the_sources_activity_and_stops() {
        let dir = scratch("thread");
        let path = dir.join("heartbeat.json");
        let beating = start(
            path.clone(),
            Duration::from_secs(60),
            "b10x-llm-gateway",
            Arc::new(|| vec![("small".to_owned(), None, 1)]),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while read(&path).is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        beating.stop();
        let beat = read(&path).expect("the first beat is written at once");
        assert_eq!(beat.pid, std::process::id());
        assert_eq!(beat.routes.len(), 1);
        assert_eq!(beat.routes[0].in_flight, 1);
    }
}
