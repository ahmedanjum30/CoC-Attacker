//! Reads numbers off the screen with a local neural OCR model (ocrs).

use std::path::Path;

use anyhow::{Context, Result};
use image::RgbImage;
use image::imageops::{self, FilterType};
use imageproc::rect::Rect;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;

pub struct Ocr {
    engine: OcrEngine,
}

impl Ocr {
    /// Loads `text-detection.onnx` and `text-recognition.onnx` from `dir`.
    pub fn load(dir: &Path) -> Result<Self> {
        let load = |name: &str| {
            let path = dir.join(name);
            Model::load_file(&path)
                .with_context(|| format!("failed to load OCR model {}", path.display()))
        };
        let engine = OcrEngine::new(OcrEngineParams {
            detection_model: Some(load("text-detection.onnx")?),
            recognition_model: Some(load("text-recognition.onnx")?),
            ..Default::default()
        })?;
        Ok(Self { engine })
    }

    /// Reads all text inside `area` of the frame as one line.
    pub fn read(&self, frame: &RgbImage, area: Rect, color: TextColor) -> Result<String> {
        let mut crop = imageops::crop_imm(
            frame,
            area.left() as u32,
            area.top() as u32,
            area.width(),
            area.height(),
        )
        .to_image();
        // Turn text-coloured pixels black and everything else white, so busy
        // backgrounds don't confuse the model.
        for p in crop.pixels_mut() {
            p.0 = if color.matches(p.0) { [0, 0, 0] } else { [255, 255, 255] };
        }
        // The game's numbers are small; the model reads them far better enlarged.
        let crop = imageops::resize(
            &crop,
            crop.width() * 3,
            crop.height() * 3,
            FilterType::CatmullRom,
        );
        let source = ImageSource::from_bytes(crop.as_raw(), crop.dimensions())?;
        let input = self.engine.prepare_input(source)?;
        let text = self.engine.get_text(&input)?;
        Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
    }

    /// Reads the digits inside `area` as one number, ignoring spaces and symbols.
    pub fn read_number(&self, frame: &RgbImage, area: Rect, color: TextColor) -> Result<Option<u32>> {
        let text = self.read(frame, area, color)?;
        Ok(parse_number(&text))
    }
}

/// Fill colour of the text to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextColor {
    /// Loot, counters and most UI numbers.
    White,
    /// The damage percentage on the results screen.
    Green,
}

impl TextColor {
    fn matches(self, [r, g, b]: [u8; 3]) -> bool {
        match self {
            TextColor::White => r > 200 && g > 200 && b > 200,
            TextColor::Green => g > 150 && g as i32 > r as i32 + 40 && g as i32 > b as i32 + 40,
        }
    }
}

/// Keeps only the digits ("1 210" -> 1210, "72%" -> 72), fixing letters the
/// model commonly confuses with digits in the game font.
pub fn parse_number(text: &str) -> Option<u32> {
    let digits: String = text
        .chars()
        .filter_map(|c| match c {
            'O' | 'o' | 'D' => Some('0'),
            'I' | 'l' | '|' => Some('1'),
            'S' | 's' => Some('5'),
            'B' => Some('8'),
            c if c.is_ascii_digit() => Some(c),
            _ => None,
        })
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::parse_number;

    #[test]
    fn parses_game_numbers() {
        assert_eq!(parse_number("1 210"), Some(1210));
        assert_eq!(parse_number("72%"), Some(72));
        assert_eq!(parse_number("5 S82"), Some(5582));
        assert_eq!(parse_number(""), None);
    }
}
