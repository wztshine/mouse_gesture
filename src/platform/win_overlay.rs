use std::ptr::null_mut;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreatePen, Polyline, SelectObject, SetBkMode,
    AC_SRC_ALPHA, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBRUSH,
    HDC, HGDIOBJ, PEN_STYLE, RGBQUAD,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetSystemMetrics, RegisterClassW, SetWindowPos, ShowWindow,
    HWND_TOPMOST, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, ULW_ALPHA,
    UpdateLayeredWindow, WINDOW_EX_STYLE, WNDCLASSW, WM_NCHITTEST, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};

/// Trail color as a COLORREF (0x00BBGGRR), fully opaque green.
const TRAIL_COLOR: u32 = 0x0000_FF00;
/// Trail line width in pixels.
const LINE_WIDTH: i32 = 3;

/// Layered fullscreen overlay that renders the gesture trail on Windows.
///
/// Owns a topmost layered window backed by a 32-bit DIB section. The window is
/// shown once and kept permanently visible; each gesture paints the trail into
/// the DIB with GDI and pushes it to the screen via `UpdateLayeredWindow`, and
/// on gesture end the pixels are cleared back to transparent. The window is
/// never hidden between gestures: Win10 can fail to composite a layered
/// window that is repeatedly hidden and re-shown, leaving the trail invisible
/// even though `UpdateLayeredWindow` reports success.
pub struct WinOverlay {
    hwnd: HWND,
    mem_dc: HDC,
    bits: *mut u32,
    width: i32,
    height: i32,
    offset_x: i32,
    offset_y: i32,
    prev_bbox: Option<(i32, i32, i32, i32)>,
}

/// WinOverlay must be created and used on the same thread: GDI objects (the
/// DC, DIB and pen) are thread-affine. The overlay worker thread does both.
unsafe impl Send for WinOverlay {}
unsafe impl Sync for WinOverlay {}

/// Window procedure: hand everything to the default handler. Hit testing
/// always reports transparent so the always-visible overlay never intercepts
/// mouse input (clicks must fall through to the window underneath).
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCHITTEST {
        return LRESULT(-1); // HTTRANSPARENT
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl WinOverlay {
    /// Create the overlay window covering the primary screen.
    ///
    /// :return: The overlay, or None when window creation fails.
    pub fn create() -> Option<Result<WinOverlay, String>> {
        Some(create_inner())
    }

    /// Begin a gesture: clear any leftover trail and (re-)assert the topmost
    /// Z-order so the trail paints above the active window.
    pub fn show(&self) -> Result<(), String> {
        // Clear any leftover trail from the previous gesture.
        let n = (self.width * self.height) as usize;
        unsafe {
            std::ptr::write_bytes(self.bits, 0, n);
        }
        self.push()
            .map_err(|e| format!("failed to clear overlay: {e}"))?;
        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOMOVE,
            )
        };
        let _ = unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
        Ok(())
    }

    /// Redraw the overlay with the given trail points.
    pub fn draw(&mut self, points: &[(f64, f64)]) -> Result<(), String> {
        // Clear only the region the previous trail occupied (grown by the pen
        // width) instead of the whole screen, which is costly on large
        // multi-monitor DPI-scaled surfaces.
        if let Some((px, py, pw, ph)) = self.prev_bbox {
            self.zero_region(px, py, pw, ph);
        }

        if points.len() >= 2 {
            let pts: Vec<windows::Win32::Foundation::POINT> = points
                .iter()
                .map(|&(x, y)| windows::Win32::Foundation::POINT {
                    x: x as i32 - self.offset_x,
                    y: y as i32 - self.offset_y,
                })
                .collect();
            let ok = unsafe { Polyline(self.mem_dc, &pts) };
            if !ok.as_bool() {
                return Err("failed to draw trail".into());
            }
            // GDI leaves the alpha channel at 0; mark drawn pixels opaque so
            // they become visible through UpdateLayeredWindow. Only the
            // bounding box of the trail is scanned to keep this cheap.
            let margin = LINE_WIDTH;
            let min_x = (pts.iter().map(|p| p.x).min().unwrap_or(0) - margin).max(0);
            let min_y = (pts.iter().map(|p| p.y).min().unwrap_or(0) - margin).max(0);
            let max_x = (pts.iter().map(|p| p.x).max().unwrap_or(0) + margin).min(self.width - 1);
            let max_y = (pts.iter().map(|p| p.y).max().unwrap_or(0) + margin).min(self.height - 1);
            self.prev_bbox = Some((min_x, min_y, max_x, max_y));
            self.set_alpha(min_x, min_y, max_x, max_y);
        }

        self.push().map_err(|e| format!("failed to update overlay: {e}"))
    }

    /// Hide the overlay. The window itself stays visible (it is shown once at
    /// startup and never hidden, to avoid Win10 layered-window update quirks
    /// from repeated show/hide cycles); "hiding" just clears the trail pixels
    /// back to transparent.
    pub fn hide(&mut self) -> Result<(), String> {
        if let Some((px, py, pw, ph)) = self.prev_bbox.take() {
            self.zero_region(px, py, pw, ph);
            self.push()
                .map_err(|e| format!("failed to clear overlay: {e}"))?;
        }
        Ok(())
    }

    /// Zero out all pixels (color + alpha) in the given region so the trail
    /// disappears. Coordinates are in window space, inclusive.
    fn zero_region(&self, x0: i32, y0: i32, x1: i32, y1: i32) {
        if x0 > x1 || y0 > y1 {
            return;
        }
        let row_len = self.width as usize;
        let x0 = x0.max(0) as usize;
        let y0 = y0.max(0) as usize;
        let x1 = x1.min(self.width - 1) as usize;
        let y1 = y1.min(self.height - 1) as usize;
        let len = x1 - x0 + 1;
        unsafe {
            for y in y0..=y1 {
                let base = y * row_len + x0;
                std::ptr::write_bytes(self.bits.add(base), 0, len);
            }
        }
    }

    /// Force the alpha channel to opaque for every non-transparent pixel in a
    /// region, revealing the GDI-drawn line through the layered window.
    fn set_alpha(&self, x0: i32, y0: i32, x1: i32, y1: i32) {
        if x0 > x1 || y0 > y1 {
            return;
        }
        let x0 = x0.max(0) as usize;
        let y0 = y0.max(0) as usize;
        let x1 = x1.min(self.width - 1) as usize;
        let y1 = y1.min(self.height - 1) as usize;
        let row_len = self.width as usize;
        unsafe {
            for y in y0..=y1 {
                let base = y * row_len;
                for x in x0..=x1 {
                    let i = base + x;
                    let px = *self.bits.add(i);
                    if px & 0x00FF_FFFF != 0 {
                        *self.bits.add(i) = px | 0xFF00_0000;
                    }
                }
            }
        }
    }

    /// Composite the DIB backing store into the layered window.
    fn push(&self) -> windows::core::Result<()> {
        let size = windows::Win32::Foundation::SIZE {
            cx: self.width,
            cy: self.height,
        };
        let pt_src = windows::Win32::Foundation::POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: 0,             // AC_SRC_OVER
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            // pptDst = None keeps the window at its creation position.
            UpdateLayeredWindow(
                self.hwnd,
                None,
                None,
                Some(&size),
                Some(self.mem_dc),
                Some(&pt_src),
                windows::Win32::Foundation::COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        }
    }
}

