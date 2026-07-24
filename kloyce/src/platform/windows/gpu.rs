use chrono::Utc;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::{broadcast, RwLock};

use crate::platform::GpuMetrics;

pub struct GpuMonitor {
    latest: Arc<RwLock<Option<GpuMetrics>>>,
    event_tx: broadcast::Sender<crate::web::SseEvent>,
    interval_ms: u64,
    gpu_name: String,
    power_limit_w: f64,
}

impl GpuMonitor {
    pub fn new(
        latest: Arc<RwLock<Option<GpuMetrics>>>,
        event_tx: broadcast::Sender<crate::web::SseEvent>,
        interval_ms: u64,
    ) -> Self {
        Self {
            latest,
            event_tx,
            interval_ms,
            gpu_name: String::new(),
            power_limit_w: 0.0,
        }
    }

    async fn query_static_info(&mut self) {
        let result = Command::new("nvidia-smi")
            .args([
                "--query-gpu=name,power.limit",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .await;

        match result {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout);
                let text = text.trim();
                let parts: Vec<&str> = text.splitn(2, ", ").collect();
                if parts.len() == 2 {
                    self.gpu_name = parts[0].trim().to_string();
                    self.power_limit_w = parts[1].trim().parse().unwrap_or(0.0);
                    tracing::info!(
                        "GPU detected: {} (power limit: {}W)",
                        self.gpu_name,
                        self.power_limit_w
                    );
                } else {
                    tracing::warn!("Unexpected nvidia-smi static output: {text}");
                    self.gpu_name = "Unknown GPU".to_string();
                    self.power_limit_w = 0.0;
                }
            }
            Ok(output) => {
                tracing::warn!(
                    "nvidia-smi static query failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                self.gpu_name = "Unknown GPU".to_string();
                self.power_limit_w = 0.0;
            }
            Err(_) => {
                self.gpu_name = "Unknown GPU".to_string();
                self.power_limit_w = 0.0;
            }
        }
    }

    pub async fn run(mut self) {
        // Check if nvidia-smi is available
        match Command::new("nvidia-smi").arg("--version").output().await {
            Ok(output) if output.status.success() => {}
            _ => {
                tracing::warn!("nvidia-smi not found or not working, GPU monitoring disabled");
                return;
            }
        }

        self.query_static_info().await;

        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(self.interval_ms)).await;

            let result = Command::new("nvidia-smi")
                .args([
                    "--query-gpu=utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,fan.speed",
                    "--format=csv,noheader,nounits",
                ])
                .output()
                .await;

            match result {
                Ok(output) if output.status.success() => {
                    let text = String::from_utf8_lossy(&output.stdout);
                    let text = text.trim();
                    let parts: Vec<&str> = text.split(", ").collect();

                    if parts.len() == 6 {
                        let metrics = GpuMetrics {
                            gpu_name: self.gpu_name.clone(),
                            utilization_pct: parts[0].trim().parse().unwrap_or(0),
                            vram_used_mb: parts[1].trim().parse().unwrap_or(0),
                            vram_total_mb: parts[2].trim().parse().unwrap_or(0),
                            temperature_c: parts[3].trim().parse().unwrap_or(0),
                            power_draw_w: parts[4].trim().parse().unwrap_or(0.0),
                            power_limit_w: self.power_limit_w,
                            fan_speed_pct: parts[5]
                                .trim()
                                .strip_suffix(" %")
                                .unwrap_or(parts[5].trim())
                                .parse()
                                .unwrap_or(0),
                            timestamp: Utc::now(),
                        };

                        *self.latest.write().await = Some(metrics.clone());

                        let _ = self.event_tx.send(crate::web::SseEvent::GpuMetrics {
                            gpu_name: metrics.gpu_name,
                            utilization_pct: metrics.utilization_pct,
                            vram_used_mb: metrics.vram_used_mb,
                            vram_total_mb: metrics.vram_total_mb,
                            temperature_c: metrics.temperature_c,
                            power_draw_w: metrics.power_draw_w,
                            power_limit_w: metrics.power_limit_w,
                            fan_speed_pct: metrics.fan_speed_pct,
                            timestamp: metrics.timestamp,
                        });
                    } else {
                        tracing::warn!("Unexpected nvidia-smi output format: {text}");
                    }
                }
                Ok(output) => {
                    tracing::warn!(
                        "nvidia-smi query failed: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                }
                Err(e) => {
                    tracing::warn!("Failed to run nvidia-smi: {e}");
                }
            }
        }
    }
}
