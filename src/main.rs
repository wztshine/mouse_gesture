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
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[mouse] {e}");
            ExitCode::FAILURE
        }
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
    let config = config::Config::load(&path)?;
    eprintln!(
        "[mouse] loaded {} rules, listening for gestures",
        count_rules(&config)
    );
    platform.run(&config)
}

fn count_rules(config: &config::Config) -> usize {
    config.default.len() + config.app.values().map(|m| m.len()).sum::<usize>()
}