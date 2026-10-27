//! Game state machine: moves between screens and runs attacks.

use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use device::Device;
use image::{DynamicImage, RgbImage};
use vision::{Button, Screen, Vision};

const SINGLE_PLAYER_TAB: (u32, u32) = (400, 56);
const FIRST_TROOP_SLOT: (u32, u32) = (153, 650);
/// Placeholder drop points around the map edge, until deploy planning exists.
/// Points that land inside a base's no-deploy zone are simply ignored by the game.
const EDGE_POINTS: [(u32, u32); 5] = [(120, 250), (120, 280), (120, 310), (640, 60), (1150, 300)];

const POLL: Duration = Duration::from_millis(500);
/// A battle lasts at most 3 minutes, plus time for the results animation.
const BATTLE_TIMEOUT: Duration = Duration::from_secs(200);

pub struct Bot {
    device: Device,
    vision: Vision,
}

impl Bot {
    pub fn new(device: Device, vision: Vision) -> Self {
        Self { device, vision }
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
        self.wait_for(Screen::Battle, Duration::from_secs(20))?;

        self.deploy()?;

        tracing::info!("waiting for the battle to finish");
        let frame = self.wait_for(Screen::Results, BATTLE_TIMEOUT)?;
        self.tap_button(&frame, Button::ReturnHome)?;
        self.wait_for(Screen::Home, Duration::from_secs(30))?;
        tracing::info!("attack complete");
        Ok(())
    }

    /// Gets back to the home screen from wherever the game is.
    pub fn go_home(&self) -> Result<()> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(60) {
            let frame = self.frame()?;
            match self.vision.screen(&frame) {
                Screen::Home => return Ok(()),
                Screen::BattleMenu => self.device.back()?,
                Screen::Results => self.tap_button(&frame, Button::ReturnHome)?,
                // Never abandon a battle on our own; it ends by itself.
                Screen::Battle => tracing::info!("battle in progress, waiting"),
                Screen::Unknown => {}
            }
            sleep(Duration::from_secs(2));
        }
        bail!("could not reach the home screen");
    }

    /// Placeholder: drops the first troop type along the map edges.
    fn deploy(&self) -> Result<()> {
        tracing::info!("deploying troops");
        self.device.tap(FIRST_TROOP_SLOT.0, FIRST_TROOP_SLOT.1)?;
        for i in 0..30 {
            let (x, y) = EDGE_POINTS[i % EDGE_POINTS.len()];
            self.device.tap(x, y)?;
        }
        Ok(())
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
        let start = Instant::now();
        loop {
            let frame = self.frame()?;
            let screen = self.vision.screen(&frame);
            if screen == want {
                tracing::debug!("reached {want:?} after {:?}", start.elapsed());
                return Ok(frame);
            }
            if start.elapsed() > timeout {
                bail!("timed out waiting for {want:?} (stuck on {screen:?})");
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
