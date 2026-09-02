use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ButtonIndex, ConnectionExt as _, EventMask, GrabMode, ModMask, Window,
};
use x11rb::rust_connection::RustConnection;
use x11rb::{connect, protocol::Event};

use crate::gesture::GestureTracker;
use crate::platform::{dispatch, Platform};

/// Right mouse button (X11 button 3).
const RIGHT_BUTTON: u8 = 3;
/// Right mouse button as a `ButtonIndex` for grab requests.
const RIGHT_BUTTON_INDEX: ButtonIndex = ButtonIndex::M3;
/// X11 `NONE` atom/window id.
const NONE: u32 = 0;

/// X11 connection together with the screen index for its root window.
pub struct LinuxPlatform {
    conn: RustConnection,
    screen_num: usize,
}

impl LinuxPlatform {
    pub fn new() -> Result<LinuxPlatform, String> {
        let (conn, screen_num) = connect(None)
            .map_err(|e| format!("failed to connect to X server: {e}"))?;
        Ok(LinuxPlatform { conn, screen_num })
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

    fn run(&mut self) -> Result<(), String> {
        self.grab_right_button()?;

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
                }
                Event::MotionNotify(ev) if tracking => {
                    tracker.add(ev.root_x as f64, ev.root_y as f64);
                }
                Event::ButtonRelease(ev) if tracking && ev.detail == RIGHT_BUTTON => {
                    tracking = false;
                    if let Some(outcome) = tracker.finish(ev.root_x as f64, ev.root_y as f64) {
                        match dispatch(self, outcome) {
                            Ok(()) => {}
                            Err(e) => eprintln!("[mouse] error: {e}"),
                        }
                    }
                }
                _ => {}
            }
        }
    }
}