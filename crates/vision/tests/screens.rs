//! Regression tests against real 1280x720 captures in tests/screens.

use std::path::Path;

use vision::{Button, Screen, Vision};

fn vision() -> Vision {
    Vision::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/templates")).unwrap()
}

fn frame(name: &str) -> image::RgbImage {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/screens").join(name);
    image::open(path).unwrap().to_rgb8()
}

#[test]
fn classifies_every_capture() {
    let v = vision();
    for (file, expected) in [
        ("home.png", Screen::Home),
        ("home_alt.png", Screen::Home),
        ("battle_menu.png", Screen::BattleMenu),
        ("single_player.png", Screen::BattleMenu),
        ("battle_start.png", Screen::Battle),
        ("battle_live.png", Screen::Battle),
        ("result.png", Screen::Results),
    ] {
        assert_eq!(v.screen(&frame(file)), expected, "{file}");
    }
}

#[test]
fn finds_buttons_where_expected() {
    let v = vision();
    let near = |a: (u32, u32), b: (u32, u32)| a.0.abs_diff(b.0) <= 5 && a.1.abs_diff(b.1) <= 5;

    let menu = frame("battle_menu.png");
    assert!(near(v.find(&menu, Button::FindMatch).unwrap(), (290, 513)));
    assert!(v.find(&menu, Button::SinglePlayerAttack).is_none());

    let sp = frame("single_player.png");
    assert!(near(v.find(&sp, Button::SinglePlayerAttack).unwrap(), (612, 506)));
    assert!(v.find(&sp, Button::FindMatch).is_none());
}
