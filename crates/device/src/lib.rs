//! Talks to the Android emulator over ADB: screenshots and touch input.

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use image::RgbaImage;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceConfig {
    /// Path to adb.exe. LDPlayer ships one in its install folder.
    #[serde(default = "default_adb_path")]
    pub adb_path: PathBuf,
    /// Device serial, e.g. "emulator-5554" or "127.0.0.1:5555".
    pub serial: String,
}

fn default_adb_path() -> PathBuf {
    PathBuf::from("adb")
}

pub struct Device {
    cfg: DeviceConfig,
}

impl Device {
    /// Connects to the configured device and checks that it is online.
    pub fn connect(cfg: DeviceConfig) -> Result<Self> {
        let dev = Self { cfg };

        // TCP serials must be connected explicitly before use.
        if dev.cfg.serial.contains(':') {
            let out = dev.run(&["connect", &dev.cfg.serial])?;
            tracing::debug!("adb connect: {}", String::from_utf8_lossy(&out.stdout).trim());
        }

        let out = dev.adb(&["get-state"])?;
        let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if state != "device" {
            bail!("device {} is in state '{state}', expected 'device'", dev.cfg.serial);
        }
        Ok(dev)
    }

    pub fn serial(&self) -> &str {
        &self.cfg.serial
    }

    /// Current screen resolution, honouring any `wm size` override.
    pub fn screen_size(&self) -> Result<(u32, u32)> {
        let out = self.adb(&["shell", "wm", "size"])?;
        let text = String::from_utf8_lossy(&out.stdout);
        // Output is "Physical size: WxH", optionally followed by "Override size: WxH".
        let size = text
            .lines()
            .rev()
            .find_map(|l| l.split(':').nth(1))
            .context("could not parse `wm size` output")?;
        let (w, h) = size.trim().split_once('x').context("bad size format")?;
        Ok((w.parse()?, h.parse()?))
    }

    /// Grabs the current frame as raw RGBA (skips PNG encoding on the device).
    pub fn screenshot(&self) -> Result<RgbaImage> {
        let out = self.adb(&["exec-out", "screencap"])?;
        decode_raw_screencap(&out.stdout)
    }

    pub fn tap(&self, x: u32, y: u32) -> Result<()> {
        self.adb(&["shell", "input", "tap", &x.to_string(), &y.to_string()])?;
        Ok(())
    }

    pub fn swipe(&self, from: (u32, u32), to: (u32, u32), duration: Duration) -> Result<()> {
        self.adb(&[
            "shell",
            "input",
            "swipe",
            &from.0.to_string(),
            &from.1.to_string(),
            &to.0.to_string(),
            &to.1.to_string(),
            &duration.as_millis().to_string(),
        ])?;
        Ok(())
    }

    /// Presses the Android back button (closes menus and dialogs).
    pub fn back(&self) -> Result<()> {
        self.adb(&["shell", "input", "keyevent", "4"])?;
        Ok(())
    }

    /// Runs an adb command against this device.
    fn adb(&self, args: &[&str]) -> Result<Output> {
        let mut full = vec!["-s", self.cfg.serial.as_str()];
        full.extend_from_slice(args);
        self.run(&full)
    }

    /// Runs a raw adb command (no device selected).
    fn run(&self, args: &[&str]) -> Result<Output> {
        let out = Command::new(&self.cfg.adb_path)
            .args(args)
            .output()
            .with_context(|| format!("failed to launch adb at {}", self.cfg.adb_path.display()))?;
        if !out.status.success() {
            bail!(
                "adb {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(out)
    }
}

/// Decodes `screencap` raw output: a little-endian header (width, height, format,
/// plus a colour-space field on Android 9+) followed by RGBA_8888 pixels.
fn decode_raw_screencap(buf: &[u8]) -> Result<RgbaImage> {
    if buf.len() < 12 {
        bail!("screencap output too short ({} bytes)", buf.len());
    }
    let word = |i: usize| u32::from_le_bytes(buf[i * 4..i * 4 + 4].try_into().unwrap());
    let (width, height, format) = (word(0), word(1), word(2));

    const RGBA_8888: u32 = 1;
    if format != RGBA_8888 {
        bail!("unsupported pixel format {format}");
    }

    let pixels = width as usize * height as usize * 4;
    let header = buf
        .len()
        .checked_sub(pixels)
        .context("screencap output smaller than its declared size")?;
    if header != 12 && header != 16 {
        bail!("unexpected screencap header size {header}");
    }

    RgbaImage::from_raw(width, height, buf[header..].to_vec())
        .context("failed to build image from screencap data")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(width: u32, height: u32, header_extra: bool) -> Vec<u8> {
        let mut buf = Vec::new();
        for v in [width, height, 1] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        if header_extra {
            buf.extend_from_slice(&0u32.to_le_bytes());
        }
        buf.extend((0..width * height).flat_map(|i| [i as u8, 0, 0, 255]));
        buf
    }

    #[test]
    fn decodes_both_header_layouts() {
        for extra in [false, true] {
            let img = decode_raw_screencap(&raw(3, 2, extra)).unwrap();
            assert_eq!(img.dimensions(), (3, 2));
            assert_eq!(img.get_pixel(2, 1).0, [5, 0, 0, 255]);
        }
    }

    #[test]
    fn rejects_truncated_data() {
        let mut buf = raw(3, 2, true);
        buf.truncate(20);
        assert!(decode_raw_screencap(&buf).is_err());
    }
}
