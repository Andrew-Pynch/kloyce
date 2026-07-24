use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

const ICON_IDLE: &[u8] = include_bytes!("../icons/idle.png");
const ICON_RECORDING: &[u8] = include_bytes!("../icons/recording.png");
const ICON_TRANSCRIBING: &[u8] = include_bytes!("../icons/transcribing.png");

pub fn create(app: &AppHandle) -> Result<TrayIcon, Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, "show", "Show Dashboard", true, None::<&str>)?;
    let toggle = MenuItem::with_id(
        app,
        "toggle",
        "Toggle Recording  \u{2318}\u{21e7}R",
        true,
        None::<&str>,
    )?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &toggle, &sep, &quit])?;

    let icon = Image::from_bytes(ICON_IDLE)?;

    let tray = TrayIconBuilder::with_id("kloyce-tray")
        .icon(icon)
        .tooltip("Kloyce - Idle")
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => show_window(app),
            "toggle" => {
                let _app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::ipc_client::send_toggle().await {
                        tracing::error!("Failed to send toggle: {e}");
                    }
                });
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                    } else {
                        show_window(app);
                    }
                }
            }
        })
        .build(app)?;

    Ok(tray)
}

pub fn update_icon(tray: &TrayIcon, state: &str) {
    let (icon_bytes, tooltip) = match state {
        "recording" => (ICON_RECORDING, "Kloyce - Recording"),
        "transcribing" => (ICON_TRANSCRIBING, "Kloyce - Transcribing"),
        _ => (ICON_IDLE, "Kloyce - Idle"),
    };

    if let Ok(icon) = Image::from_bytes(icon_bytes) {
        let _ = tray.set_icon(Some(icon));
    }
    let _ = tray.set_tooltip(Some(tooltip));
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
