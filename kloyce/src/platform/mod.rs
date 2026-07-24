use chrono::{DateTime, Utc};

#[derive(Debug, Clone, serde::Serialize)]
pub struct GpuMetrics {
    pub gpu_name: String,
    pub utilization_pct: u32,
    pub vram_used_mb: u64,
    pub vram_total_mb: u64,
    pub temperature_c: u32,
    pub power_draw_w: f64,
    pub power_limit_w: f64,
    pub fan_speed_pct: u32,
    pub timestamp: DateTime<Utc>,
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;
