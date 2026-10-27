//! Screen understanding: finds known UI buttons by template matching and
//! works out which game screen is showing.
//!
//! All coordinates assume a 1280x720 frame.

pub mod deploy;
pub mod hud;
pub mod ocr;

use std::path::Path;

use anyhow::{Context, Result};
use image::{GrayImage, ImageBuffer as Image, Luma, RgbImage};
use imageproc::rect::Rect;
use imageproc::template_matching::{MatchTemplateMethod, find_extremes, match_template_parallel};

/// Normalised squared error below which a template counts as found.
const MATCH_THRESHOLD: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    HomeBattle,
    MenuPracticeTab,
    FindMatch,
    SinglePlayerAttack,
    EndBattle,
    Surrender,
    Next,
    ReturnHome,
    ArmyAttack,
    CloseWindow,
}

impl Button {
    pub const ALL: [Button; 10] = [
        Button::HomeBattle,
        Button::MenuPracticeTab,
        Button::FindMatch,
        Button::SinglePlayerAttack,
        Button::EndBattle,
        Button::Surrender,
        Button::Next,
        Button::ReturnHome,
        Button::ArmyAttack,
        Button::CloseWindow,
    ];

    fn file_name(self) -> &'static str {
        match self {
            Button::HomeBattle => "home_battle.png",
            Button::MenuPracticeTab => "menu_practice_tab.png",
            Button::FindMatch => "find_match.png",
            Button::SinglePlayerAttack => "single_player_attack.png",
            Button::EndBattle => "end_battle.png",
            Button::Surrender => "surrender.png",
            Button::Next => "next.png",
            Button::ReturnHome => "return_home.png",
            Button::ArmyAttack => "army_attack.png",
            Button::CloseWindow => "close_x.png",
        }
    }

    /// Area of the frame to search. Keeping it tight makes matching fast and
    /// avoids false hits elsewhere on screen.
    fn search_area(self) -> Rect {
        match self {
            Button::HomeBattle => Rect::at(0, 560).of_size(180, 160),
            Button::MenuPracticeTab => Rect::at(580, 20).of_size(200, 80),
            Button::FindMatch => Rect::at(100, 450).of_size(400, 140),
            // The Attack button follows the next unbeaten map, so search the whole map panel.
            Button::SinglePlayerAttack => Rect::at(40, 140).of_size(720, 480),
            Button::EndBattle | Button::Surrender => Rect::at(0, 480).of_size(220, 110),
            Button::Next => Rect::at(1040, 450).of_size(240, 120),
            Button::ReturnHome => Rect::at(480, 540).of_size(320, 160),
            Button::ArmyAttack => Rect::at(980, 590).of_size(290, 100),
            Button::CloseWindow => Rect::at(1170, 20).of_size(110, 90),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Home,
    BattleMenu,
    ArmyWindow,
    /// Looking at an enemy base before deploying; Next is available.
    Scouting,
    Battle,
    Results,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct Match {
    pub button: Button,
    /// Centre of the match, ready to tap.
    pub center: (u32, u32),
    /// Normalised squared error; lower is better.
    pub error: f32,
}

impl Match {
    pub fn found(&self) -> bool {
        self.error < MATCH_THRESHOLD
    }
}

pub struct Vision {
    templates: Vec<(Button, RgbImage)>,
}

impl Vision {
    /// Loads every button template from `dir`.
    pub fn load(dir: &Path) -> Result<Self> {
        let templates = Button::ALL
            .iter()
            .map(|&b| {
                let path = dir.join(b.file_name());
                let img = image::open(&path)
                    .with_context(|| format!("failed to load template {}", path.display()))?;
                Ok((b, img.to_rgb8()))
            })
            .collect::<Result<_>>()?;
        Ok(Self { templates })
    }

    /// Best match for one button, whether or not it clears the threshold.
    pub fn locate(&self, frame: &RgbImage, button: Button) -> Match {
        let template = &self.templates.iter().find(|(b, _)| *b == button).unwrap().1;
        let area = button.search_area();
        let region = image::imageops::crop_imm(
            frame,
            area.left() as u32,
            area.top() as u32,
            area.width(),
            area.height(),
        )
        .to_image();

        // Match each colour channel separately and average the errors: in
        // grayscale a green button looks a lot like grass.
        let mut scores: Option<Image<Luma<f32>, Vec<f32>>> = None;
        for c in 0..3 {
            let s = match_template_parallel(
                &channel(&region, c),
                &channel(template, c),
                MatchTemplateMethod::SumOfSquaredErrorsNormalized,
            );
            scores = Some(match scores {
                None => s,
                Some(mut acc) => {
                    acc.pixels_mut().zip(s.pixels()).for_each(|(a, b)| a.0[0] += b.0[0]);
                    acc
                }
            });
        }
        let mut scores = scores.unwrap();
        scores.pixels_mut().for_each(|p| p.0[0] /= 3.0);
        let ext = find_extremes(&scores);
        let (x, y) = ext.min_value_location;
        Match {
            button,
            center: (
                area.left() as u32 + x + template.width() / 2,
                area.top() as u32 + y + template.height() / 2,
            ),
            error: ext.min_value,
        }
    }

    /// Returns the button's tap point if it is on screen.
    pub fn find(&self, frame: &RgbImage, button: Button) -> Option<(u32, u32)> {
        let m = self.locate(frame, button);
        m.found().then_some(m.center)
    }

    pub fn screen(&self, frame: &RgbImage) -> Screen {
        let visible = |b| self.locate(frame, b).found();
        // The battle menu overlays the home screen, so check it before Home.
        if visible(Button::ReturnHome) {
            Screen::Results
        } else if visible(Button::Next) {
            Screen::Scouting
        } else if visible(Button::EndBattle) || visible(Button::Surrender) {
            Screen::Battle
        } else if visible(Button::ArmyAttack) {
            Screen::ArmyWindow
        } else if visible(Button::MenuPracticeTab) {
            Screen::BattleMenu
        } else if visible(Button::HomeBattle) {
            Screen::Home
        } else {
            Screen::Unknown
        }
    }
}

fn channel(img: &RgbImage, c: usize) -> GrayImage {
    GrayImage::from_fn(img.width(), img.height(), |x, y| Luma([img.get_pixel(x, y).0[c]]))
}
