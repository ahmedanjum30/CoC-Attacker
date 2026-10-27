//! Reads numbers from fixed places in the game's UI.

use anyhow::Result;
use image::RgbImage;
use imageproc::rect::Rect;

use crate::ocr::{Ocr, TextColor};

/// Spacing between troop slots in the battle bar.
const SLOT_PITCH: i32 = 97;
const SLOT_COUNT: i32 = 11;

#[derive(Debug, Clone, Copy, Default)]
pub struct Loot {
    pub gold: u32,
    pub elixir: u32,
}

impl Loot {
    pub fn total(&self) -> u32 {
        self.gold + self.elixir
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TroopSlot {
    /// Tap point that selects this troop.
    pub pos: (u32, u32),
    pub count: u32,
}

/// Loot available in the enemy base, shown top-left while scouting.
pub fn enemy_loot(ocr: &Ocr, frame: &RgbImage) -> Result<Loot> {
    let read = |y| ocr.read_number(frame, Rect::at(62, y).of_size(90, 28), TextColor::White);
    Ok(Loot {
        gold: read(88)?.unwrap_or(0),
        elixir: read(126)?.unwrap_or(0),
    })
}

/// Troops in the battle bar, left to right, stopping at the first empty slot.
pub fn troop_slots(ocr: &Ocr, frame: &RgbImage) -> Result<Vec<TroopSlot>> {
    let mut slots = Vec::new();
    for i in 0..SLOT_COUNT {
        let left = 110 + i * SLOT_PITCH;
        // The "x20" count label at the top of the card.
        let area = Rect::at(left + 8, 597).of_size(80, 24);
        match ocr.read_number(frame, area, TextColor::White)? {
            Some(count) if count > 0 => slots.push(TroopSlot {
                pos: ((left + 44) as u32, 655),
                count,
            }),
            _ => break,
        }
    }
    Ok(slots)
}

/// "Total damage" percentage on the results screen.
pub fn damage_percent(ocr: &Ocr, frame: &RgbImage) -> Result<Option<u32>> {
    let pct = ocr.read_number(frame, Rect::at(596, 100).of_size(95, 36), TextColor::Green)?;
    Ok(pct.filter(|&p| p <= 100))
}
