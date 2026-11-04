use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ButtonIndex, ClientMessageEvent, ConnectionExt as _, CreateWindowAux,
    EventMask, GrabMode, ModMask, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::{COPY_FROM_PARENT, connect, protocol::Event};

use crate::gesture::GestureTracker;
use crate::platform::{dispatch, Platform};
#[cfg(feature = "trail")]
use crate::platform::x11_overlay::X11Overlay;

/// Right mouse button (X11 button 3).
const RIGHT_BUTTON: u8 = 3;
/// Right mouse button as a `ButtonIndex` for grab requests.
const RIGHT_BUTTON_INDEX: ButtonIndex = ButtonIndex::M3;
/// X11 `NONE` atom/window id.
const NONE: u32 = 0;
/// Atom of the client message that wakes the loop on quit.
const QUIT_ATOM_NAME: &[u8] = b"_MOUSE_QUIT";
/// Size of the unmapped window that receives the quit message.
const QUIT_WINDOW_SIZE: u16 = 1;

/// X11 connection together with the screen index for its root window.
pub struct LinuxPlatform {
    conn: RustConnection,
    screen_num: usize,
    /// Unmapped window used to wake `run` when the tray asks to quit.
    quit_window: Window,
    quit_atom: Atom,
}

/// Asks the `wait_for_event` loop to exit.
///
/// Safe to call from another thread: it opens a short-lived connection and
/// sends a client message to the window owned by the event loop.
#[derive(Clone, Copy)]
pub struct QuitHandle {
    window: Window,
    atom: Atom,
}

impl QuitHandle {
    pub fn request(self) {
        let Ok((conn, _)) = connect(None) else {
            eprintln!("[mouse] quit: failed to connect to X server");
            return;
        };
        let event = ClientMessageEvent::new(32, self.window, self.atom, [0u32; 5]);
        // Check the request synchronously: the temporary connection is closed
        // right after this call, and closing it before the server has processed
        // the send would occasionally drop the event.
        let sent = conn
            .send_event(false, self.window, EventMask::NO_EVENT, event)
            .map_err(|e| e.to_string())
            .and_then(|cookie| cookie.check().map_err(|e| e.to_string()));
        if let Err(e) = sent {
            eprintln!("[mouse] quit: failed to send quit event: {e}");
        }
    }
}

impl LinuxPlatform {
    pub fn new() -> Result<LinuxPlatform, String> {
        let (conn, screen_num) = connect(None)
            .map_err(|e| format!("failed to connect to X server: {e}"))?;

        let root = conn.setup().roots[screen_num].root;
        let quit_atom = conn
            .intern_atom(false, QUIT_ATOM_NAME)
            .map_err(|e| format!("failed to intern quit atom: {e}"))?
            .reply()
            .map_err(|e| format!("failed to intern quit atom: {e}"))?
            .atom;
        let quit_window = conn
            .generate_id()
            .map_err(|e| format!("failed to allocate quit window: {e}"))?;
        conn.create_window(
            0,
            quit_window,
            root,
            0,
            0,
            QUIT_WINDOW_SIZE,
            QUIT_WINDOW_SIZE,
            0,
            WindowClass::INPUT_ONLY,
            COPY_FROM_PARENT,
            &CreateWindowAux::new().event_mask(EventMask::NO_EVENT),
        )
        .map_err(|e| format!("failed to create quit window: {e}"))?
        .check()
        .map_err(|e| format!("failed to create quit window: {e}"))?;

        Ok(LinuxPlatform {
            conn,
            screen_num,
            quit_window,
            quit_atom,
        })
    }
    fn root(&self) -> Window {
        self.conn.setup().roots[self.screen_num].root
    }

    fn grab_right_button(&self) -> Result<(), String> {
        let mask = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
        self.conn
            .grab_button(
                false,
                self.root(),
                mask,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                NONE,
                NONE,
                RIGHT_BUTTON_INDEX,
                ModMask::ANY,
            )
            .map_err(|e| format!("failed to grab right button: {e}"))?
            .check()
            .map_err(|e| format!("failed to grab right button: {e}"))?;
        Ok(())
    }

