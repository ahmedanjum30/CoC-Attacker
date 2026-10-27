//! Game state machine: moves between screens and runs attacks.

use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow, bail};
use device::Device;
use image::{DynamicImage, RgbImage};
use serde::Deserialize;
use vision::deploy::find_deploy_zone;
use vision::hud;
use vision::ocr::Ocr;
use vision::{Button, Screen, Vision};

use crate::army::{ArmyEntry, Catalog, Role};
use crate::strategy::{AttackRecord, Learner, Strategy};

const SINGLE_PLAYER_TAB: (u32, u32) = (400, 56);
/// Rays cast from the base centre when looking for deploy points.
const DEPLOY_RAYS: usize = 32;
/// Used when no deploy border can be found (e.g. single-player maps whose
/// border runs off screen). Taps inside a no-deploy zone are ignored by the game.
const FALLBACK_POINTS: [(u32, u32); 5] = [(120, 250), (120, 280), (120, 310), (640, 60), (1150, 300)];

/// Fixed spots in the battle menu and army editor.
const EDIT_ARMY: (u32, u32) = (150, 680);
const TROOP_ROW: (u32, u32) = (800, 240);
const CLEAR_ARMY: (u32, u32) = (1237, 166);
/// Dimmed area above the troop drawer; tapping it closes the drawer.
const CLOSE_DRAWER: (u32, u32) = (640, 110);
/// How many of the strongest buildable armies the learner chooses between.
const ARMY_CANDIDATES: usize = 3;

/// Pause between deploy waves so earlier troops engage first.
const WAVE_GAP: Duration = Duration::from_millis(1500);
/// Share of each damage-dealing troop held back for the second push.
const RESERVE_PERCENT: u32 = 25;
const MONITOR_INTERVAL: Duration = Duration::from_secs(3);
/// No damage gained for this long counts as a stalled attack.
const STALL_TIME: Duration = Duration::from_secs(9);

/// Where scouted bases are saved for labelling.
const DATASET_DIR: &str = "dataset/raw";

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
    /// Save every scouted base to dataset/raw/ for training building detection.
    pub save_bases: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self { min_loot: 2000, max_searches: 20, explore: 0.2, save_bases: true }
    }
}

/// What happened at deploy time, needed to run the rest of the battle.
struct Deployment {
    /// False if the border wasn't found and fallback points were used.
    planned: bool,
    /// Troops held back: (slot tap point, count).
    reserve: Vec<((u32, u32), u32)>,
    reserve_points: Vec<(u32, u32)>,
}

pub struct Bot {
    device: Device,
    vision: Vision,
    ocr: Ocr,
    catalog: Catalog,
}

impl Bot {
    pub fn new(device: Device, vision: Vision, ocr: Ocr, catalog: Catalog) -> Self {
        Self { device, vision, ocr, catalog }
    }

