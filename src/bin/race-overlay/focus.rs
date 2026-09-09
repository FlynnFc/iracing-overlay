// Rust guideline compliant 2026-02-16

//! Detects whether iRacing's own window currently has OS focus, so the
//! overlay can hide its panels while the user is alt-tabbed into another
//! app. The overlay stays click-through either way; this is purely about
//! not visually cluttering other apps (browser, Discord, etc.).

use std::time::{Duration, Instant};

use sysinfo::{ProcessesToUpdate, System};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, MAX_PATH};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow,
};
use windows::core::{BOOL, PWSTR};

/// How often to actually re-check focus. `GetForegroundWindow` itself is
/// cheap, but the process-name lookup behind it isn't free enough to run
/// unthrottled at the overlay's continuous repaint rate.
const CHECK_INTERVAL: Duration = Duration::from_millis(300);

/// Caches the foreground-window check so it only actually runs every
/// [`CHECK_INTERVAL`], not every frame.
#[derive(Debug, Default)]
pub struct FocusTracker {
    last_check: Option<Instant>,
    /// The process id the cached answer was worked out for, so the same window
    /// staying in front costs nothing at all — see [`FocusTracker::is_focused`].
    last_pid: Option<u32>,
    cached_focused: bool,
}

impl FocusTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `process_name` (case-insensitive) currently owns the
    /// foreground window. Throttled internally; safe to call every frame.
    ///
    /// The name lookup is skipped entirely while the foreground *process* is
    /// unchanged, which is the overwhelmingly common case — a driver in the sim
    /// or reading a browser is one pid for minutes at a time, and asking the OS
    /// again every 300 ms only to be told the same thing is pure cost.
    ///
    /// This used to ask `sysinfo` instead, against a `System` held for the
    /// life of the process. That type *caches* every process it is asked
    /// about, holding an open handle to each, and it was refreshed with dead
    /// processes left in place — so every distinct window a user brought to the
    /// front added a handle to a process that had since exited, and none were
    /// ever released. An hour of ordinary alt-tabbing is hundreds of them; an
    /// endurance race with the overlay left up beside a browser is thousands.
    /// One `OpenProcess`/`CloseHandle` pair per *change* of foreground window
    /// has no such state to grow.
    pub fn is_focused(&mut self, process_name: &str) -> bool {
        let now = Instant::now();
        if self.last_check.is_some_and(|last| now.duration_since(last) < CHECK_INTERVAL) {
            return self.cached_focused;
        }
        self.last_check = Some(now);

        let pid = foreground_process_pid();
        if pid == self.last_pid {
            return self.cached_focused;
        }
        self.last_pid = pid;
        self.cached_focused =
            pid.and_then(process_image_name).is_some_and(|name| name.eq_ignore_ascii_case(process_name));
        self.cached_focused
    }
}

/// The executable file name (e.g. `iRacingSim64DX11.exe`) of a running
/// process, or `None` if it has exited or cannot be opened.
///
/// `PROCESS_QUERY_LIMITED_INFORMATION` is the least privilege that answers
/// this, and is what lets an unelevated overlay ask about a process it does
/// not own. The handle is closed on every path, including the failure ones.
fn process_image_name(pid: u32) -> Option<String> {
    // SAFETY: `OpenProcess` takes only flags and a pid, and reports failure
    // through its `Result` rather than through an invalid handle.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;

    let mut buffer = [0_u16; MAX_PATH as usize];
    let mut length = u32::try_from(buffer.len()).unwrap_or(0);
    // SAFETY: `handle` is live, and `buffer`/`length` are a valid out-buffer
    // and its capacity in wide characters, which is the contract this asks for.
    let queried =
        unsafe { QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &raw mut length) };
    // SAFETY: `handle` came from `OpenProcess` above and is closed exactly once.
    let _ = unsafe { CloseHandle(handle) };
    queried.ok()?;

    let path = String::from_utf16_lossy(buffer.get(..length as usize)?);
    // The full path is not what callers compare against — `iracing_process_name`
    // is a bare executable name, as it reads in Task Manager.
    Some(path.rsplit(['\\', '/']).next()?.to_owned())
}

