use std::path::Path;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

use rdev::{grab, Button, Event, EventType};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::core::PWSTR;

use crate::gesture::{GestureTracker, Outcome};
use crate::platform::Platform;

/// Gesture tracker state, only touched by the hook callback thread.
static STATE: OnceLock<Mutex<GestureState>> = OnceLock::new();

/// Global last known pointer position, refreshed by `MouseMove` events.
static LAST_POS: OnceLock<Mutex<(f64, f64)>> = OnceLock::new();

/// Remaining synthetic right-button press/release events to pass through
/// untouched. Set before replaying a click so the injected events do not
/// re-trigger gesture tracking (which would loop forever).
static REPLAY_SKIP: OnceLock<Mutex<u32>> = OnceLock::new();

struct GestureState {
    tracker: GestureTracker,
    tracking: bool,
}

impl Default for GestureState {
    fn default() -> Self {
        GestureState {
            tracker: GestureTracker::default(),
            tracking: false,
        }
    }
}

fn last_pos() -> Option<(f64, f64)> {
    let guard = LAST_POS.get_or_init(|| Mutex::new((0.0, 0.0))).lock().ok()?;
    Some(*guard)
}

fn update_pos(x: f64, y: f64) {
    if let Ok(mut guard) = LAST_POS.get_or_init(|| Mutex::new((0.0, 0.0))).lock() {
        *guard = (x, y);
    }
}

/// Consume one pass-through slot for a synthetic right-button event.
fn consume_replay_skip() -> bool {
    let mut guard = REPLAY_SKIP
        .get_or_init(|| Mutex::new(0))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if *guard > 0 {
        *guard -= 1;
        true
    } else {
        false
    }
}

/// Windows implementation backed by a `WH_MOUSE_LL` low-level hook via rdev.
///
/// rdev keeps the callback in a global static, so the platform instance itself
/// holds no capture state.
pub struct WindowsPlatform;

impl WindowsPlatform {
    pub fn new() -> WindowsPlatform {
        WindowsPlatform
    }

    /// Returns the base name (without extension) of the foreground process,
    /// e.g. "firefox".
    fn foreground_process_name() -> Option<String> {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == 0 {
            return None;
        }

        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) };
        let Ok(process) = process else {
            return None;
        };
        let name = Self::query_process_name(process);
        unsafe { _ = CloseHandle(process) };
        let name = name?;
        Path::new(&name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
    }

    fn query_process_name(process: HANDLE) -> Option<String> {
        let mut buffer = [0u16; 1024];
        let mut size = buffer.len() as u32;
        let pwstr = PWSTR(buffer.as_mut_ptr());
        unsafe {
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, pwstr, &mut size).ok()?;
        }
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }
}

impl Platform for WindowsPlatform {
    fn foreground_app(&mut self) -> Option<String> {
        Self::foreground_process_name()
    }

    fn replay_right_click(&mut self) -> Result<(), String> {
        crate::action::click_right()
    }

    fn run(&mut self) -> Result<(), String> {
        // Heavy work (foreground lookup, key injection) is moved to a worker
        // thread so the low-level hook callback stays fast and the system
        // input pipeline never blocks behind it.
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut platform = WindowsPlatform;
            while let Ok(outcome) = rx.recv() {
                if let Err(e) = crate::platform::dispatch(&mut platform, outcome) {
                    eprintln!("[mouse] error: {e}");
                }
            }
        });

        let callback = move |event: Event| -> Option<Event> {
            let mut state = STATE
                .get_or_init(|| Mutex::new(GestureState::default()))
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match event.event_type {
                EventType::ButtonPress(Button::Right) => {
                    // Synthetic events replayed for a normal right-click.
                    if consume_replay_skip() {
                        return Some(event);
                    }
                    let Some((x, y)) = last_pos() else {
                        return Some(event);
                    };
                    state.tracker.start(x, y);
                    state.tracking = true;
                    None // swallow the press so the context menu does not open
                }
                EventType::MouseMove { x, y } if state.tracking => {
                    update_pos(x, y);
                    state.tracker.add(x, y);
                    Some(event) // pass through so the cursor can still move
                }
                EventType::MouseMove { x, y } => {
                    update_pos(x, y);
                    Some(event)
                }
                EventType::ButtonRelease(Button::Right) => {
                    // Synthetic events replayed for a normal right-click.
                    if consume_replay_skip() {
                        return Some(event);
                    }
                    if !state.tracking {
                        return Some(event);
                    }
                    state.tracking = false;
                    let Some((x, y)) = last_pos() else {
                        return None;
                    };
                    if let Some(outcome) = state.tracker.finish(x, y) {
                        if let Outcome::Click = outcome {
                            // Replay a synthetic click; set skip for its press+release.
                            *REPLAY_SKIP
                                .get_or_init(|| Mutex::new(0))
                                .lock()
                                .unwrap_or_else(|e| e.into_inner()) = 2;
                        }
                        let _ = tx.send(outcome);
                    }
                    None // swallow the release
                }
                _ => Some(event),
            }
        };

        grab(callback).map_err(|e| format!("failed to install input hook: {e:?}"))?;
        Ok(())
    }
}