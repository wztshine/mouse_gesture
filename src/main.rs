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

fn run(args: Vec<String>) -> Result<(), String> {
    let identify = args.iter().any(|a| a == "--identify" || a == "identify");

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

    let path = config_path()?;
    let config = config::Config::init_shared(&path)?;
    // Reload the config every 5 seconds when the file changes, so edits take
    // effect without restarting the program.
    config::Config::watch(path.clone(), std::time::Duration::from_secs(5));
    eprintln!(
        "[mouse] loaded {} rules, listening for gestures (config auto-reload: 5s)",
        count_rules(&config)
    );
    platform.run()
}

fn count_rules(config: &config::Config) -> usize {
    config.default.len() + config.app.values().map(|m| m.len()).sum::<usize>()
}