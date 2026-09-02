use std::path::Path;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

use rdev::{grab, Button, Event, EventType};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId,
};
#[cfg(feature = "trail")]
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
};
use windows::core::PWSTR;

use crate::gesture::{GestureTracker, Outcome};
use crate::platform::Platform;
#[cfg(feature = "trail")]
use crate::platform::win_overlay::WinOverlay;

/// Gesture tracker state, only touched by the hook callback thread.
static STATE: OnceLock<Mutex<GestureState>> = OnceLock::new();

/// Command for the trail overlay worker thread.
#[cfg(feature = "trail")]
enum OverlayMsg {
    Show,
    Draw(Vec<(f64, f64)>),
    Hide,
}

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

/// Seed LAST_POS with the real cursor position at startup, so a right-click
/// performed before any mouse move is tracked from a correct origin instead
/// of the default (0, 0).
fn init_last_pos() {
    if let Ok(mut guard) = LAST_POS.get_or_init(|| Mutex::new((0.0, 0.0))).lock() {
        let mut point = windows::Win32::Foundation::POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&raw mut point) }.is_ok() {
            *guard = (point.x as f64, point.y as f64);
        }
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
        // Seed the pointer cache with the real cursor position so a gesture
        // started before any mouse move uses a correct origin.
        init_last_pos();

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

        // The trail overlay is created on its own worker thread so every GDI
        // object (DC, DIB, pen) is created and used on the same thread --
        // GDI handles are thread-affine and crossing threads is UB. The hook
        // callback only sends cheap, coalesced messages.
        #[cfg(feature = "trail")]
        let (otx, orx) = mpsc::channel::<OverlayMsg>();
        #[cfg(feature = "trail")]
        std::thread::spawn(move || overlay_thread(orx));

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
                    #[cfg(feature = "trail")]
                    let _ = otx.send(OverlayMsg::Show);
                    None // swallow the press so the context menu does not open
                }
                EventType::MouseMove { x, y } if state.tracking => {
                    update_pos(x, y);
                    state.tracker.add(x, y);
                    #[cfg(feature = "trail")]
                    let _ = otx.send(OverlayMsg::Draw(state.tracker.points().to_vec()));
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
                    #[cfg(feature = "trail")]
                    let _ = otx.send(OverlayMsg::Hide);
                    let Some((x, y)) = last_pos() else {
                        return None;
                    };
                    if let Some(outcome) = state.tracker.finish(x, y) {
                        if let Outcome::Click = outcome {
                            // Replay a synthetic click; set skip for its press+release.
                            // Accumulate so overlapping replays are not lost.
                            *REPLAY_SKIP
                                .get_or_init(|| Mutex::new(0))
                                .lock()
                                .unwrap_or_else(|e| e.into_inner()) += 2;
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

/// Run the trail overlay on a dedicated thread.
///
/// The overlay window and all GDI objects are created here so they live on a
/// single thread (GDI handles are thread-affine). Messages from the hook
/// callback are coalesced: only the latest `Draw` is rendered, which keeps the
/// overlay responsive and avoids flooding the system with redraws. A message
/// pump is run so the layered window receives and processes its messages.
#[cfg(feature = "trail")]
fn overlay_thread(rx: mpsc::Receiver<OverlayMsg>) {
    let mut overlay = match WinOverlay::create() {
        Some(Ok(o)) => o,
        Some(Err(e)) => {
            eprintln!("[mouse] trail overlay disabled: {e}");
            return;
        }
        None => {
            eprintln!("[mouse] trail overlay disabled");
            return;
        }
    };

    // Pump window messages (required for the layered window to behave) while
    // waiting for and coalescing overlay commands.
    let mut pending: Option<OverlayMsg> = None;
    loop {
        let mut msg = MSG::default();
        let has_msg = unsafe {
            PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool()
        };
        if has_msg {
            unsafe {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }
            continue;
        }

        // Drain any queued commands (non-blocking) and coalesce Draw messages
        // so a fast-moving mouse does not flood the renderer.
        loop {
            match rx.try_recv() {
                Ok(OverlayMsg::Show) => {
                    pending = Some(OverlayMsg::Show);
                }
                Ok(OverlayMsg::Draw(points)) => {
                    pending = Some(OverlayMsg::Draw(points));
                }
                Ok(OverlayMsg::Hide) => {
                    pending = Some(OverlayMsg::Hide);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }

        if let Some(msg) = pending.take() {
            let result = match msg {
                OverlayMsg::Show => overlay.show(),
                OverlayMsg::Draw(points) => overlay.draw(&points),
                OverlayMsg::Hide => overlay.hide(),
            };
            if let Err(e) = result {
                eprintln!("[mouse] {e}");
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(4));
    }
}