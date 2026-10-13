# RAM Optimizer

A lightweight Windows memory optimizer written in Rust, with a neumorphism UI built in [Slint](https://slint.dev).

## Layout

- `crates/core`: memory stats and actions behind the `MemorySource` trait: `win::WindowsSource` (real data) and `MockSource` (demo data).
  - `trend.rs`: `GrowthTracker`, which flags apps whose memory keeps climbing (possible leaks).
  - `auto.rs`: `AutoClear`, which frees idle apps when RAM passes a threshold (30–90%), at most once per cooldown.
  - `game.rs`: game mode, plus `GameDetector` for turning it on and off automatically.
  - `history.rs`, `alerts.rs`, `checkup.rs`, `settings.rs`, `closing.rs`: 24-hour history, notifications, RAM check-up, saved settings, closing apps.
  - `win/`: the Windows implementation (memory, processes, registry, startup apps, start-at-sign-in, single instance).
- `crates/app`: the Slint GUI, a frameless transparent window where the rounded card is the whole app.
  - `ui/neu.slint`: neumorphism building blocks (`Raised`, `Inset`, `NeuButton`, `Palette`, `Icons`).
  - `ui/widgets.slint`: traffic lights, usage bar, history graph, rows, switches, segmented control.
  - `ui/app.slint`: the main window.
  - `src/`: `state.rs` (what the app does), `view.rs` (drawing it), `tray.rs`, `notify.rs`, `main.rs`.
  - `assets/`, `app.rc`, `app.manifest`: icon and Windows resources. Regenerate the icon with `python3 scripts/make_icons.py`.

## Run

```bash
cargo run -p ram-optimizer
```

On Windows it reads and acts on the real system (`crates/core/src/win/`). Elsewhere, or with `--demo`, it shows demo data.

## Build the Windows .exe from a Mac

```bash
rustup target add x86_64-pc-windows-gnu
brew install mingw-w64
./scripts/build-windows.sh
```

This produces `dist/RAMOptimizer-test-windows.zip` (a standalone .exe plus `HOW-TO-TEST.txt`). Releases are built on GitHub Actions instead, together with the installer (`installer/RAMOptimizer.iss`); see `scripts/release.sh`.

## License

MIT; see [LICENSE](LICENSE).
