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
/// trail only grows while a gesture is in progress, so it is painted
/// incrementally into a backing pixmap (only the segments added since the
/// previous call) and just the newly covered region is copied to the
/// window. Per-mouse-move cost is therefore independent of screen size and
/// never waits on the server. Its input shape is emptied so it never
/// intercepts clicks.
pub struct X11Overlay {
    window: Window,
    pixmap: Pixmap,
    line_gc: Gcontext,
    clear_gc: Gcontext,
    copy_gc: Gcontext,
    width: u16,
    height: u16,
    /// Number of trail points already painted into the backing pixmap.
    /// Drawing resumes after this point, so each mouse-move event paints
    /// only the segments added since the last call.
    drawn_points: usize,
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

    /// Show the overlay (clear any leftover trail, blit the empty trail to
    /// the window, then map and raise it). Called at gesture start.
    pub fn show(&mut self, conn: &RustConnection) -> Result<(), String> {
        // Any leftover trail from the previous gesture is about to be
        // cleared; incremental drawing restarts from scratch even if a
        // later step of this call fails.
        self.drawn_points = 0;
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

    /// Extend the trail with the points added since the previous call.
    ///
    /// Previously painted segments stay valid (the trail never shrinks
    /// during a gesture), so this draws just the new segments -- resuming
    /// from the last painted point so the polyline joins the existing
    /// trail -- and blits only the rectangle they cover. Requests are sent
    /// without `check()` round trips: synchronizing with the server on
    /// every mouse-move event would stall the event loop behind a fast
    /// pointer. Transport-level errors still surface through `flush`.
    pub fn draw(&mut self, conn: &RustConnection, points: &[(f64, f64)]) -> Result<(), String> {
        if points.len() < 2 {
            return Ok(()); // a single point has no segment to draw
        }
        let start = segment_start(self.drawn_points, points.len());
        let new_points = &points[start..];
        if new_points.len() < 2 {
            return Ok(()); // nothing new since the last draw
        }

        let pts: Vec<_> = new_points
            .iter()
            .map(|&(x, y)| x11rb::protocol::xproto::Point {
                x: x as i16,
                y: y as i16,
            })
            .collect();
        conn.poly_line(CoordMode::ORIGIN, self.pixmap, self.line_gc, &pts)
            .map_err(|e| format!("failed to draw trail: {e}"))?;

        // Blit only the rectangle the new segments occupy (grown by the pen
        // width) instead of the whole screen.
        let margin = LINE_WIDTH as i32;
        let min_x = (pts.iter().map(|p| p.x as i32).min().unwrap_or(0) - margin).max(0);
        let min_y = (pts.iter().map(|p| p.y as i32).min().unwrap_or(0) - margin).max(0);
        let max_x = (pts.iter().map(|p| p.x as i32).max().unwrap_or(0) + margin)
            .min(self.width as i32 - 1);
        let max_y = (pts.iter().map(|p| p.y as i32).max().unwrap_or(0) + margin)
            .min(self.height as i32 - 1);
        if min_x <= max_x && min_y <= max_y {
            conn.copy_area(
                self.pixmap,
                self.window,
                self.copy_gc,
                min_x as i16,
                min_y as i16,
                min_x as i16,
                min_y as i16,
                (max_x - min_x + 1) as u16,
                (max_y - min_y + 1) as u16,
            )
            .map_err(|e| format!("failed to blit trail: {e}"))?;
        }

        self.drawn_points = points.len();
        conn.flush()
            .map_err(|e| format!("failed to flush overlay: {e}"))
    }

    /// Hide the overlay and clear the trail.
    pub fn hide(&self, conn: &RustConnection) -> Result<(), String> {
        conn.unmap_window(self.window)
            .map_err(|e| format!("failed to unmap overlay: {e}"))?
            .check()
            .map_err(|e| format!("failed to unmap overlay: {e}"))
    }
}

/// Index of the first point to paint in this round: the last painted point
/// (index `drawn_points - 1`) so the new polyline joins the existing trail,
/// or 0 for a fresh trail. Clamped against `points_len` so a stale count
/// degrades to "nothing new" instead of panicking.
fn segment_start(drawn_points: usize, points_len: usize) -> usize {
    drawn_points
        .saturating_sub(1)
        .min(points_len.saturating_sub(1))
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
        drawn_points: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn segment_start_resumes_at_the_junction_point() {
        // Fresh trail: paint from the very first point.
        assert_eq!(segment_start(0, 2), 0);
        // drawn_points = k means points[0..k] are painted; the next polyline
        // must start at the junction point k-1, or segments would be skipped.
        assert_eq!(segment_start(2, 3), 1);
        assert_eq!(segment_start(3, 4), 2);
        // No new points: caller bails out on the < 2 slice length.
        assert_eq!(segment_start(5, 5), 4);
        // Stale count is clamped instead of panicking.
        assert_eq!(segment_start(9, 4), 3);
        assert_eq!(segment_start(0, 0), 0);
    }

    /// Visual smoke test: create the overlay, draw a sine curve and keep it
    /// visible for a few seconds. Requires a running X server.
    #[test]
    #[ignore]
    fn overlay_draws_visible_trail() {
        let (conn, screen_num) = x11rb::connect(None).expect("connect");
        let mut overlay = X11Overlay::create(&conn, screen_num)
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