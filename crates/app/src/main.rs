mod bot;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use device::{Device, DeviceConfig};
use serde::Deserialize;
use vision::{Button, Vision};

const TEMPLATES_DIR: &str = "assets/templates";

#[derive(Parser)]
#[command(about = "Clash of Clans attack bot")]
struct Cli {
    /// Path to the config file.
    #[arg(long, default_value = "config.toml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Check the connection and print the screen resolution.
    Info,
    /// Save a screenshot to disk.
    Screenshot {
        #[arg(long, default_value = "captures/screen.png")]
        out: PathBuf,
    },
    /// Tap a point on the screen.
    Tap { x: u32, y: u32 },
    /// Swipe between two points.
    Swipe {
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        #[arg(long, default_value_t = 300)]
        ms: u64,
    },
    /// Identify the current screen and show every button's match score.
    Detect {
        /// Use a saved image instead of the live device.
        #[arg(long)]
        image: Option<PathBuf>,
    },
    /// Navigate back to the home screen from anywhere.
    Home,
    /// Play single-player Goblin map attacks.
    Goblin {
        #[arg(long, default_value_t = 1)]
        count: u32,
    },
    /// Cut a region out of an image (for making templates). Needs no device.
    Crop {
        input: PathBuf,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        out: PathBuf,
    },
}

#[derive(Deserialize)]
struct Config {
    device: DeviceConfig,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    if let Cmd::Crop { input, x, y, w, h, out } = &cli.command {
        let img = image::open(input)?.crop_imm(*x, *y, *w, *h);
        img.save(out)?;
        return Ok(());
    }
    if let Cmd::Detect { image: Some(path) } = &cli.command {
        detect(&image::open(path)?.to_rgb8())?;
        return Ok(());
    }

    let text = std::fs::read_to_string(&cli.config).with_context(|| {
        format!(
            "could not read {} (copy config.example.toml to get started)",
            cli.config.display()
        )
    })?;
    let config: Config = toml::from_str(&text).context("invalid config")?;
    let device = Device::connect(config.device)?;

    match cli.command {
        Cmd::Info => {
            let (w, h) = device.screen_size()?;
            tracing::info!("connected to {} ({w}x{h})", device.serial());
        }
        Cmd::Screenshot { out } => {
            let start = Instant::now();
            let img = device.screenshot()?;
            let elapsed = start.elapsed();
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            img.save(&out)?;
            tracing::info!(
                "saved {}x{} frame to {} (capture took {elapsed:?})",
                img.width(),
                img.height(),
                out.display()
            );
        }
        Cmd::Tap { x, y } => device.tap(x, y)?,
        Cmd::Swipe { x1, y1, x2, y2, ms } => {
            device.swipe((x1, y1), (x2, y2), Duration::from_millis(ms))?
        }
        Cmd::Detect { .. } => detect(&image::DynamicImage::from(device.screenshot()?).to_rgb8())?,
        Cmd::Home => bot::Bot::new(device, Vision::load(Path::new(TEMPLATES_DIR))?).go_home()?,
        Cmd::Goblin { count } => {
            let bot = bot::Bot::new(device, Vision::load(Path::new(TEMPLATES_DIR))?);
            for i in 1..=count {
                tracing::info!("goblin attack {i}/{count}");
                bot.goblin_attack()?;
            }
        }
        Cmd::Crop { .. } => unreachable!("handled before connecting"),
    }
    Ok(())
}

fn detect(frame: &image::RgbImage) -> Result<()> {
    let vision = Vision::load(Path::new(TEMPLATES_DIR))?;
    let start = Instant::now();
    let screen = vision.screen(frame);
    tracing::info!("screen: {screen:?} (classified in {:?})", start.elapsed());
    for button in Button::ALL {
        let m = vision.locate(frame, button);
        tracing::info!(
            "  {:<20} error {:.4} at {:?}{}",
            format!("{button:?}"),
            m.error,
            m.center,
            if m.found() { "  <- found" } else { "" }
        );
    }
    Ok(())
}
