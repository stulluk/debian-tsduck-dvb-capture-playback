//! Background DVB device monitor (hotplug detection).
//!
//! A dedicated thread listens on the kernel `NETLINK_KOBJECT_UEVENT` socket for
//! `dvb`/`usb` add/remove events. This is the raw kernel uevent multicast group
//! (group 1), so it works without a running `udevd` and links cleanly into the
//! static musl binary (no libudev dependency). On any relevant event the device
//! catalogue is re-discovered (debounced) and the change is broadcast to the UI,
//! printed to stdout, and written to the activity log.
//!
//! If the netlink socket is unavailable (e.g. a non-Linux host) the thread falls
//! back to slow periodic polling so device presence still stays fresh.

use crate::config::TsduckPaths;
use crate::devices::{self, Discovery, DvbDevice};
use crate::session::WsEvent;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Shared, cached snapshot of the currently usable devices.
pub type DeviceState = Arc<RwLock<Discovery>>;

/// Slow-poll interval used when the netlink socket cannot be opened.
const POLL_INTERVAL: Duration = Duration::from_secs(4);

/// Coalescing window: a single plug event fans out into many dvb sub-device
/// uevents, so we wait briefly and rescan once.
const DEBOUNCE: Duration = Duration::from_millis(800);

/// Spawns the device monitor on its own OS thread.
pub fn spawn(paths: Arc<TsduckPaths>, state: DeviceState, bus: broadcast::Sender<WsEvent>) {
    std::thread::Builder::new()
        .name("dvb-monitor".to_string())
        .spawn(move || run(paths, state, bus))
        .ok();
}

/// Monitor entry point: initial scan, then event-driven or polling loop.
fn run(paths: Arc<TsduckPaths>, state: DeviceState, bus: broadcast::Sender<WsEvent>) {
    rescan(&paths, &state, &bus, false);

    let fd = match open_uevent_socket() {
        Some(fd) => fd,
        None => {
            poll_loop(&paths, &state, &bus);
            return;
        }
    };

    let mut buf = [0u8; 8192];
    loop {
        if !wait_relevant_event(fd, &mut buf) {
            // Socket failure: close and degrade to polling.
            unsafe {
                libc::close(fd);
            }
            poll_loop(&paths, &state, &bus);
            return;
        }
        drain_for(fd, &mut buf, DEBOUNCE);
        rescan(&paths, &state, &bus, true);
    }
}

/// Periodic-rescan fallback when no netlink socket is available.
fn poll_loop(paths: &TsduckPaths, state: &DeviceState, bus: &broadcast::Sender<WsEvent>) {
    loop {
        std::thread::sleep(POLL_INTERVAL);
        rescan(paths, state, bus, true);
    }
}

/// Re-runs discovery and, when the device set changed, updates the shared
/// state and announces the change. `announce` is false for the initial scan.
fn rescan(
    paths: &TsduckPaths,
    state: &DeviceState,
    bus: &broadcast::Sender<WsEvent>,
    announce: bool,
) {
    let discovery = match devices::discover(paths) {
        Ok(d) => d,
        Err(_) => return,
    };

    let prev = state.read().ok().map(|d| signatures(&d.devices));
    let now_sig = signatures(&discovery.devices);
    let changed = prev.as_ref().map(|p| p != &now_sig).unwrap_or(true);

    if let Ok(mut w) = state.write() {
        *w = discovery.clone();
    }

    if !changed {
        return;
    }

    if announce {
        if let Some(prev) = prev {
            for line in change_lines(&prev, &discovery.devices) {
                println!("{line}");
                let _ = bus.send(WsEvent::Log {
                    line: format!(">>> {line}"),
                });
            }
        }
    }

    let _ = bus.send(WsEvent::Devices {
        devices: discovery.devices.clone(),
        warnings: discovery.warnings.clone(),
    });
}

/// Stable per-device signatures used to detect catalogue changes.
fn signatures(devices: &[DvbDevice]) -> Vec<String> {
    let mut s: Vec<String> = devices
        .iter()
        .map(|d| format!("{}|{}|{}", d.label, d.address, d.usb_present))
        .collect();
    s.sort();
    s
}

/// Human-readable connected/disconnected lines from a device-set diff.
fn change_lines(prev: &[String], now: &[DvbDevice]) -> Vec<String> {
    let now_sig = signatures(now);
    let mut lines = Vec::new();
    for d in now {
        let sig = format!("{}|{}|{}", d.label, d.address, d.usb_present);
        if !prev.contains(&sig) {
            lines.push(format!("device connected: {} @ {}", d.label, d.address));
        }
    }
    for p in prev {
        if !now_sig.contains(p) {
            let label = p.split('|').next().unwrap_or(p);
            lines.push(format!("device disconnected: {label}"));
        }
    }
    lines
}

/// Opens and binds a `NETLINK_KOBJECT_UEVENT` socket to the kernel group.
fn open_uevent_socket() -> Option<libc::c_int> {
    unsafe {
        let fd = libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::NETLINK_KOBJECT_UEVENT,
        );
        if fd < 0 {
            return None;
        }
        let mut addr: libc::sockaddr_nl = std::mem::zeroed();
        addr.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        addr.nl_pid = 0; // let the kernel assign a unique port id
        addr.nl_groups = 1; // multicast group 1 = kernel-generated uevents
        let ret = libc::bind(
            fd,
            &addr as *const libc::sockaddr_nl as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        );
        if ret < 0 {
            libc::close(fd);
            return None;
        }
        Some(fd)
    }
}

/// Blocks until a relevant `dvb`/`usb` uevent arrives. Returns false on a
/// socket error so the caller can degrade to polling.
fn wait_relevant_event(fd: libc::c_int, buf: &mut [u8]) -> bool {
    loop {
        let n = unsafe {
            libc::recv(
                fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                0,
            )
        };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return false;
        }
        if n == 0 {
            continue;
        }
        if event_is_relevant(&buf[..n as usize]) {
            return true;
        }
    }
}

/// Drains additional pending events for `window` to coalesce a burst.
fn drain_for(fd: libc::c_int, buf: &mut [u8], window: Duration) {
    let deadline = Instant::now() + window;
    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = (deadline - now).as_millis().max(1) as libc::c_int;
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let r = unsafe { libc::poll(&mut pfd, 1, remaining) };
        if r <= 0 {
            break;
        }
        let _ = unsafe {
            libc::recv(
                fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
                0,
            )
        };
    }
}

/// True if a raw uevent message concerns a `dvb`/`usb` device being added or
/// removed (NUL-separated `KEY=VALUE` fields).
fn event_is_relevant(msg: &[u8]) -> bool {
    let mut subsystem: Option<&str> = None;
    let mut action: Option<&str> = None;
    for field in msg.split(|&b| b == 0) {
        if let Ok(s) = std::str::from_utf8(field) {
            if let Some(v) = s.strip_prefix("SUBSYSTEM=") {
                subsystem = Some(v);
            } else if let Some(v) = s.strip_prefix("ACTION=") {
                action = Some(v);
            }
        }
    }
    let sub_ok = matches!(subsystem, Some("dvb") | Some("usb"));
    let act_ok = matches!(
        action,
        Some("add") | Some("remove") | Some("bind") | Some("unbind") | Some("change")
    );
    sub_ok && act_ok
}
