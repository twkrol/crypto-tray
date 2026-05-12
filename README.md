# Crypto Tray

**English** | [Polski](README.pl.md)

[![CI](https://github.com/twkrol/crypto-tray/actions/workflows/ci.yml/badge.svg)](https://github.com/twkrol/crypto-tray/actions/workflows/ci.yml)
[![Release](https://github.com/twkrol/crypto-tray/actions/workflows/release.yml/badge.svg)](https://github.com/twkrol/crypto-tray/actions/workflows/release.yml)
[![Latest release](https://img.shields.io/github/v/release/twkrol/crypto-tray)](https://github.com/twkrol/crypto-tray/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A small Windows application showing live cryptocurrency prices (BTC, ETH, XMR, KAS) as a floating widget that sits above the taskbar, plus a tray icon.

Written in Rust, compiles to a native `.exe` (~2 MB). Releases ship both **x64** and **ARM64** builds — pick the one matching your Windows install.

## Features

- Floating widget with USD price and 24h change for the selected coins
- Mini chart (sparkline) of the last hour next to each coin
- Coin icons fetched from CoinGecko (cached on disk, can be replaced with your own)
- Tray icon with a tooltip showing prices and a context menu
- Auto-detects Windows theme (light/dark) — widget matches the system colour
- Auto-detects system language (English / Polish)
- Auto-positioning above the taskbar; widget width adapts to how many coins are enabled
- Draggable with the mouse (left click + drag)
- Optional install into Windows startup (autorun)
- Update check via the GitHub Releases API

## Requirements

- Windows 10 or 11 (x86-64 or ARM64)
- To build: Rust toolchain (stable)

## Build

```bash
cargo build --release
```

Output: `target/release/crypto-tray.exe`.

## Run

```bash
crypto-tray.exe          # default price refresh interval (60 s)
crypto-tray.exe 30       # refresh every 30 s (minimum 5 s)
```

After launch you'll see the widget placed near the right edge of the taskbar by default, plus an icon in the system tray.

## Interaction

| Action | Effect |
|---|---|
| Left-click on widget + drag | move it across the screen |
| Double-click on widget | popup with full details (USD and PLN, 24h change) |
| Right-click on widget | context menu (same options as the tray menu) |
| Left-click on tray icon | popup with full details |
| Hover the tray icon | tooltip with current prices |
| Right-click on tray icon | menu (toggle coins, charts, autostart, refresh icons, info) |

## User configuration

Settings live in `%APPDATA%\CryptoTray\`:

- `coins.txt` — list of enabled coins (one CoinGecko id per line)
- `charts_off` — flag file; if it exists, sparklines are disabled
- `icons/<id>.png` — icon cache. Drop in your own square PNG with a transparent background named `bitcoin.png`, `ethereum.png`, `monero.png` or `kaspa.png` to replace the one fetched from CoinGecko.

Autostart entry lives under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\CryptoTray`. Toggled from the menu (no admin rights required). It's automatically removed when the app is uninstalled via the MSI.

## Data sources

[CoinGecko Public API v3](https://www.coingecko.com/en/api) — free tier, no API key:

- `/simple/price` — current prices (USD, PLN, 24h change)
- `/coins/markets` — coin logo URLs
- `/coins/{id}/market_chart/range` — last-hour price history for sparklines

Coin logos are the property of their respective projects.

## Stack

- [`tao`](https://crates.io/crates/tao) — windowing and event loop
- [`tray-icon`](https://crates.io/crates/tray-icon) + [`muda`](https://crates.io/crates/muda) — system tray icon and menus
- [`softbuffer`](https://crates.io/crates/softbuffer) — pixel buffer for the widget window
- [`png`](https://crates.io/crates/png) — PNG decoding for downloaded icons
- [`ureq`](https://crates.io/crates/ureq) — synchronous HTTP/JSON (rustls)
- [`windows-sys`](https://crates.io/crates/windows-sys) — Win32 API (GDI for Segoe UI text rendering, registry, taskbar, SetWindowPos)

## License

[MIT](LICENSE)
