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
    vram_total_mb: u64,
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
            vram_total_mb: 0,
        }
    }

    async fn query_static_info(&mut self) -> bool {
        // Get GPU model and core count from ioreg
        let ioreg_result = Command::new("ioreg")
            .args(["-r", "-d", "1", "-c", "IOAccelerator"])
            .output()
            .await;

        let mut model = String::new();
        let mut core_count: u32 = 0;

        if let Ok(output) = ioreg_result {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("\"model\"") {
                        // Format: "model" = "Apple M4 Max"
                        if let Some(eq) = trimmed.find('=') {
                            let val = trimmed[eq + 1..].trim().trim_matches('"');
                            if !val.is_empty() {
                                model = val.to_string();
                            }
                        }
                    } else if trimmed.starts_with("\"gpu-core-count\"") {
                        // Format: "gpu-core-count" = 40
                        if let Some(eq) = trimmed.find('=') {
                            let val = trimmed[eq + 1..].trim();
                            core_count = val.parse().unwrap_or(0);
                        }
                    }
                }
            }
        }

        if model.is_empty() {
            tracing::warn!("Could not detect Apple Silicon GPU via ioreg");
            return false;
        }

        // Get total unified memory via sysctl
        let sysctl_result = Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .await;

        if let Ok(output) = sysctl_result {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                if let Ok(bytes) = text.trim().parse::<u64>() {
                    self.vram_total_mb = bytes / (1024 * 1024);
                }
            }
        }

        self.gpu_name = if core_count > 0 {
            format!("{model} ({core_count} cores)")
        } else {
            model
        };

        tracing::info!(
            "GPU detected: {} (unified memory: {} MB)",
            self.gpu_name,
            self.vram_total_mb
        );
        true
    }

    fn parse_performance_stats(text: &str) -> (u32, u64) {
        let mut utilization: u32 = 0;
        let mut mem_used_bytes: u64 = 0;

        // PerformanceStatistics is a single-line dict like:
        // "PerformanceStatistics" = {"In use system memory"=1683947520,"Device Utilization %"=0,...}
        for line in text.lines() {
            let trimmed = line.trim();
            if !trimmed.contains("\"PerformanceStatistics\"") {
                continue;
            }
            // Extract the dict content between { and }
            if let Some(start) = trimmed.find('{') {
                if let Some(end) = trimmed.rfind('}') {
                    let dict = &trimmed[start + 1..end];
                    // Parse comma-separated key=value pairs
                    for pair in dict.split(',') {
                        let pair = pair.trim();
                        if let Some(eq) = pair.find('=') {
                            let key = pair[..eq].trim().trim_matches('"');
                            let val = pair[eq + 1..].trim();
                            match key {
                                "Device Utilization %" => {
                                    utilization = val.parse().unwrap_or(0);
                                }
                                "In use system memory" => {
                                    mem_used_bytes = val.parse().unwrap_or(0);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            break;
        }

        let mem_used_mb = mem_used_bytes / (1024 * 1024);
        (utilization, mem_used_mb)
    }

    pub async fn run(mut self) {
        if !self.query_static_info().await {
            tracing::warn!("Apple Silicon GPU not detected, GPU monitoring disabled");
            return;
        }

        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(self.interval_ms)).await;

            let result = Command::new("ioreg")
                .args(["-r", "-d", "1", "-c", "IOAccelerator"])
                .output()
                .await;

            match result {
                Ok(output) if output.status.success() => {
                    let text = String::from_utf8_lossy(&output.stdout);
                    let (utilization_pct, vram_used_mb) = Self::parse_performance_stats(&text);

                    let metrics = GpuMetrics {
                        gpu_name: self.gpu_name.clone(),
                        utilization_pct,
                        vram_used_mb,
                        vram_total_mb: self.vram_total_mb,
                        temperature_c: 0,
                        power_draw_w: 0.0,
                        power_limit_w: 0.0,
                        fan_speed_pct: 0,
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
                }
                Ok(output) => {
                    tracing::warn!(
                        "ioreg query failed: {}",
                        String::from_utf8_lossy(&output.stderr).trim()
                    );
                }
                Err(e) => {
                    tracing::warn!("Failed to run ioreg: {e}");
                }
            }
        }
    }
}
