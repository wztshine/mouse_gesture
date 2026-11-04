//! System tray icon with a quit action.
//!
//! Linux uses the D-Bus StatusNotifierItem backend (`ksni` feature), so no GTK
//! or libappindicator is required.

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::platform::QuitHandle;

const QUIT_ID: &str = "quit";
const ICON_SIZE: u32 = 32;
/// Tray icon color (soft blue).
const ICON_COLOR: [u8; 3] = [0x5a, 0xa9, 0xe6];

/// Keeps the tray icon alive; dropping it removes the icon.
pub struct Tray {
    _icon: TrayIcon,
}

impl Tray {
    /// Build the tray icon and wire its Quit item to `quit`.
    ///
    /// Must be called on the thread that runs the message/event loop. Returns
    /// `None` when no tray is available; the caller should keep running.
    pub fn start(quit: QuitHandle) -> Option<Self> {
        let quit_item = MenuItem::with_id(QUIT_ID, "Quit", true, None);
        let menu = Menu::new();
        menu.append(&quit_item).ok()?;

        let icon = TrayIconBuilder::new()
            .with_icon(make_icon()?)
            .with_tooltip("mouse")
            .with_menu(Box::new(menu))
            .build()
            .ok()?;

        let quit_id = MenuId::new(QUIT_ID);
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if event.id == quit_id {
                quit.request();
            }
        }));

        Some(Self { _icon: icon })
    }
}

/// A soft blue dot, drawn programmatically so there are no asset files.
fn make_icon() -> Option<Icon> {
    let mut rgba = vec![0u8; (ICON_SIZE * ICON_SIZE * 4) as usize];
    let center = (ICON_SIZE as f32 - 1.0) / 2.0;
    let radius = center;

    for y in 0..ICON_SIZE {
        for x in 0..ICON_SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            if distance > radius + 1.0 {
                continue;
            }

            let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0);
            let offset = ((y * ICON_SIZE + x) * 4) as usize;
            rgba[offset] = ICON_COLOR[0];
            rgba[offset + 1] = ICON_COLOR[1];
            rgba[offset + 2] = ICON_COLOR[2];
            rgba[offset + 3] = (coverage * 255.0) as u8;
        }
    }

    Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE).ok()
}
