//! GPU buffers placed in system RAM. Only amdgpu reports this (GTT usage) through sysfs.

use std::fs;

use crate::units::Bytes;

#[derive(Debug, Clone)]
pub struct GpuGtt {
    pub used: Bytes,
}

pub fn read() -> Vec<GpuGtt> {
    fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            // `card0` is the device; `card0-DP-1` and friends are its connectors.
            let name = entry.file_name();
            let number = name.to_str()?.strip_prefix("card")?;
            number.chars().all(|c| c.is_ascii_digit()).then_some(())?;
            let used = fs::read_to_string(entry.path().join("device/mem_info_gtt_used")).ok()?;
            Some(GpuGtt { used: Bytes(used.trim().parse().ok()?) })
        })
        .collect()
}
