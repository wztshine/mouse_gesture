// Compile as a GUI-subsystem app on Windows so double-clicking the binary
// does not open a console window. When run from a terminal the console is
// re-attached at startup so log output still appears.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::process::ExitCode;

mod action;
mod config;
mod gesture;
mod platform;

#[cfg(target_os = "linux")]
use platform::linux::LinuxPlatform;
#[cfg(target_os = "windows")]
use platform::windows::WindowsPlatform;

use crate::platform::{config_path, Platform};

fn main() -> ExitCode {
    #[cfg(target_os = "windows")]
    attach_to_parent_console();
    #[cfg(all(target_os = "windows", feature = "trail"))]
    set_dpi_awareness();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[mouse] {e}");
            ExitCode::FAILURE
        }
    }
}

/// Attach to the parent console when launched from a terminal. When launched
/// by double-clicking there is no parent console and this is a no-op, leaving
/// the program windowless. Rust's stdout/stderr obtain their handles lazily,
/// so attaching before the first print routes log output to the console.
#[cfg(target_os = "windows")]
fn attach_to_parent_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Make the process per-monitor DPI aware so screen metrics and pointer
/// coordinates are both physical pixels (needed for the trail overlay).
#[cfg(all(target_os = "windows", feature = "trail"))]
fn set_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let identify = args.iter().any(|a| a == "--identify" || a == "identify");
    #[cfg(feature = "trail")]
    let overlay_test = args.iter().any(|a| a == "--overlay-test");

    #[cfg(target_os = "linux")]
    let mut platform = LinuxPlatform::new()?;
    #[cfg(target_os = "windows")]
    let mut platform = WindowsPlatform::new();

    if identify {
        loop {
            match platform.foreground_app() {
                Some(app) => println!("{app}"),
                None => println!("<unknown>"),
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }

    #[cfg(feature = "trail")]
    if overlay_test {
        return run_overlay_test();
    }

    let path = config_path()?;
    let config = config::Config::load(&path)?;
    eprintln!(
        "[mouse] loaded {} rules, listening for gestures",
        count_rules(&config)
    );
    platform.run(&config)
}

/// Draw a temporary sine curve trail for a few seconds so the overlay can be
/// verified independently of gesture capture.
#[cfg(all(target_os = "linux", feature = "trail"))]
fn run_overlay_test() -> Result<(), String> {
    use x11rb::connect;
    let (conn, screen_num) = connect(None).map_err(|e| format!("connect: {e}"))?;
    let overlay = crate::platform::x11_overlay::X11Overlay::create(&conn, screen_num)
        .ok_or("no 32-bit visual")?
        .map_err(|e| format!("create: {e}"))?;
    overlay.show(&conn)?;
    let pts: Vec<(f64, f64)> = (0..200)
        .map(|i| (100.0 + i as f64 * 4.0, 300.0 + (i as f64).sin() * 80.0))
        .collect();
    overlay.draw(&conn, &pts)?;
    std::thread::sleep(std::time::Duration::from_secs(3));
    overlay.hide(&conn)?;
    Ok(())
}

/// Draw a temporary sine curve trail for a few seconds so the overlay can be
/// verified independently of gesture capture.
#[cfg(all(target_os = "windows", feature = "trail"))]
fn run_overlay_test() -> Result<(), String> {
    let mut overlay = crate::platform::win_overlay::WinOverlay::create()
        .ok_or("overlay not available")?
        .map_err(|e| format!("create: {e}"))?;
    overlay.show()?;
    let pts: Vec<(f64, f64)> = (0..200)
        .map(|i| (100.0 + i as f64 * 4.0, 300.0 + (i as f64).sin() * 80.0))
        .collect();
    overlay.draw(&pts)?;
    std::thread::sleep(std::time::Duration::from_secs(3));
    overlay.hide()?;
    Ok(())
}

fn count_rules(config: &config::Config) -> usize {
    config.default.len() + config.app.values().map(|m| m.len()).sum::<usize>()
}