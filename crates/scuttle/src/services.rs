//! Background threads that feed the UI: compositor state, machine
//! stats, the app list, the file index and the control socket.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use async_channel::Sender;
use scuttle_core::ipc::{self, Request, Response};
use scuttle_core::paths;
use scuttle_hypr::Hyprland;
use scuttle_launcher::{desktop, files};
use scuttle_sys::{battery, cpu, memory, network};

use crate::msg::{DesktopState, Msg, Stats};

/// Reads everything the UI shows about the compositor in one go.
pub fn snapshot(hypr: &Hyprland) -> io::Result<DesktopState> {
    let workspaces = hypr.workspaces()?;
    let active_workspace = hypr.active_workspace()?.id;
    let active = hypr.active_window()?;
    let clients = hypr.clients().unwrap_or_default();
    let focused_monitor = hypr
        .monitors()
        .ok()
        .and_then(|list| list.into_iter().find(|m| m.focused).map(|m| m.name));
    Ok(DesktopState {
        workspaces,
        active_workspace,
        active_class: active.as_ref().map(|c| c.class.clone()).unwrap_or_default(),
        active_title: active.map(|c| c.title).unwrap_or_default(),
        clients,
        focused_monitor,
    })
}

/// Watches Hyprland and sends a fresh [`DesktopState`] whenever
/// something the UI shows has changed.
///
/// One thread reads the event socket and only raises a flag. A second
/// thread turns bursts of events into a single re-read, so switching
/// workspace (which fires several events at once) costs one refresh.
pub fn watch_hyprland(hypr: Hyprland, tx: Sender<Msg>) {
    let dirty = Arc::new(AtomicBool::new(true));

    let reader_dirty = Arc::clone(&dirty);
    let reader_hypr = hypr.clone();
    let worker = thread::spawn(move || {
        let mut last: Option<DesktopState> = None;
        loop {
            if !dirty.swap(false, Ordering::AcqRel) {
                thread::park();
                continue;
            }
            // Let the rest of a burst arrive before reading.
            thread::sleep(Duration::from_millis(12));
            dirty.store(false, Ordering::Release);
            if let Ok(state) = snapshot(&hypr) {
                if last.as_ref() != Some(&state) {
                    last = Some(state.clone());
                    if tx.send_blocking(Msg::Desktop(state)).is_err() {
                        return;
                    }
                }
            }
        }
    });

    thread::spawn(move || loop {
        if let Ok(events) = reader_hypr.events() {
            // A fresh connection may have missed events.
            reader_dirty.store(true, Ordering::Release);
            worker.thread().unpark();
            for event in events {
                if event.affects_workspaces() || event.affects_title() {
                    reader_dirty.store(true, Ordering::Release);
                    worker.thread().unpark();
                }
            }
        }
        // The compositor went away or restarted. Try again shortly.
        thread::sleep(Duration::from_secs(2));
    });
}

/// Samples CPU, memory, battery and network on a timer and reports only
/// when a value the bar shows has changed.
pub fn watch_stats(tx: Sender<Msg>) {
    thread::spawn(move || {
        let mut previous = cpu::sample().unwrap_or_default();
        let mut last: Option<Stats> = None;
        loop {
            let power = battery::read_system();
            // Wake less often on battery.
            let interval = match power {
                Some(b) if b.discharging() => 10,
                _ => 3,
            };
            thread::sleep(Duration::from_secs(interval));
            let current = cpu::sample().unwrap_or(previous);
            let stats = Stats {
                cpu: cpu::usage(previous, current),
                memory: memory::read(),
                battery: battery::read_system(),
                link: network::read_system(),
            };
            previous = current;
            if last.as_ref() != Some(&stats) {
                last = Some(stats.clone());
                if tx.send_blocking(Msg::Stats(stats)).is_err() {
                    return;
                }
            }
        }
    });
}

/// Reads the first stats sample without waiting for the timer, so the
/// bar is never blank at startup. CPU load needs two samples, so it
/// starts at zero.
pub fn initial_stats() -> Stats {
    Stats {
        cpu: 0,
        memory: memory::read(),
        battery: battery::read_system(),
        link: network::read_system(),
    }
}

/// Scans installed applications off the UI thread.
pub fn scan_apps(tx: Sender<Msg>) {
    thread::spawn(move || {
        let apps = desktop::scan(&paths::application_dirs(), &desktop::current_desktops());
        let _ = tx.send_blocking(Msg::Apps(apps));
    });
}

/// Builds the file index off the UI thread.
pub fn index_files(roots: Vec<PathBuf>, home: PathBuf, tx: Sender<Msg>) {
    thread::spawn(move || {
        let roots = files::resolve_roots(&roots, &home);
        let index = files::build_index(&roots, 60_000);
        let _ = tx.send_blocking(Msg::Files(index));
    });
}

/// Why the control socket could not be opened.
pub enum SocketError {
    /// Another Scuttle is already answering on it.
    AlreadyRunning,
    Io(io::Error),
}

/// Opens the control socket and serves requests from the `scuttle`
/// command, forwarding each to the UI thread and relaying its answer.
pub fn serve_ipc(socket: &Path, tx: Sender<Msg>) -> Result<(), SocketError> {
    if socket.exists() {
        // A live shell answers a ping. A stale socket from a crash
        // refuses the connection and can be replaced.
        if ipc::send(socket, &Request::Ping).is_ok() {
            return Err(SocketError::AlreadyRunning);
        }
        let _ = fs::remove_file(socket);
    }
    if let Some(parent) = socket.parent() {
        fs::create_dir_all(parent).map_err(SocketError::Io)?;
    }
    let listener = UnixListener::bind(socket).map_err(SocketError::Io)?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            thread::spawn(move || {
                let _ = handle_connection(stream, tx);
            });
        }
    });
    Ok(())
}

fn handle_connection(stream: UnixStream, tx: Sender<Msg>) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut line = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut line)?;
    let response = match ipc::decode_request(&line) {
        Err(message) => Response::Error { message },
        Ok(request) => {
            let (reply_tx, reply_rx) = sync_channel(1);
            if tx.send_blocking(Msg::Ipc(request, reply_tx)).is_err() {
                Response::Error {
                    message: "Scuttle is shutting down.".into(),
                }
            } else {
                reply_rx
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap_or(Response::Error {
                        message: "Scuttle did not answer in time.".into(),
                    })
            }
        }
    };
    let mut stream = stream;
    stream.write_all(ipc::encode_response(&response).as_bytes())
}
