use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{
    ColormapAlloc, ConnectionExt as _, CoordMode, CreateGCAux, CreateWindowAux, EventMask,
    Gcontext, Pixmap, VisualClass, Visualid, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

/// Trail color as an ARGB32 pixel (fully opaque green).
const TRAIL_COLOR: u32 = 0xFF00_FF00;
/// Transparent black pixel, used to clear the backing store.
const TRANSPARENT: u32 = 0x0000_0000;
/// Trail line width in pixels.
const LINE_WIDTH: u32 = 3;

/// Fullscreen overlay that renders the gesture trail on X11.
///
/// Uses a depth-32 (ARGB) window so undrawn areas stay transparent. The
/// trail is painted into a backing pixmap and copied to the window to avoid
/// flicker. Its input shape is emptied so it never intercepts clicks.
pub struct X11Overlay {
    window: Window,
    pixmap: Pixmap,
    line_gc: Gcontext,
    clear_gc: Gcontext,
    copy_gc: Gcontext,
    width: u16,
    height: u16,
}

impl X11Overlay {
    /// Create the overlay window for the given screen.
    ///
    /// :param conn: X11 connection.
    /// :param screen_num: Screen index whose geometry and visual are used.
    /// :return: The overlay, or None when no depth-32 visual is available.
    pub fn create(conn: &RustConnection, screen_num: usize) -> Option<Result<X11Overlay, String>> {
        let screen = &conn.setup().roots[screen_num];
        let visual = find_argb_visual(screen)?;
        let width = screen.width_in_pixels;
        let height = screen.height_in_pixels;
        Some(create_inner(conn, screen, visual, width, height))
    }

    /// Show the overlay and draw a single point (gesture start).
    pub fn show(&self, conn: &RustConnection) -> Result<(), String> {
        // Clear any leftover trail from the previous gesture so it does not
        // reappear when the window is shown again.
        let rect = x11rb::protocol::xproto::Rectangle {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        };
        conn.poly_fill_rectangle(self.pixmap, self.clear_gc, &[rect])
            .map_err(|e| format!("failed to clear trail: {e}"))?
            .check()
            .map_err(|e| format!("failed to clear trail: {e}"))?;
        conn.copy_area(
            self.pixmap,
            self.window,
            self.copy_gc,
            0,
            0,
            0,
            0,
            self.width,
            self.height,
        )
        .map_err(|e| format!("failed to blit trail: {e}"))?
        .check()
        .map_err(|e| format!("failed to blit trail: {e}"))?;

        conn.map_window(self.window)
            .map_err(|e| format!("failed to map overlay: {e}"))?
            .check()
            .map_err(|e| format!("failed to map overlay: {e}"))?;
        // Explicitly raise above all other windows; override-redirect windows
        // are not managed by the WM and may otherwise stay buried underneath.
        let aux = x11rb::protocol::xproto::ConfigureWindowAux::new()
            .stack_mode(x11rb::protocol::xproto::StackMode::ABOVE);
        conn.configure_window(self.window, &aux)
            .map_err(|e| format!("failed to raise overlay: {e}"))?
            .check()
            .map_err(|e| format!("failed to raise overlay: {e}"))?;
        conn.flush().map_err(|e| format!("failed to flush overlay: {e}"))
    }

    /// Redraw the overlay with the given trail points.
    pub fn draw(&self, conn: &RustConnection, points: &[(f64, f64)]) -> Result<(), String> {
        // Clear the backing pixmap to transparent.
        let rect = x11rb::protocol::xproto::Rectangle {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        };
        conn.poly_fill_rectangle(self.pixmap, self.clear_gc, &[rect])
            .map_err(|e| format!("failed to clear trail: {e}"))?
            .check()
            .map_err(|e| format!("failed to clear trail: {e}"))?;

        if points.len() >= 2 {
            let pts: Vec<_> = points
                .iter()
                .map(|&(x, y)| x11rb::protocol::xproto::Point {
                    x: x as i16,
                    y: y as i16,
                })
                .collect();
            conn.poly_line(CoordMode::ORIGIN, self.pixmap, self.line_gc, &pts)
                .map_err(|e| format!("failed to draw trail: {e}"))?
                .check()
                .map_err(|e| format!("failed to draw trail: {e}"))?;
        }

        conn.copy_area(
            self.pixmap,
            self.window,
            self.copy_gc,
            0,
            0,
            0,
            0,
            self.width,
            self.height,
        )
        .map_err(|e| format!("failed to blit trail: {e}"))?
        .check()
        .map_err(|e| format!("failed to blit trail: {e}"))?;

        conn.flush().map_err(|e| format!("failed to flush overlay: {e}"))
    }

    /// Hide the overlay and clear the trail.
    pub fn hide(&self, conn: &RustConnection) -> Result<(), String> {
        conn.unmap_window(self.window)
            .map_err(|e| format!("failed to unmap overlay: {e}"))?
            .check()
            .map_err(|e| format!("failed to unmap overlay: {e}"))
    }
}

/// Look for a depth-32 TrueColor visual usable as an ARGB overlay.
fn find_argb_visual(screen: &x11rb::protocol::xproto::Screen) -> Option<Visualid> {
    for depth in &screen.allowed_depths {
        if depth.depth != 32 {
            continue;
        }
        for visual in &depth.visuals {
            if visual.class == VisualClass::TRUE_COLOR {
                return Some(visual.visual_id);
            }
        }
    }
    None
}

fn create_inner(
    conn: &RustConnection,
    screen: &x11rb::protocol::xproto::Screen,
    visual: Visualid,
    width: u16,
    height: u16,
) -> Result<X11Overlay, String> {
    let root = screen.root;
    let window = conn.generate_id().map_err(|e| format!("id: {e}"))?;
    let colormap = conn.generate_id().map_err(|e| format!("id: {e}"))?;
    let pixmap = conn.generate_id().map_err(|e| format!("id: {e}"))?;
    let line_gc = conn.generate_id().map_err(|e| format!("id: {e}"))?;
    let clear_gc = conn.generate_id().map_err(|e| format!("id: {e}"))?;
    let copy_gc = conn.generate_id().map_err(|e| format!("id: {e}"))?;

    conn.create_colormap(ColormapAlloc::NONE, colormap, root, visual)
        .map_err(|e| format!("create_colormap: {e}"))?
        .check()
        .map_err(|e| format!("create_colormap: {e}"))?;

    let aux = CreateWindowAux::new()
        .background_pixmap(0)
        .border_pixel(0)
        .override_redirect(1)
        .event_mask(EventMask::NO_EVENT)
        .colormap(colormap);
    conn.create_window(
        32,
        window,
        root,
        0,
        0,
        width,
        height,
        0,
        WindowClass::INPUT_OUTPUT,
        visual,
        &aux,
    )
    .map_err(|e| format!("create_window: {e}"))?
    .check()
    .map_err(|e| format!("create_window: {e}"))?;

    // Empty the input shape so clicks pass through to windows below.
    conn.shape_rectangles(
        SO::SET,
        SK::INPUT,
        x11rb::protocol::xproto::ClipOrdering::UNSORTED,
        window,
        0,
        0,
        &[],
    )
    .map_err(|e| format!("shape_rectangles: {e}"))?
    .check()
    .map_err(|e| format!("shape_rectangles: {e}"))?;

    conn.create_pixmap(32, pixmap, window, width, height)
        .map_err(|e| format!("create_pixmap: {e}"))?
        .check()
        .map_err(|e| format!("create_pixmap: {e}"))?;

    let line_aux = CreateGCAux::new()
        .foreground(TRAIL_COLOR)
        .line_width(LINE_WIDTH)
        .graphics_exposures(0);
    conn.create_gc(line_gc, pixmap, &line_aux)
        .map_err(|e| format!("create_gc: {e}"))?
        .check()
        .map_err(|e| format!("create_gc: {e}"))?;

    let clear_aux = CreateGCAux::new().foreground(TRANSPARENT);
    conn.create_gc(clear_gc, pixmap, &clear_aux)
        .map_err(|e| format!("create_gc: {e}"))?
        .check()
        .map_err(|e| format!("create_gc: {e}"))?;

    let copy_aux = CreateGCAux::new().graphics_exposures(0);
    conn.create_gc(copy_gc, window, &copy_aux)
        .map_err(|e| format!("create_gc: {e}"))?
        .check()
        .map_err(|e| format!("create_gc: {e}"))?;

    Ok(X11Overlay {
        window,
        pixmap,
        line_gc,
        clear_gc,
        copy_gc,
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Visual smoke test: create the overlay, draw a sine curve and keep it
    /// visible for a few seconds. Requires a running X server.
    #[test]
    #[ignore]
    fn overlay_draws_visible_trail() {
        let (conn, screen_num) = x11rb::connect(None).expect("connect");
        let overlay = X11Overlay::create(&conn, screen_num)
            .expect("overlay create")
            .expect("create ok");
        overlay.show(&conn).expect("show");
        let pts: Vec<(f64, f64)> = (0..200)
            .map(|i| (100.0 + i as f64 * 4.0, 300.0 + (i as f64).sin() * 80.0))
            .collect();
        overlay.draw(&conn, &pts).expect("draw");
        std::thread::sleep(Duration::from_secs(3));
        overlay.hide(&conn).expect("hide");
    }
}