    /// Searches for a base worth attacking, attacks it and records the result.
    /// Returns false if no suitable base was found.
    pub fn multiplayer_attack(
        &self,
        search: &SearchConfig,
        learner: &mut Learner,
        army_name: &str,
        army: &[ArmyEntry],
    ) -> Result<bool> {
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
            if search.save_bases {
                std::fs::create_dir_all(DATASET_DIR)?;
                frame.save(format!("{DATASET_DIR}/{}_{searches}.png", unix_now()?))?;
            }
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

        let strategy = learner.choose_strategy();
        tracing::info!("attacking with {army_name} using strategy {strategy:?}");
        let deployment = self.deploy(&frame, strategy, Some(army))?;
        let planned = deployment.planned;
        self.manage_battle(deployment)?;

        let damage = self.finish_battle()?;
        // A fallback deploy says nothing about the strategy, so don't learn from it.
        if planned {
            learner.record(AttackRecord {
                unix_time: unix_now()?,
                army: army_name.to_string(),
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

        let deployment = self.deploy(&frame, Strategy::Surround, None)?;
        self.manage_battle(deployment)?;
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
                Screen::ArmyWindow | Screen::Window => self.tap_button(&frame, Button::CloseWindow)?,
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

    /// Reads the unlocked troops, picks an army (strongest few the account can
    /// build, chosen by past results) and sets it in the army editor.
    pub fn prepare_army(&self, learner: &Learner) -> Result<(String, Vec<ArmyEntry>)> {
        self.go_home()?;
        let frame = self.frame()?;
        self.tap_button(&frame, Button::HomeBattle)?;
        self.wait_for(Screen::BattleMenu, Duration::from_secs(10))?;
        self.device.tap(EDIT_ARMY.0, EDIT_ARMY.1)?;
        let frame = self.wait_for(Screen::ArmyWindow, Duration::from_secs(10))?;
        let (used, capacity) = hud::army_capacity(&self.ocr, &frame)?
            .ok_or_else(|| anyhow!("could not read the army camp capacity"))?;

        self.device.tap(TROOP_ROW.0, TROOP_ROW.1)?;
        sleep(Duration::from_millis(1500));
        let cards = hud::editor_cards(&self.ocr, &self.frame()?)?;
        let unlocked: Vec<usize> = cards.iter().filter(|c| c.unlocked).map(|c| c.index).collect();
        for c in cards.iter().filter(|c| c.unlocked) {
            let name = self.catalog.troop_at(c.index).map_or("?", |t| t.name.as_str());
            let level = c.level.map_or("?".into(), |l| l.to_string());
            tracing::info!("unlocked: {name} (level {level})");
        }

        let candidates: Vec<String> = self
            .catalog
            .available(&unlocked)
            .iter()
            .take(ARMY_CANDIDATES)
            .map(|a| a.name.clone())
            .collect();
        if candidates.is_empty() {
            bail!("no army in armies.toml can be built from the unlocked troops");
        }
        let name = learner.choose_army(&candidates);
        let army = self.catalog.armies.iter().find(|a| a.name == name).unwrap();
        let plan = self.catalog.plan(army, capacity);
        let summary: Vec<String> = plan.iter().map(|e| format!("{} x{}", e.name, e.count)).collect();
        tracing::info!("army {name} for {capacity} space: {}", summary.join(", "));

        // With a single troop type unlocked, a full camp already holds the plan.
        if !(unlocked.len() == 1 && used == capacity) {
            if used > 0 {
                // The trash button only works with the troop drawer closed,
                // and asks for confirmation.
                self.device.tap(CLOSE_DRAWER.0, CLOSE_DRAWER.1)?;
                sleep(Duration::from_millis(800));
                self.device.tap(CLEAR_ARMY.0, CLEAR_ARMY.1)?;
                let okay = self.wait_for_button(Button::Okay, Duration::from_secs(5))?;
                self.device.tap(okay.0, okay.1)?;
                sleep(Duration::from_millis(800));
                self.device.tap(TROOP_ROW.0, TROOP_ROW.1)?;
                sleep(Duration::from_millis(1500));
            }
            for entry in &plan {
                let pos = cards[entry.card].pos;
                for _ in 0..entry.count {
                    self.device.tap(pos.0, pos.1)?;
                }
            }
            sleep(Duration::from_millis(800));
            let filled = hud::army_capacity(&self.ocr, &self.frame()?)?;
            if filled != Some((capacity, capacity)) {
                tracing::warn!("army camp shows {filled:?} after filling, expected {capacity}/{capacity}");
            }
        }
        // With the troop drawer open the window isn't recognisable, so close it first.
        self.device.tap(CLOSE_DRAWER.0, CLOSE_DRAWER.1)?;
        sleep(Duration::from_millis(800));
        self.go_home()?;
        Ok((name, plan))
    }

    /// Deploys the army in waves by role (tanks first, support last) over the
    /// strategy's points, holding back part of the damage dealers. Returns
    /// whether the deploy border was found, and the plan for the reserve.
    fn deploy(&self, frame: &RgbImage, strategy: Strategy, army: Option<&[ArmyEntry]>) -> Result<Deployment> {
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
        let (points, reserve_points) = match &zone {
            Some(zone) => {
                tracing::debug!("deploy zone: centre {:?}, {} points", zone.center, zone.points.len());
                (strategy.points(zone), strategy.reserve_points(zone))
            }
            None => {
                let path = format!("captures/no_border_{}.png", unix_now()?);
                tracing::warn!("no deploy border found, using fallback points (frame saved to {path})");
                std::fs::create_dir_all("captures")?;
                last.as_ref().unwrap_or(frame).save(&path)?;
                (FALLBACK_POINTS.to_vec(), FALLBACK_POINTS.to_vec())
            }
        };

        let mut slots = hud::troop_slots(&self.ocr, frame)?;
        if slots.is_empty() {
            bail!("no troops in the battle bar");
        }
        // The battle bar lists troops in the same order as the army editor,
        // so the planned army tells us each slot's role. Its counts are also
        // more reliable than OCR of the small "x15" labels.
        let roles: Vec<Role> = match army {
            Some(a) if a.len() == slots.len() => {
                for (slot, entry) in slots.iter_mut().zip(a) {
                    slot.count = slot.count.min(entry.count);
                }
                a.iter().map(|e| e.role).collect()
            }
            _ => vec![Role::Damage; slots.len()],
        };
        let mut order: Vec<usize> = (0..slots.len()).collect();
        order.sort_by_key(|&i| roles[i].wave());

        // Tanks go to the middle of the chosen side so they draw fire first.
        let middle = &points[points.len() / 3..(points.len() * 2 / 3).max(points.len() / 3 + 1)];
        let mut reserve = Vec::new();
        let mut last_wave = None;
        for i in order {
            let (slot, role) = (slots[i], roles[i]);
            if last_wave.is_some_and(|w| w != role.wave()) {
                // Give the previous wave a moment to engage.
                sleep(WAVE_GAP);
            }
            last_wave = Some(role.wave());

            let held = if role == Role::Damage && strategy != Strategy::Surround {
                slot.count * RESERVE_PERCENT / 100
            } else {
                0
            };
            let targets = if role == Role::Tank { middle } else { &points[..] };
            tracing::info!(
                "wave {}: {} x{role:?} over {} points{}",
                role.wave(),
                slot.count - held,
                targets.len(),
                if held > 0 { format!(", {held} held back") } else { String::new() }
            );
            self.drop_troops(slot.pos, slot.count - held, targets)?;
            if held > 0 {
                reserve.push((slot.pos, held));
            }
        }
        Ok(Deployment { planned: zone.is_some(), reserve, reserve_points })
    }

    /// Watches the live damage and sends the reserve to another side once the
    /// attack stalls, then waits for the battle to end.
    fn manage_battle(&self, mut deployment: Deployment) -> Result<()> {
        let start = Instant::now();
        let (mut best, mut last_gain) = (0, Instant::now());
        while start.elapsed() < BATTLE_TIMEOUT {
            sleep(MONITOR_INTERVAL);
            let frame = self.frame()?;
            if self.vision.screen(&frame) != Screen::Battle {
                return Ok(());
            }
            if deployment.reserve.is_empty() {
                continue;
            }
            if let Some(d) = hud::live_damage(&self.ocr, &frame)?
                && d > best
            {
                (best, last_gain) = (d, Instant::now());
            }
            if last_gain.elapsed() >= STALL_TIME {
                tracing::info!("attack stalled at {best}%, sending the reserve to the next side");
                for (pos, count) in std::mem::take(&mut deployment.reserve) {
                    self.drop_troops(pos, count, &deployment.reserve_points)?;
                }
            }
        }
        Ok(())
    }

    /// Selects a troop slot and drops `count` troops spread over `points`.
    fn drop_troops(&self, slot: (u32, u32), count: u32, points: &[(u32, u32)]) -> Result<()> {
        if count == 0 || points.is_empty() {
            return Ok(());
        }
        self.device.tap(slot.0, slot.1)?;
        for i in 0..count as usize {
            let (x, y) = points[i % points.len()];
            self.device.tap(x, y)?;
        }
        Ok(())
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
