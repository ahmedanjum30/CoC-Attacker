//! Game state machine: moves between screens and runs attacks.

use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use device::Device;
use image::{DynamicImage, RgbImage};
use serde::Deserialize;
use vision::deploy::find_deploy_zone;
use vision::hud;
use vision::ocr::Ocr;
use vision::{Button, Screen, Vision};

use crate::strategy::{AttackRecord, Learner, Strategy};

const SINGLE_PLAYER_TAB: (u32, u32) = (400, 56);
/// Rays cast from the base centre when looking for deploy points.
const DEPLOY_RAYS: usize = 32;
/// Used when no deploy border can be found (e.g. single-player maps whose
/// border runs off screen). Taps inside a no-deploy zone are ignored by the game.
const FALLBACK_POINTS: [(u32, u32); 5] = [(120, 250), (120, 280), (120, 310), (640, 60), (1150, 300)];

const POLL: Duration = Duration::from_millis(500);
/// Extra frames to check for the deploy border before using fallback points.
const BORDER_RETRIES: usize = 4;
/// A battle lasts at most 3 minutes, plus time for the results animation.
const BATTLE_TIMEOUT: Duration = Duration::from_secs(200);

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    /// Attack a base only if gold + elixir available is at least this.
    pub min_loot: u32,
    /// Give up and return home after this many bases.
    pub max_searches: u32,
    /// Share of attacks that try a random deploy strategy instead of the best one.
    pub explore: f64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self { min_loot: 2000, max_searches: 20, explore: 0.2 }
    }
}

pub struct Bot {
    device: Device,
    vision: Vision,
    ocr: Ocr,
}

impl Bot {
    pub fn new(device: Device, vision: Vision, ocr: Ocr) -> Self {
        Self { device, vision, ocr }
    }

    /// Searches for a base worth attacking, attacks it and records the result.
    /// Returns false if no suitable base was found.
    pub fn multiplayer_attack(&self, search: &SearchConfig, learner: &mut Learner) -> Result<bool> {
        self.go_home()?;
        let frame = self.frame()?;
        self.tap_button(&frame, Button::HomeBattle)?;
        let frame = self.wait_for(Screen::BattleMenu, Duration::from_secs(10))?;
        self.tap_button(&frame, Button::FindMatch)?;

        // Find a Match opens the army screen first; its Attack! button starts the search.
        let start = Instant::now();
        loop {
            let frame = self.frame()?;
            match self.vision.screen(&frame) {
                Screen::ArmyWindow => {
                    self.tap_button(&frame, Button::ArmyAttack)?;
                    break;
                }
                Screen::Scouting => break,
                _ if start.elapsed() > Duration::from_secs(10) => {
                    bail!("Find a Match did not open the army screen or a base")
                }
                _ => sleep(POLL),
            }
        }

        let mut searches = 0;
        let (frame, loot) = loop {
            // Searching shows clouds; the Next button appears once a base is ready.
            // If the 30 s scouting timer runs out the battle starts by itself and
            // Next disappears, so the only option left is to attack.
            let (screen, frame) =
                self.wait_for_any(&[Screen::Scouting, Screen::Battle], Duration::from_secs(60))?;
            searches += 1;
            let loot = hud::enemy_loot(&self.ocr, &frame)?;
            tracing::info!(
                "base {searches}: {} gold, {} elixir",
                loot.gold,
                loot.elixir
            );
            if screen == Screen::Battle {
                tracing::warn!("scouting time ran out, attacking this base");
                break (frame, loot);
            }
            if loot.total() >= search.min_loot {
                break (frame, loot);
            }
            if searches >= search.max_searches {
                tracing::info!("no base worth attacking after {searches} searches");
                self.tap_button(&frame, Button::EndBattle)?;
                self.go_home()?;
                return Ok(false);
            }
            self.tap_button(&frame, Button::Next)?;
            // Let the old base disappear before looking for the next one.
            sleep(Duration::from_millis(1500));
        };

        let strategy = learner.choose();
        tracing::info!("attacking with strategy {strategy:?}");
        let planned = self.deploy(&frame, strategy)?;

        let damage = self.finish_battle()?;
        // A fallback deploy says nothing about the strategy, so don't learn from it.
        if planned {
            learner.record(AttackRecord {
                unix_time: unix_now()?,
                strategy,
                gold: loot.gold,
                elixir: loot.elixir,
                damage,
            })?;
        }
        Ok(true)
    }

    /// Plays one single-player (Goblin map) attack from start to finish.
    pub fn goblin_attack(&self) -> Result<()> {
        self.go_home()?;

        let frame = self.frame()?;
        self.tap_button(&frame, Button::HomeBattle)?;
        self.wait_for(Screen::BattleMenu, Duration::from_secs(10))?;

        self.device.tap(SINGLE_PLAYER_TAB.0, SINGLE_PLAYER_TAB.1)?;
        let attack = self.wait_for_button(Button::SinglePlayerAttack, Duration::from_secs(10))?;
        tracing::info!("starting single-player attack");
        self.device.tap(attack.0, attack.1)?;
        let frame = self.wait_for(Screen::Battle, Duration::from_secs(20))?;

        self.deploy(&frame, Strategy::Surround)?;
        self.finish_battle()?;
        Ok(())
    }