/// The process ID owning the current foreground window, or `None` if there
/// isn't one (e.g. transiently during a window switch).
fn foreground_process_pid() -> Option<u32> {
    // SAFETY: takes no arguments and only reads global desktop state; always
    // safe to call.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let mut pid = 0u32;
    // SAFETY: `hwnd` was just obtained from `GetForegroundWindow`, and `pid`
    // is a valid out-pointer for the duration of this call.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
    (pid != 0).then_some(pid)
}

/// Finds `process_name`'s main window and gives it OS focus. No-op if the
/// process isn't running or has no visible top-level window.
///
/// Called once right after the overlay window is created: a freshly-created
/// window can briefly hold OS focus/activation before this app's own
/// `WS_EX_NOACTIVATE` style takes effect (applied on the first frame, which
/// runs after the window already exists), and DWM's blur-behind
/// transparency can flash its raw opaque surface during that activation —
/// the same "screen goes black" symptom already fixed for clicks mid-run,
/// just happening at launch instead. Handing focus straight back to
/// iRacing closes that window instead of leaving it until the user clicks
/// away themselves.
pub fn focus_iracing_window(process_name: &str) {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, false);
    let Some(target_pid) = system
        .processes()
        .values()
        .find(|p| p.name().to_string_lossy().eq_ignore_ascii_case(process_name))
        .map(|p| p.pid().as_u32())
    else {
        return;
    };

    let mut state = EnumState { target_pid, found: None };
    // SAFETY: `enum_callback` only touches `state` through the pointer
    // passed here, which stays valid for the duration of this call since
    // `state` outlives it.
    unsafe {
        let _ = EnumWindows(Some(enum_callback), LPARAM(std::ptr::addr_of_mut!(state) as isize));
    }
    if let Some(hwnd) = state.found {
        // SAFETY: `hwnd` was just found live via `EnumWindows`.
        unsafe {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

struct EnumState {
    target_pid: u32,
    found: Option<HWND>,
}

unsafe extern "system" fn enum_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` was set by `focus_iracing_window` to point at a live
    // `EnumState` for the duration of this `EnumWindows` call.
    let state = unsafe { &mut *(lparam.0 as *mut EnumState) };
    let mut pid = 0u32;
    // SAFETY: `hwnd` is a window handle supplied by `EnumWindows` itself;
    // `pid` is a valid out-pointer for the call.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
    // SAFETY: forwarding `EnumWindows`' own contract for `hwnd`.
    if pid == state.target_pid && unsafe { IsWindowVisible(hwnd) }.as_bool() {
        state.found = Some(hwnd);
        return BOOL(0); // stop enumeration
    }
    BOOL(1) // continue
}

/// The window that currently has foreground, if any.
#[must_use]
pub fn foreground_window() -> Option<HWND> {
    // SAFETY: takes no arguments and only reads global desktop state; always
    // safe to call.
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.0.is_null()).then_some(hwnd)
}

/// Hands foreground back to `previous` if `own` — the overlay window — still
/// has it.
///
/// A freshly created window takes foreground, and nothing about this one
/// deserves it: it never wants input, and being the foreground window is half
/// of what makes the desktop compositor stop compositing it (see
/// `apply_window_styles` in `app`). `focus_iracing_window` handles the case
/// that matters most; this covers every launch where iRacing is not up yet,
/// by giving foreground back to whatever had it before — the launcher, a
/// terminal, the desktop — rather than leaving the overlay holding it until
/// the user happens to click somewhere.
///
/// `previous` is taken as it was before the overlay's window existed, since
/// by the time this runs the overlay itself is what the OS would report.
pub fn yield_foreground(own: HWND, previous: Option<HWND>) {
    let Some(previous) = previous.filter(|p| *p != own) else {
        return;
    };
    if foreground_window() != Some(own) {
        return;
    }
    // SAFETY: `previous` was a live foreground window when captured; if it has
    // since been destroyed `SetForegroundWindow` fails harmlessly.
    unsafe {
        let _ = SetForegroundWindow(previous);
    }
}