    fn ungrab_right_button(&self) -> Result<(), String> {
        self.conn
            .ungrab_button(RIGHT_BUTTON_INDEX, self.root(), ModMask::ANY)
            .map_err(|e| format!("failed to ungrab right button: {e}"))?
            .check()
            .map_err(|e| format!("failed to ungrab right button: {e}"))?;
        Ok(())
    }

    /// Read the WM_CLASS class name for a window, walking up to its top-level
    /// ancestor if needed.
    fn window_class(&self, window: Window) -> Option<String> {
        let mut current = window;
        let root = self.root();
        loop {
            if let Some(class) = self.wm_class_of(current) {
                return Some(class);
            }
            if current == root {
                return None;
            }
            let reply = self.conn.query_tree(current).ok()?.reply().ok()?;
            let parent = reply.parent;
            if parent == current || parent == 0 {
                return None;
            }
            current = parent;
        }
    }

    fn wm_class_of(&self, window: Window) -> Option<String> {
        let reply = self
            .conn
            .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        let value = reply.value8()?;
        let bytes: Vec<u8> = value.collect();
        if bytes.is_empty() {
            return None;
        }
        // WM_CLASS is "instance\0class\0"; the class is the last non-empty token.
        let class = String::from_utf8_lossy(&bytes)
            .split('\0')
            .rfind(|s| !s.is_empty())?
            .to_string();
        Some(class)
    }
}

impl Platform for LinuxPlatform {
    fn foreground_app(&mut self) -> Option<String> {
        let focus = self.conn.get_input_focus().ok()?.reply().ok()?.focus;
        if focus == 0 {
            return None;
        }
        self.window_class(focus)
    }

    fn replay_right_click(&mut self) -> Result<(), String> {
        // Temporarily release the grab so the synthetic click reaches the app.
        self.ungrab_right_button()?;
        let result = crate::action::click_right();
        self.grab_right_button()?;
        result
    }

    fn quit_handle(&self) -> QuitHandle {
        QuitHandle {
            window: self.quit_window,
            atom: self.quit_atom,
        }
    }

    fn run(&mut self) -> Result<(), String> {
        self.grab_right_button()?;

        #[cfg(feature = "trail")]
        let overlay = match X11Overlay::create(&self.conn, self.screen_num) {
            Some(Ok(overlay)) => Some(overlay),
            Some(Err(e)) => {
                eprintln!("[mouse] trail overlay disabled: {e}");
                None
            }
            None => {
                eprintln!("[mouse] trail overlay disabled: no 32-bit visual");
                None
            }
        };

        let mut tracker = GestureTracker::default();
        let mut tracking = false;

        loop {
            let event = self
                .conn
                .wait_for_event()
                .map_err(|e| format!("failed to wait for X event: {e}"))?;

            match event {
                Event::ButtonPress(ev) if ev.detail == RIGHT_BUTTON => {
                    tracker.start(ev.root_x as f64, ev.root_y as f64);
                    tracking = true;
                    #[cfg(feature = "trail")]
                    if let Some(overlay) = &overlay
                        && let Err(e) = overlay.show(&self.conn) {
                            eprintln!("[mouse] {e}");
                        }
                }
                Event::MotionNotify(ev) if tracking => {
                    tracker.add(ev.root_x as f64, ev.root_y as f64);
                    #[cfg(feature = "trail")]
                    if let Some(overlay) = &overlay
                        && let Err(e) = overlay.draw(&self.conn, tracker.points()) {
                            eprintln!("[mouse] {e}");
                        }
                }
                Event::ButtonRelease(ev) if tracking && ev.detail == RIGHT_BUTTON => {
                    tracking = false;
                    #[cfg(feature = "trail")]
                    if let Some(overlay) = &overlay
                        && let Err(e) = overlay.hide(&self.conn) {
                            eprintln!("[mouse] {e}");
                        }
                    if let Some(outcome) = tracker.finish(ev.root_x as f64, ev.root_y as f64) {
                        match dispatch(self, outcome) {
                            Ok(()) => {}
                            Err(e) => eprintln!("[mouse] error: {e}"),
                        }
                    }
                }
                Event::ClientMessage(ev)
                    if ev.window == self.quit_window && ev.type_ == self.quit_atom =>
                {
                    break;
                }
                _ => {}
            }
        }

        self.ungrab_right_button()?;
        Ok(())
    }
}