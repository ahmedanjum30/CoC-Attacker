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
        ("scout1.png", Screen::Scouting),
        ("mp_battle.png", Screen::Battle),
        ("mp_result.png", Screen::Results),
        ("army_window.png", Screen::ArmyWindow),
        ("army_empty.png", Screen::ArmyWindow),
        ("army_not_full.png", Screen::ArmyWindow),
        ("army_tabs.png", Screen::ArmyWindow),
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

#[test]
fn reads_hud_numbers() {
    let ocr = vision::ocr::Ocr::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models"))
        .expect("OCR models missing: see README");

    let loot = vision::hud::enemy_loot(&ocr, &frame("scout1.png")).unwrap();
    assert_eq!((loot.gold, loot.elixir), (1210, 5582));

    let slots = vision::hud::troop_slots(&ocr, &frame("scout1.png")).unwrap();
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].count, 20);
    assert!(vision::hud::troop_slots(&ocr, &frame("mp_battle.png")).unwrap().is_empty());

    assert_eq!(vision::hud::damage_percent(&ocr, &frame("mp_result.png")).unwrap(), Some(72));
    assert_eq!(vision::hud::damage_percent(&ocr, &frame("result.png")).unwrap(), Some(100));
}

#[test]
fn finds_deploy_border_only_when_shown() {
    assert!(vision::deploy::find_deploy_zone(&frame("scout1.png"), 32).is_some());
    for file in ["home.png", "home_alt.png", "mp_battle.png", "result.png"] {
        assert!(vision::deploy::find_deploy_zone(&frame(file), 32).is_none(), "{file}");
    }
}

#[test]
fn reads_army_editor() {
    let ocr = vision::ocr::Ocr::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models"))
        .expect("OCR models missing: see README");
    let editor = frame("army_editor.png");

    let cards = vision::hud::editor_cards(&ocr, &editor).unwrap();
    let unlocked: Vec<_> = cards.iter().filter(|c| c.unlocked).collect();
    assert_eq!(unlocked.len(), 1, "only Barbarians are unlocked");
    assert_eq!(unlocked[0].index, 0);
    assert_eq!(unlocked[0].level, Some(1));

    assert_eq!(vision::hud::army_capacity(&ocr, &editor).unwrap(), Some((20, 20)));
    assert_eq!(vision::hud::live_damage(&ocr, &frame("mp_battle.png")).unwrap(), Some(0));
}

#[test]
fn reads_partly_filled_army_and_more_unlocks() {
    let ocr = vision::ocr::Ocr::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models"))
        .expect("OCR models missing: see README");

    let not_full = frame("army_not_full.png");
    assert_eq!(vision::hud::army_capacity(&ocr, &not_full).unwrap(), Some((20, 30)));

    let editor = frame("army_editor_three.png");
    assert_eq!(vision::hud::army_capacity(&ocr, &editor).unwrap(), Some((0, 30)));
    let cards = vision::hud::editor_cards(&ocr, &editor).unwrap();
    let unlocked: Vec<usize> = cards.iter().filter(|c| c.unlocked).map(|c| c.index).collect();
    // Barbarian, Archer, Giant.
    assert_eq!(unlocked, [0, 1, 2]);
}
