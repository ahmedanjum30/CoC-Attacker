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

/// Troop cards visible in the army editor without scrolling (two rows).
pub const EDITOR_CARDS: usize = 18;

#[derive(Debug, Clone, Copy)]
pub struct EditorCard {
    /// Position in the editor, column by column (0 = top-left, 1 = below it).
    pub index: usize,
    /// Tap point that adds one of this troop to the army.
    pub pos: (u32, u32),
    pub unlocked: bool,
    /// Read from the badge; `None` if locked or unreadable.
    pub level: Option<u32>,
}

/// Reads which troops are unlocked in the army editor's troop picker.
pub fn editor_cards(ocr: &Ocr, frame: &RgbImage) -> Result<Vec<EditorCard>> {
    let mut cards = Vec::new();
    for index in 0..EDITOR_CARDS {
        let (col, row) = ((index / 2) as f32, (index % 2) as u32);
        let left = 44 + (130.5 * col) as u32;
        let top = 496 + 136 * row;
        // Unlocked troops carry a dark level badge in the card's corner.
        let dark = (0..16)
            .flat_map(|dy| (0..16).map(move |dx| (left + dx, top + dy)))
            .filter(|&(x, y)| {
                let p = frame.get_pixel(x, y).0;
                (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3 < 70
            })
            .count();
        let unlocked = dark > 16 * 16 / 2;
        let level = if unlocked {
            let area = Rect::at(left as i32 + 1, top as i32 + 1).of_size(17, 18);
            ocr.read_number(frame, area, TextColor::White)?
        } else {
            None
        };
        cards.push(EditorCard {
            index,
            pos: (95 + (130.5 * col) as u32, 492 + 136 * row),
            unlocked,
            level,
        });
    }
    Ok(cards)
}

/// Army camp usage, e.g. "20/30" -> (20, 30), from the army window.
pub fn army_capacity(ocr: &Ocr, frame: &RgbImage) -> Result<Option<(u32, u32)>> {
    // A red "!" follows the numbers when the camp isn't full; stop before it.
    let right = (430..540)
        .find(|&x| {
            let [r, g, b] = frame.get_pixel(x, 164).0;
            r > 190 && g < 90 && b < 90
        })
        .unwrap_or(540);
    let area = Rect::at(400, 150).of_size(right - 2 - 400, 30);
    let text = ocr.read(frame, area, TextColor::White)?;
    Ok(text
        .split_once('/')
        .and_then(|(a, b)| Some((crate::ocr::parse_number(a)?, crate::ocr::parse_number(b)?))))
}

/// "Overall Damage" percentage shown during a battle.
pub fn live_damage(ocr: &Ocr, frame: &RgbImage) -> Result<Option<u32>> {
    let pct = ocr.read_number(frame, Rect::at(1200, 522).of_size(65, 34), TextColor::White)?;
    Ok(pct.filter(|&p| p <= 100))
}

/// "Total damage" percentage on the results screen.
pub fn damage_percent(ocr: &Ocr, frame: &RgbImage) -> Result<Option<u32>> {
    let pct = ocr.read_number(frame, Rect::at(596, 100).of_size(95, 36), TextColor::Green)?;
    Ok(pct.filter(|&p| p <= 100))
}
