use reqwest_eventsource::{Event, RequestBuilderExt};
use tauri::AppHandle;
use tokio_stream::StreamExt;

const DAEMON_URL: &str = "http://127.0.0.1:9876";

pub async fn listen(app: AppHandle) {
    // Fetch initial state
    fetch_initial_state(&app).await;

    // SSE reconnection loop
    loop {
        tracing::info!("SSE connecting to daemon...");
        let client = reqwest::Client::new();
        let mut es = client
            .get(format!("{DAEMON_URL}/api/events"))
            .eventsource()
            .unwrap();

        while let Some(event) = es.next().await {
            match event {
                Ok(Event::Open) => {
                    tracing::info!("SSE connected to daemon");
                }
                Ok(Event::Message(msg)) => {
                    handle_message(&app, &msg.data);
                }
                Err(reqwest_eventsource::Error::StreamEnded) => {
                    tracing::warn!("SSE stream ended");
                    break;
                }
                Err(e) => {
                    tracing::warn!("SSE error: {e}");
                    break;
                }
            }
        }

        es.close();
        tracing::info!("SSE reconnecting in 3s...");
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

async fn fetch_initial_state(app: &AppHandle) {
    let url = format!("{DAEMON_URL}/api/status");
    match reqwest::get(&url).await {
        Ok(resp) => match resp.json::<serde_json::Value>().await {
            Ok(json) => {
                if let Some(state) = json.get("state").and_then(|s| s.as_str()) {
                    if let Some(tray) = app.tray_by_id("kloyce-tray") {
                        crate::tray::update_icon(&tray, state);
                    }
                }
            }
            Err(e) => tracing::warn!("Failed to parse status response: {e}"),
        },
        Err(e) => {
            tracing::warn!("Failed to fetch initial state: {e}");
        }
    }
}

fn handle_message(app: &AppHandle, data: &str) {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else {
        return;
    };

    if json.get("type").and_then(|t| t.as_str()) == Some("state_change") {
        if let Some(state) = json.get("state").and_then(|s| s.as_str()) {
            tracing::debug!("State changed to: {state}");
            if let Some(tray) = app.tray_by_id("kloyce-tray") {
                crate::tray::update_icon(&tray, state);
            }
        }
    }
}
