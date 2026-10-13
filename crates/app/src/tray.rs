//! Tray icon with a small menu, so the app keeps running when its window is
//! closed.

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const ICON_SIZE: u32 = 32;
const ICON_RGBA: &[u8] = include_bytes!("../assets/tray-32.rgba");

pub enum TrayAction {
    Show,
    Optimize,
    ToggleGameMode,
    Quit,
}

pub struct Tray {
    icon: TrayIcon,
    game_mode: CheckMenuItem,
    open: MenuItem,
    optimize: MenuItem,
    quit: MenuItem,
}

impl Tray {
    /// Must be called once the event loop is running (macOS requires it).
    pub fn new() -> Result<Self, String> {
        let open = MenuItem::new("Open RAM Optimizer", true, None);
        let optimize = MenuItem::new("Optimize now", true, None);
        let game_mode = CheckMenuItem::new("Game mode", true, false, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &open,
            &PredefinedMenuItem::separator(),
            &optimize,
            &game_mode,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|e| e.to_string())?;

        let image = Icon::from_rgba(ICON_RGBA.to_vec(), ICON_SIZE, ICON_SIZE).map_err(|e| e.to_string())?;
        let icon = TrayIconBuilder::new()
            .with_icon(image)
            .with_tooltip("RAM Optimizer")
            .with_menu(Box::new(menu))
            // Left click opens the window; right click shows the menu.
            .with_menu_on_left_click(false)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { icon, game_mode, open, optimize, quit })
    }

    /// Clicks and menu choices since the last call.
    pub fn actions(&self) -> Vec<TrayAction> {
        let mut actions = Vec::new();
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            let left_click = matches!(
                event,
                TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
                    | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. }
            );
            if left_click {
                actions.push(TrayAction::Show);
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id();
            if id == self.open.id() {
                actions.push(TrayAction::Show);
            } else if id == self.optimize.id() {
                actions.push(TrayAction::Optimize);
            } else if id == self.game_mode.id() {
                actions.push(TrayAction::ToggleGameMode);
            } else if id == self.quit.id() {
                actions.push(TrayAction::Quit);
            }
        }
        actions
    }

    pub fn update(&self, tooltip: &str, game_mode: bool) {
        let _ = self.icon.set_tooltip(Some(tooltip));
        self.game_mode.set_checked(game_mode);
    }
}