    /// Gets back to the home screen from wherever the game is.
    pub fn go_home(&self) -> Result<()> {
        let start = Instant::now();
        // Generous: the game reloads (black screen, then loading) after idling.
        while start.elapsed() < Duration::from_secs(180) {
            let frame = self.frame()?;
            match self.vision.screen(&frame) {
                Screen::Home => return Ok(()),
                Screen::BattleMenu => self.device.back()?,
                Screen::Results => self.tap_button(&frame, Button::ReturnHome)?,
                // Never abandon a battle on our own; it ends by itself.
                Screen::Battle => tracing::info!("battle in progress, waiting"),
                Screen::ArmyWindow => self.tap_button(&frame, Button::CloseWindow)?,
                Screen::Scouting => {
                    tracing::info!("leaving the enemy base without attacking");
                    self.tap_button(&frame, Button::EndBattle)?
                }
                Screen::Unknown => {}
            }
            sleep(Duration::from_secs(2));
        }
        bail!("could not reach the home screen");
    }

    /// Deploys every troop in the bar across the strategy's points. Returns
    /// false if the border wasn't found and fallback points were used instead.
    fn deploy(&self, frame: &RgbImage, strategy: Strategy) -> Result<bool> {
        // The border can lag behind the clouds clearing, so give it a moment.
        let mut zone = find_deploy_zone(frame, DEPLOY_RAYS);
        let mut last = None;
        for _ in 0..BORDER_RETRIES {
            if zone.is_some() {
                break;
            }
            sleep(POLL);
            let f = self.frame()?;
            zone = find_deploy_zone(&f, DEPLOY_RAYS);
            last = Some(f);
        }
        let points = match &zone {
            Some(zone) => {
                tracing::debug!("deploy zone: centre {:?}, {} points", zone.center, zone.points.len());
                strategy.points(zone)
            }
            None => {
                let path = format!("captures/no_border_{}.png", unix_now()?);
                tracing::warn!("no deploy border found, using fallback points (frame saved to {path})");
                std::fs::create_dir_all("captures")?;
                last.as_ref().unwrap_or(frame).save(&path)?;
                FALLBACK_POINTS.to_vec()
            }
        };

        let slots = hud::troop_slots(&self.ocr, frame)?;
        if slots.is_empty() {
            bail!("no troops in the battle bar");
        }
        for slot in slots {
            tracing::info!("deploying {} troops from slot {:?} over {} points", slot.count, slot.pos, points.len());
            self.device.tap(slot.pos.0, slot.pos.1)?;
            for i in 0..slot.count as usize {
                let (x, y) = points[i % points.len()];
                self.device.tap(x, y)?;
            }
        }
        Ok(zone.is_some())
    }

    /// Waits for the results, reads the damage and returns home.
    fn finish_battle(&self) -> Result<u32> {
        tracing::info!("waiting for the battle to finish");
        let mut frame = self.wait_for(Screen::Results, BATTLE_TIMEOUT)?;
        // The percentage counts up during the results animation.
        sleep(Duration::from_secs(2));
        if let Ok(f) = self.frame() {
            frame = f;
        }
        let damage = hud::damage_percent(&self.ocr, &frame)?.unwrap_or_else(|| {
            tracing::warn!("could not read the damage percentage");
            0
        });
        tracing::info!("battle over: {damage}% damage");
        self.tap_button(&frame, Button::ReturnHome)?;
        self.wait_for(Screen::Home, Duration::from_secs(30))?;
        Ok(damage)
    }

    fn frame(&self) -> Result<RgbImage> {
        Ok(DynamicImage::from(self.device.screenshot()?).to_rgb8())
    }

    fn tap_button(&self, frame: &RgbImage, button: Button) -> Result<()> {
        match self.vision.find(frame, button) {
            Some((x, y)) => self.device.tap(x, y),
            None => bail!("{button:?} is not on screen"),
        }
    }

    /// Polls until `want` is showing and returns that frame.
    fn wait_for(&self, want: Screen, timeout: Duration) -> Result<RgbImage> {
        Ok(self.wait_for_any(&[want], timeout)?.1)
    }

    /// Polls until any of `wanted` is showing and returns which one, with its frame.
    fn wait_for_any(&self, wanted: &[Screen], timeout: Duration) -> Result<(Screen, RgbImage)> {
        let start = Instant::now();
        loop {
            let frame = self.frame()?;
            let screen = self.vision.screen(&frame);
            if wanted.contains(&screen) {
                tracing::debug!("reached {screen:?} after {:?}", start.elapsed());
                return Ok((screen, frame));
            }
            if start.elapsed() > timeout {
                bail!("timed out waiting for {wanted:?} (stuck on {screen:?})");
            }
            sleep(POLL);
        }
    }

    fn wait_for_button(&self, button: Button, timeout: Duration) -> Result<(u32, u32)> {
        let start = Instant::now();
        loop {
            if let Some(pos) = self.vision.find(&self.frame()?, button) {
                return Ok(pos);
            }
            if start.elapsed() > timeout {
                bail!("timed out waiting for {button:?}");
            }
            sleep(POLL);
        }
    }
}

fn unix_now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}
