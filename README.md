## Datum Tunnels

CLI, GUI app, and shared library for exposing local environments to the internet.

### Download

**macOS (Homebrew):**

```bash
brew install datum-cloud/tap/desktop
```

**nix**

```
# GUI app
nix run github:datum-cloud/app#desktop

# CLI
nix run github:datum-cloud/app#cli -- auth login
nix run github:datum-cloud/app#cli -- tunnel list
```

**Direct download:**

[![Download for macOS](https://img.shields.io/badge/Download-macOS-000000?logo=apple&logoColor=white)](https://github.com/datum-cloud/datum-connect/releases/latest/download/Datum.dmg)
[![Download for Windows](https://img.shields.io/badge/Download-Windows-0078D6?logo=windows&logoColor=white)](https://github.com/datum-cloud/datum-connect/releases/latest/download/Datum-setup.exe)
[![Download for Linux](https://img.shields.io/badge/Download-Linux-FCC624?logo=linux&logoColor=black)](https://github.com/datum-cloud/datum-connect/releases/latest/download/Datum.AppImage)

[Latest release →](https://github.com/datum-cloud/datum-connect/releases/latest)

### Development

**Requirements:** [Rust](https://rust-lang.org/tools/install/), [Dioxus CLI](https://dioxuslabs.com/learn/0.6/getting_started/) (`cargo install dioxus-cli` or `cargo binstall dioxus-cli`)

**Run the GUI:**

```bash
cd ui
dx serve --platform desktop
```

**Run the CLI:**

```bash
cd cli
cargo run -- --help
```
