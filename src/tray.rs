//! System tray icon with a small menu.
//!
//! `tray-icon` handles its own message loop on Windows, so the tray works
//! without touching the winit event loop. Events are polled by the UI from
//! the global receivers each frame.

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// Actions requested through the tray, polled by the UI thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Quit,
}

pub struct Tray {
    _tray: TrayIcon,
    show_id: MenuId,
    quit_id: MenuId,
}

/// A simple 32×32 RGBA icon: rounded dark square with a light note stem.
fn icon_rgba() -> Vec<u8> {
    let size: usize = 32;
    let mut rgba = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let i = (y * size + x) * 4;
            // Rounded-square mask
            let dx = (x as i32 - 16).abs();
            let dy = (y as i32 - 16).abs();
            let inside_square = dx <= 14 && dy <= 14;
            let corner = dx > 12 && dy > 12 && (dx - 12) * (dx - 12) + (dy - 12) * (dy - 12) > 8;
            if inside_square && !corner {
                // Body: deep violet like a music app
                rgba[i] = 88;
                rgba[i + 1] = 101;
                rgba[i + 2] = 242;
                rgba[i + 3] = 255;
            }
            // Note stem: vertical light bar
            if (19..=21).contains(&x) && (7..=20).contains(&y) {
                rgba[i] = 245;
                rgba[i + 1] = 245;
                rgba[i + 2] = 255;
                rgba[i + 3] = 255;
            }
            // Note head: small filled ellipse
            let hx = (x as i32 - 16) * (x as i32 - 16);
            let hy = (y as i32 - 21) * (y as i32 - 21);
            if hx <= 9 && hy <= 12 && hx * 3 + hy * 2 <= 40 {
                rgba[i] = 245;
                rgba[i + 1] = 245;
                rgba[i + 2] = 255;
                rgba[i + 3] = 255;
            }
        }
    }
    rgba
}

impl Tray {
    pub fn new() -> Result<Self, String> {
        let menu = Menu::new();
        let show = MenuItem::new("Открыть MusicPresence", true, None);
        let quit = MenuItem::new("Выход", true, None);
        menu.append(&show).map_err(|e| e.to_string())?;
        menu.append(&quit).map_err(|e| e.to_string())?;

        let icon = Icon::from_rgba(icon_rgba(), 32, 32).map_err(|e| e.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .with_tooltip("MusicPresence — слушаем музыку")
            .build()
            .map_err(|e| format!("создание трея: {e}"))?;
        Ok(Self {
            _tray: tray,
            show_id: show.id().clone(),
            quit_id: quit.id().clone(),
        })
    }

    /// Non-blocking poll of menu and double-click events.
    pub fn poll_action(&self) -> Option<TrayAction> {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.show_id {
                return Some(TrayAction::Show);
            }
            if event.id == self.quit_id {
                return Some(TrayAction::Quit);
            }
        }
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            // Double-click on the icon shows the window.
            if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                return Some(TrayAction::Show);
            }
        }
        None
    }
}
