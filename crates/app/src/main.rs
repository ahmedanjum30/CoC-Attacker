mod army;
mod bot;
mod strategy;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use device::{Device, DeviceConfig};
use serde::Deserialize;
use army::Catalog;
use bot::SearchConfig;
use strategy::Learner;
use vision::ocr::{Ocr, TextColor};
use vision::{Button, Vision};

const ASSETS_DIR: &str = "assets";
const TEMPLATES_DIR: &str = "assets/templates";
const MODELS_DIR: &str = "models";
const ATTACK_LOG: &str = "data/attacks.jsonl";

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
    /// Pick the best army for the unlocked troops and set it in the army editor.
    Army,
    /// Search for bases and attack them, learning which army and deploy strategy work best.
    Attack {
        #[arg(long, default_value_t = 1)]
        count: u32,
    },
    /// Show how each deploy strategy has performed. Needs no device.
    Stats,
    /// Draw the deploy zone found in a saved image. Needs no device.
    Plan {
        input: PathBuf,
        #[arg(long, default_value = "captures/plan.png")]
        out: PathBuf,
    },
    /// Read text from a region of a saved image. Needs no device.
    Ocr {
        input: PathBuf,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        #[arg(long, value_enum, default_value_t = OcrColor::White)]
        color: OcrColor,
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

#[derive(Clone, Copy, clap::ValueEnum)]
enum OcrColor {
    White,
    Green,
}

#[derive(Deserialize)]
struct Config {
    device: DeviceConfig,
    #[serde(default)]
    search: SearchConfig,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    if let Cmd::Stats = &cli.command {
        print_stats(&Learner::load(PathBuf::from(ATTACK_LOG), 0.0)?);
        return Ok(());
    }
    if let Cmd::Crop { input, x, y, w, h, out } = &cli.command {
        let img = image::open(input)?.crop_imm(*x, *y, *w, *h);
        img.save(out)?;
        return Ok(());
    }
    if let Cmd::Ocr { input, x, y, w, h, color } = &cli.command {
        let ocr = vision::ocr::Ocr::load(Path::new(MODELS_DIR))?;
        let frame = image::open(input)?.to_rgb8();
        let start = Instant::now();
        let area = imageproc::rect::Rect::at(*x, *y).of_size(*w, *h);
        let color = match color {
            OcrColor::White => TextColor::White,
            OcrColor::Green => TextColor::Green,
        };
        let text = ocr.read(&frame, area, color)?;
        tracing::info!("{text:?} (took {:?})", start.elapsed());
        return Ok(());
    }
    if let Cmd::Plan { input, out } = &cli.command {
        let mut frame = image::open(input)?.to_rgb8();
        let start = Instant::now();
        match vision::deploy::find_deploy_zone(&frame, 32) {
            Some(zone) => {
                tracing::info!(
                    "centre {:?}, {} points (took {:?})",
                    zone.center,
                    zone.points.len(),
                    start.elapsed()
                );
                let c = (zone.center.0 as i32, zone.center.1 as i32);
                imageproc::drawing::draw_filled_circle_mut(&mut frame, c, 6, image::Rgb([0, 0, 255]));
                for p in &zone.points {
                    let pos = (p.pos.0 as i32, p.pos.1 as i32);
                    imageproc::drawing::draw_filled_circle_mut(&mut frame, pos, 5, image::Rgb([0, 255, 255]));
                }
                frame.save(out)?;
            }
            None => tracing::info!("no deploy border visible"),
        }
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
        Cmd::Home => make_bot(device)?.go_home()?,
        Cmd::Goblin { count } => {
            let bot = make_bot(device)?;
            for i in 1..=count {
                tracing::info!("goblin attack {i}/{count}");
                bot.goblin_attack()?;
            }
        }
        Cmd::Attack { count } => {
            let bot = make_bot(device)?;
            let mut learner = Learner::load(PathBuf::from(ATTACK_LOG), config.search.explore)?;
            for i in 1..=count {
                tracing::info!("multiplayer attack {i}/{count}");
                let (name, army) = bot.prepare_army(&learner)?;
                if !bot.multiplayer_attack(&config.search, &mut learner, &name, &army)? {
                    break;
                }
            }
            print_stats(&learner);
        }
        Cmd::Army => {
            let learner = Learner::load(PathBuf::from(ATTACK_LOG), config.search.explore)?;
            make_bot(device)?.prepare_army(&learner)?;
        }
        Cmd::Stats | Cmd::Crop { .. } | Cmd::Ocr { .. } | Cmd::Plan { .. } => {
            unreachable!("handled before connecting")
        }
    }
    Ok(())
}

fn make_bot(device: Device) -> Result<bot::Bot> {
    Ok(bot::Bot::new(
        device,
        Vision::load(Path::new(TEMPLATES_DIR))?,
        Ocr::load(Path::new(MODELS_DIR))?,
        Catalog::load(Path::new(ASSETS_DIR))?,
    ))
}

fn print_stats(learner: &Learner) {
    tracing::info!("by army:");
    for (army, mean, n) in learner.army_stats() {
        tracing::info!("  {army:<16} {n:>3} attacks, mean damage {mean:.1}%");
    }
    tracing::info!("by deploy strategy:");
    for (strategy, mean, n) in learner.strategy_stats() {
        tracing::info!("  {:<16} {n:>3} attacks, mean damage {mean:.1}%", format!("{strategy:?}"));
    }
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