fn create_inner() -> Result<WinOverlay, String> {
    // Cover the whole virtual screen (all monitors). The mouse hook reports
    // physical pixels; with DPI awareness set at startup the metrics below are
    // also physical, so window geometry and pointer coordinates match.
    let offset_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let offset_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width <= 0 || height <= 0 {
        return Err("invalid screen size".into());
    }

    let class_name = windows::core::w!("mouse_trail_overlay");
    let hinstance = unsafe { GetModuleHandleW(None) }
        .map_err(|e| format!("module: {e}"))?
        .into();

    let wc = WNDCLASSW {
        style: Default::default(),
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: Default::default(),
        hCursor: Default::default(),
        hbrBackground: HBRUSH(null_mut()),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: class_name,
    };
    let registered = unsafe { RegisterClassW(&wc) };
    if registered == 0 {
        return Err("failed to register window class".into());
    }

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(
                (WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0)
                    as u32,
            ),
            class_name,
            w!(""),
            WS_POPUP,
            offset_x,
            offset_y,
            width,
            height,
            None,
            None,
            Some(hinstance),
            None,
        )
    }
    .map_err(|e| format!("create window: {e}"))?;
    if hwnd.is_invalid() {
        return Err("failed to create overlay window".into());
    }

    let mem_dc = unsafe { CreateCompatibleDC(None) };
    if mem_dc.is_invalid() {
        return Err("failed to create memory DC".into());
    }

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [RGBQUAD {
            rgbBlue: 0,
            rgbGreen: 0,
            rgbRed: 0,
            rgbReserved: 0,
        }],
    };
    let mut bits: *mut core::ffi::c_void = null_mut();
    let bitmap = unsafe {
        CreateDIBSection(
            Some(mem_dc),
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        )
    }
    .map_err(|e| format!("create DIB: {e}"))?;
    if bitmap.is_invalid() {
        return Err("failed to create DIB section".into());
    }

    let _old_bitmap = unsafe { SelectObject(mem_dc, HGDIOBJ(bitmap.0)) };

    let pen = unsafe { CreatePen(PEN_STYLE(0), LINE_WIDTH, windows::Win32::Foundation::COLORREF(TRAIL_COLOR)) };
    if pen.is_invalid() {
        return Err("failed to create pen".into());
    }
    let _old_pen = unsafe { SelectObject(mem_dc, HGDIOBJ(pen.0)) };
    unsafe { SetBkMode(mem_dc, windows::Win32::Graphics::Gdi::BACKGROUND_MODE(1)) }; // TRANSPARENT

    Ok(WinOverlay {
        hwnd,
        mem_dc,
        bits: bits as *mut u32,
        width,
        height,
        offset_x,
        offset_y,
        prev_bbox: None,
    })
}