# CoC-Attacker

A Clash of Clans attack bot written in Rust. All AI runs locally: YOLO object detection with ONNX Runtime on the GPU. No cloud APIs.

> ⚠️ Botting breaks Supercell's Terms of Service and can get accounts banned. Use an alt account and test on single-player Goblin maps first.

## Architecture

```
Emulator ──frames──▶ vision (YOLO + digit OCR) ──▶ brain (evaluate / plan) ──▶ device (taps)
```

| Crate | Purpose |
|---|---|
| `crates/device` | ADB connection, screenshots, taps and swipes |
| `crates/vision` | Template matching for buttons; screen classification |
| `crates/app` | CLI and the game state machine (`bot.rs`) |

Planned: YOLO building detection (ONNX inference via `ort` + CUDA), `brain` (base evaluation, deploy planning) and `training/` (Python, model training only).

## Setup

1. Install **LDPlayer 9** and set it to **1280×720, 240 DPI** (Settings → Advanced). The bot's templates assume a fixed resolution.
2. Enable ADB: Settings → Other settings → ADB debugging → *Open local connection*. Restart the emulator.
3. Install Clash of Clans in the emulator and log in with an alt account.
4. `cp config.example.toml config.toml` and check `adb_path` and `serial`. Run `adb devices` to see the serial.

## Usage

```sh
cargo run -- info                      # check connection and resolution
cargo run -- screenshot                # save captures/screen.png
cargo run -- tap 640 360
cargo run -- swipe 400 360 900 360 --ms 500
cargo run -- detect                    # which screen is showing + match scores
cargo run -- home                      # navigate back to the village
cargo run -- goblin --count 3          # play single-player Goblin maps
```

## Roadmap

1. [x] ADB control: screenshots, taps, swipes
2. [x] State machine: home → battle menu → single-player attack → results → home (button template matching)
   - [ ] Multiplayer: find match → scout (Next button) → attack
3. [ ] Loot OCR and skip logic
4. [ ] YOLO building detection and deploy planning
5. [ ] Learn from logged attacks
