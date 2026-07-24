mod ipc_client;
mod sse;
mod tray;

use tauri::Manager;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kloyce_app=info".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let handle = app.handle();
            tray::create(handle)?;

            app.global_shortcut().on_shortcut(
                "CommandOrControl+Shift+R",
                |_app, _shortcut, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = crate::ipc_client::send_toggle().await {
                                tracing::error!("Hotkey toggle failed: {e}");
                            }
                        });
                    }
                },
            )?;

            app.global_shortcut().on_shortcut(
                "CommandOrControl+Shift+E",
                |_app, _shortcut, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) = crate::ipc_client::send_toggle_enter().await {
                                tracing::error!("Hotkey toggle_enter failed: {e}");
                            }
                        });
                    }
                },
            )?;

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(sse::listen(handle));

            // Show the dashboard window on first launch
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error building kloyce app")
        .run(|_app, _event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } = _event
            {
                if !has_visible_windows {
                    if let Some(window) = _app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                }
            }
        });
}
