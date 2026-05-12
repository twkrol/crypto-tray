#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Deserialize;
use tao::dpi::{PhysicalPosition, PhysicalSize};
use tao::event::{ElementState, Event, MouseButton as TaoMouseButton, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
use tao::window::WindowBuilder;
use tray_icon::{
    menu::{CheckMenuItem, ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

const COINGECKO_URL: &str = "https://api.coingecko.com/api/v3/simple/price\
                             ?ids=bitcoin,ethereum,monero,kaspa\
                             &vs_currencies=usd,pln&include_24hr_change=true";
const APP_NAME: &str = "Crypto Tray";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_INTERVAL_SECS: u64 = 60;

const RIGHT_MARGIN_PX: i32 = 300;
const PAD_X: i32 = 10;
const FONT_FACE: &str = "Segoe UI";
const DOUBLE_CLICK_MS: u128 = 400;
const HEIGHT_REDUCTION_PX: i32 = 4;

#[derive(Clone, Copy)]
struct Theme {
    bg: u32,
    text: u32,
    dim: u32,
    up: u32,
    down: u32,
}

const THEME_DARK: Theme = Theme {
    bg: 0x00_20_20_20,
    text: 0x00_E0_E0_E0,
    dim: 0x00_9E_9E_9E,
    up: 0x00_4C_AF_50,
    down: 0x00_E5_3E_3E,
};

const THEME_LIGHT: Theme = Theme {
    bg: 0x00_F3_F3_F3,
    text: 0x00_1F_1F_1F,
    dim: 0x00_66_66_66,
    up: 0x00_2E_7D_32,
    down: 0x00_C6_28_28,
};

struct CoinMeta {
    id: &'static str,
    ticker: &'static str,
    name: &'static str,
    letter: &'static str,
    color_dark: u32,
    color_light: u32,
}

const COINS: &[CoinMeta] = &[
    CoinMeta {
        id: "bitcoin",
        ticker: "BTC",
        name: "Bitcoin",
        letter: "B",
        color_dark: 0x00_F7_93_1A,
        color_light: 0x00_C7_72_0A,
    },
    CoinMeta {
        id: "ethereum",
        ticker: "ETH",
        name: "Ethereum",
        letter: "E",
        color_dark: 0x00_62_7E_EA,
        color_light: 0x00_42_51_A0,
    },
    CoinMeta {
        id: "monero",
        ticker: "XMR",
        name: "Monero",
        letter: "M",
        color_dark: 0x00_FF_66_00,
        color_light: 0x00_CC_55_00,
    },
    CoinMeta {
        id: "kaspa",
        ticker: "KAS",
        name: "Kaspa",
        letter: "K",
        color_dark: 0x00_70_C7_BA,
        color_light: 0x00_2A_8A_7E,
    },
];

impl CoinMeta {
    fn color(&self, dark: bool) -> u32 {
        if dark {
            self.color_dark
        } else {
            self.color_light
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
struct CoinData {
    usd: f64,
    pln: f64,
    usd_24h_change: Option<f64>,
    pln_24h_change: Option<f64>,
}

#[derive(Debug, Deserialize, Clone)]
struct Prices {
    bitcoin: CoinData,
    ethereum: CoinData,
    monero: CoinData,
    kaspa: CoinData,
}

impl Prices {
    fn get(&self, id: &str) -> Option<&CoinData> {
        match id {
            "bitcoin" => Some(&self.bitcoin),
            "ethereum" => Some(&self.ethereum),
            "monero" => Some(&self.monero),
            "kaspa" => Some(&self.kaspa),
            _ => None,
        }
    }
}

#[derive(Clone, Default)]
struct AppState {
    data: Arc<Mutex<Option<Result<Prices, String>>>>,
    last_update: Arc<Mutex<Option<Instant>>>,
}

fn fetch_price() -> Result<Prices, String> {
    let resp = ureq::get(COINGECKO_URL)
        .set("User-Agent", concat!("crypto-tray/", env!("CARGO_PKG_VERSION")))
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(15))
        .call()
        .map_err(|e| format!("Błąd połączenia: {e}"))?;
    let parsed: Prices = resp
        .into_json()
        .map_err(|e| format!("Błąd parsowania JSON: {e}"))?;
    Ok(parsed)
}

fn fmt_price(v: f64) -> String {
    if v >= 1.0 {
        format!("{:.2}", v)
    } else {
        format!("{:.6}", v)
    }
}

fn fmt_change(c: Option<f64>) -> String {
    c.map(|v| format!("{:+.2}%", v)).unwrap_or_else(|| "—".into())
}

fn elapsed_human(t: Instant) -> String {
    let secs = t.elapsed().as_secs();
    if secs < 60 {
        format!("{secs} s temu")
    } else if secs < 3600 {
        format!("{} min {} s temu", secs / 60, secs % 60)
    } else {
        format!("{} h {} min temu", secs / 3600, (secs % 3600) / 60)
    }
}

fn format_coin_block(name: &str, ticker: &str, c: &CoinData) -> String {
    format!(
        "{name} ({ticker}):\n\
         \tUSD: ${}  ({} / 24h)\n\
         \tPLN: {} zł  ({} / 24h)",
        fmt_price(c.usd),
        fmt_change(c.usd_24h_change),
        fmt_price(c.pln),
        fmt_change(c.pln_24h_change),
    )
}

fn format_price_message(
    state: &AppState,
    enabled: &HashSet<String>,
    interval_secs: u64,
) -> String {
    let data = state.data.lock().unwrap();
    let last = state.last_update.lock().unwrap();

    let last_str = match *last {
        Some(t) => elapsed_human(t),
        None => "nigdy".to_string(),
    };

    match data.as_ref() {
        Some(Ok(p)) => {
            let blocks: Vec<String> = COINS
                .iter()
                .filter(|c| enabled.contains(c.id))
                .filter_map(|c| p.get(c.id).map(|d| format_coin_block(c.name, c.ticker, d)))
                .collect();
            if blocks.is_empty() {
                "Brak wybranych kryptowalut.\nZaznacz przynajmniej jedną w menu.".to_string()
            } else {
                format!(
                    "Aktualne kursy kryptowalut:\n\n{}\n\n\
                     Ostatnia aktualizacja: {}\n\
                     Interwał odświeżania: {} s\n\
                     Źródło: CoinGecko",
                    blocks.join("\n\n"),
                    last_str,
                    interval_secs,
                )
            }
        }
        Some(Err(e)) => format!(
            "Nie udało się pobrać kursów.\n\n{e}\n\nOstatnia próba: {last_str}"
        ),
        None => "Pobieranie kursów...\nSpróbuj ponownie za chwilę.".to_string(),
    }
}

fn format_tooltip(state: &AppState, enabled: &HashSet<String>) -> String {
    let data = state.data.lock().unwrap();
    match data.as_ref() {
        Some(Ok(p)) => {
            let lines: Vec<String> = COINS
                .iter()
                .filter(|c| enabled.contains(c.id))
                .filter_map(|c| p.get(c.id).map(|d| (c, d)))
                .map(|(c, d)| {
                    format!(
                        "{}: ${} ({})",
                        c.ticker,
                        fmt_price(d.usd),
                        fmt_change(d.usd_24h_change)
                    )
                })
                .collect();
            if lines.is_empty() {
                format!("{APP_NAME} – brak wybranych monet")
            } else {
                lines.join("\n")
            }
        }
        Some(Err(_)) => format!("{APP_NAME} – brak danych"),
        None => format!("{APP_NAME} – pobieranie..."),
    }
}

fn create_tray_icon() -> tray_icon::Icon {
    const SIZE: u32 = 32;
    const ORANGE: [u8; 4] = [0xF7, 0x93, 0x1A, 0xFF];
    const WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
    const TRANSPARENT: [u8; 4] = [0, 0, 0, 0];

    let b_pattern: [&[u8; 5]; 7] = [
        b"XXXX.",
        b"X...X",
        b"X...X",
        b"XXXX.",
        b"X...X",
        b"X...X",
        b"XXXX.",
    ];

    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    let center = SIZE as i32 / 2;
    let radius_sq = (center - 1) * (center - 1);

    for y in 0..SIZE as i32 {
        for x in 0..SIZE as i32 {
            let dx = x - center;
            let dy = y - center;
            let dist_sq = dx * dx + dy * dy;
            let idx = ((y * SIZE as i32 + x) * 4) as usize;

            let pixel = if dist_sq <= radius_sq {
                let bx = x - (center - 5);
                let by = y - (center - 7);
                if (0..10).contains(&bx) && (0..14).contains(&by) {
                    let px = (bx / 2) as usize;
                    let py = (by / 2) as usize;
                    if b_pattern[py][px] == b'X' {
                        WHITE
                    } else {
                        ORANGE
                    }
                } else {
                    ORANGE
                }
            } else {
                TRANSPARENT
            };

            rgba[idx..idx + 4].copy_from_slice(&pixel);
        }
    }

    tray_icon::Icon::from_rgba(rgba, SIZE, SIZE).expect("Failed to build icon")
}

#[cfg(windows)]
fn show_message(title: &str, text: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_SETFOREGROUND};

    let title_w: Vec<u16> = OsStr::new(title)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let text_w: Vec<u16> = OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    std::thread::spawn(move || unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text_w.as_ptr(),
            title_w.as_ptr(),
            MB_OK | MB_SETFOREGROUND,
        );
    });
}

// --- system metrics ---------------------------------------------------------

#[cfg(windows)]
fn get_taskbar_thickness() -> i32 {
    use windows_sys::Win32::UI::Shell::{SHAppBarMessage, ABM_GETTASKBARPOS, APPBARDATA};

    unsafe {
        let mut data: APPBARDATA = std::mem::zeroed();
        data.cbSize = std::mem::size_of::<APPBARDATA>() as u32;
        if SHAppBarMessage(ABM_GETTASKBARPOS, &mut data) != 0 {
            let r = data.rc;
            let h = (r.bottom - r.top) as i32;
            let w = (r.right - r.left) as i32;
            h.min(w)
        } else {
            48
        }
    }
}

#[cfg(windows)]
fn is_dark_theme() -> bool {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
    };

    let key_path: Vec<u16> = OsStr::new(
        r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
    )
    .encode_wide()
    .chain(std::iter::once(0))
    .collect();
    let value_name: Vec<u16> = OsStr::new("SystemUsesLightTheme")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut::<core::ffi::c_void>() as HKEY;
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key_path.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        ) != 0
        {
            return true;
        }

        let mut data: u32 = 0;
        let mut data_size = std::mem::size_of::<u32>() as u32;

        let result = RegQueryValueExW(
            hkey,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut data as *mut u32 as *mut u8,
            &mut data_size,
        );
        RegCloseKey(hkey);
        result != 0 || data == 0
    }
}

// --- enabled-coin selection persistence ------------------------------------

fn config_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|appdata| PathBuf::from(appdata).join("CryptoTray").join("coins.txt"))
}

fn load_enabled_coins() -> HashSet<String> {
    if let Some(path) = config_path() {
        if let Ok(s) = std::fs::read_to_string(&path) {
            return s
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    // Default — all four enabled.
    COINS.iter().map(|c| c.id.to_string()).collect()
}

fn save_enabled_coins(enabled: &HashSet<String>) {
    if let Some(path) = config_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Preserve canonical order from COINS so the file is stable on disk.
        let mut lines: Vec<&str> = COINS
            .iter()
            .filter(|c| enabled.contains(c.id))
            .map(|c| c.id)
            .collect();
        lines.sort();
        let _ = std::fs::write(&path, lines.join("\n"));
    }
}

fn charts_off_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|appdata| PathBuf::from(appdata).join("CryptoTray").join("charts_off"))
}

fn load_show_charts() -> bool {
    // Default ON; presence of the marker file disables sparklines.
    match charts_off_path() {
        Some(p) => !p.exists(),
        None => true,
    }
}

fn save_show_charts(show: bool) {
    if let Some(path) = charts_off_path() {
        if show {
            let _ = std::fs::remove_file(&path);
        } else {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, "");
        }
    }
}

/// Estimate the widget's needed width in physical pixels for the current selection.
/// Per-coin widths are conservative averages of typical Segoe UI 22px metrics.
fn compute_widget_width(enabled_count: usize, show_charts: bool) -> u32 {
    if enabled_count == 0 {
        return 100;
    }
    let per_coin: u32 = if show_charts { 310 } else { 250 };
    let pad: u32 = 20;
    enabled_count as u32 * per_coin + pad
}

// --- crypto icons -----------------------------------------------------------

struct IconImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>, // tightly packed RGBA u8
}

fn icon_cache_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|appdata| PathBuf::from(appdata).join("CryptoTray").join("icons"))
}

fn icon_cache_path(id: &str) -> Option<PathBuf> {
    icon_cache_dir().map(|d| d.join(format!("{id}.png")))
}

#[derive(Deserialize)]
struct CoinGeckoMarket {
    id: String,
    image: String,
}

fn fetch_icon_urls() -> HashMap<String, String> {
    let url = "https://api.coingecko.com/api/v3/coins/markets\
               ?vs_currency=usd&ids=bitcoin,ethereum,monero,kaspa";
    let mut map = HashMap::new();
    if let Ok(resp) = ureq::get(url)
        .set("User-Agent", concat!("crypto-tray/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .call()
    {
        if let Ok(markets) = resp.into_json::<Vec<CoinGeckoMarket>>() {
            for m in markets {
                map.insert(m.id, m.image);
            }
        }
    }
    map
}

fn download_url(url: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let resp = ureq::get(url)
        .set("User-Agent", concat!("crypto-tray/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .call()
        .ok()?;
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(5_000_000)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

fn decode_png_rgba(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let cursor = std::io::Cursor::new(bytes);
    let decoder = png::Decoder::new(cursor);
    let mut info_reader = decoder.read_info().ok()?;
    let (w, h, color_type) = {
        let info = info_reader.info();
        (info.width, info.height, info.color_type)
    };

    let mut buf = vec![0u8; info_reader.output_buffer_size()];
    info_reader.next_frame(&mut buf).ok()?;

    let rgba = match color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for chunk in buf.chunks_exact(3) {
                out.extend_from_slice(chunk);
                out.push(255);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for chunk in buf.chunks_exact(2) {
                out.push(chunk[0]);
                out.push(chunk[0]);
                out.push(chunk[0]);
                out.push(chunk[1]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for &g in &buf {
                out.push(g);
                out.push(g);
                out.push(g);
                out.push(255);
            }
            out
        }
        _ => return None, // palette etc. not handled here
    };

    Some((w, h, rgba))
}

fn resize_rgba_bilinear(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Vec<u8> {
    let mut out = vec![0u8; (dst_w * dst_h * 4) as usize];
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return out;
    }
    for dy in 0..dst_h {
        let sy_f = ((dy as f32 + 0.5) * src_h as f32 / dst_h as f32 - 0.5)
            .clamp(0.0, src_h as f32 - 1.0);
        let sy0 = sy_f.floor() as u32;
        let sy1 = (sy0 + 1).min(src_h - 1);
        let fy = sy_f - sy0 as f32;

        for dx in 0..dst_w {
            let sx_f = ((dx as f32 + 0.5) * src_w as f32 / dst_w as f32 - 0.5)
                .clamp(0.0, src_w as f32 - 1.0);
            let sx0 = sx_f.floor() as u32;
            let sx1 = (sx0 + 1).min(src_w - 1);
            let fx = sx_f - sx0 as f32;

            let p00 = ((sy0 * src_w + sx0) * 4) as usize;
            let p01 = ((sy0 * src_w + sx1) * 4) as usize;
            let p10 = ((sy1 * src_w + sx0) * 4) as usize;
            let p11 = ((sy1 * src_w + sx1) * 4) as usize;

            for c in 0..4 {
                let v = (src[p00 + c] as f32) * (1.0 - fx) * (1.0 - fy)
                    + (src[p01 + c] as f32) * fx * (1.0 - fy)
                    + (src[p10 + c] as f32) * (1.0 - fx) * fy
                    + (src[p11 + c] as f32) * fx * fy;
                out[((dy * dst_w + dx) * 4 + c as u32) as usize] =
                    v.clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

fn load_icons(target_size: u32) -> HashMap<String, IconImage> {
    let mut map = HashMap::new();

    // First pass: figure out which icons aren't on disk yet so we only hit
    // the network if needed.
    let mut needs_network = false;
    for coin in COINS {
        match icon_cache_path(coin.id) {
            Some(p) if p.exists() => {}
            _ => {
                needs_network = true;
                break;
            }
        }
    }

    let urls = if needs_network {
        fetch_icon_urls()
    } else {
        HashMap::new()
    };

    for coin in COINS {
        let bytes = (|| -> Option<Vec<u8>> {
            let path = icon_cache_path(coin.id)?;
            if let Ok(b) = std::fs::read(&path) {
                return Some(b);
            }
            let url = urls.get(coin.id)?;
            let bytes = download_url(url)?;
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, &bytes);
            Some(bytes)
        })();

        if let Some(bytes) = bytes {
            if let Some((w, h, rgba)) = decode_png_rgba(&bytes) {
                let resized =
                    resize_rgba_bilinear(&rgba, w, h, target_size, target_size);
                map.insert(
                    coin.id.to_string(),
                    IconImage {
                        width: target_size,
                        height: target_size,
                        rgba: resized,
                    },
                );
            }
        }
    }

    map
}

/// Force-refetch all icons from CoinGecko, bypassing the on-disk cache.
/// Returns a partial map of coins where fetch+decode succeeded — failed ones
/// are simply absent so the caller can merge into existing state without
/// wiping working icons.
fn force_refetch_icons(target_size: u32) -> HashMap<String, IconImage> {
    let mut map = HashMap::new();
    let urls = fetch_icon_urls();
    for coin in COINS {
        let Some(url) = urls.get(coin.id) else {
            continue;
        };
        let Some(bytes) = download_url(url) else {
            continue;
        };
        // Persist before decoding so a successful download is cached even
        // if our decoder rejects an unusual PNG variant.
        if let Some(path) = icon_cache_path(coin.id) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, &bytes);
        }
        if let Some((w, h, rgba)) = decode_png_rgba(&bytes) {
            let resized =
                resize_rgba_bilinear(&rgba, w, h, target_size, target_size);
            map.insert(
                coin.id.to_string(),
                IconImage {
                    width: target_size,
                    height: target_size,
                    rgba: resized,
                },
            );
        }
    }
    map
}

fn blit_icon_alpha(
    dib: &mut [u32],
    dib_w: usize,
    dib_h: usize,
    icon: &IconImage,
    dest_x: i32,
    dest_y: i32,
) {
    let iw = icon.width as i32;
    let ih = icon.height as i32;
    for iy in 0..ih {
        let dy = dest_y + iy;
        if dy < 0 || dy >= dib_h as i32 {
            continue;
        }
        for ix in 0..iw {
            let dx = dest_x + ix;
            if dx < 0 || dx >= dib_w as i32 {
                continue;
            }

            let pi = ((iy * iw + ix) * 4) as usize;
            let r = icon.rgba[pi] as u32;
            let g = icon.rgba[pi + 1] as u32;
            let b = icon.rgba[pi + 2] as u32;
            let a = icon.rgba[pi + 3] as u32;

            if a == 0 {
                continue;
            }
            let didx = (dy as usize) * dib_w + (dx as usize);

            if a == 255 {
                dib[didx] = (r << 16) | (g << 8) | b;
            } else {
                let bg = dib[didx];
                let bg_r = ((bg >> 16) & 0xFF) as u32;
                let bg_g = ((bg >> 8) & 0xFF) as u32;
                let bg_b = (bg & 0xFF) as u32;

                let nr = (r * a + bg_r * (255 - a)) / 255;
                let ng = (g * a + bg_g * (255 - a)) / 255;
                let nb = (b * a + bg_b * (255 - a)) / 255;

                dib[didx] = (nr << 16) | (ng << 8) | nb;
            }
        }
    }
}

// --- 1-hour price history (sparkline) ---------------------------------------

type ChartData = Arc<Mutex<HashMap<String, Vec<f64>>>>;

#[derive(Deserialize)]
struct MarketChartResponse {
    prices: Vec<[f64; 2]>,
}

fn fetch_chart(id: &str) -> Option<Vec<f64>> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let from = now.saturating_sub(3600);
    let url = format!(
        "https://api.coingecko.com/api/v3/coins/{id}/market_chart/range\
         ?vs_currency=usd&from={from}&to={now}"
    );
    let resp = ureq::get(&url)
        .set("User-Agent", concat!("crypto-tray/", env!("CARGO_PKG_VERSION")))
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(15))
        .call()
        .ok()?;
    let parsed: MarketChartResponse = resp.into_json().ok()?;
    Some(parsed.prices.into_iter().map(|p| p[1]).collect())
}

fn spawn_chart_thread(charts: ChartData, dirty: Arc<AtomicBool>) {
    // One thread per coin — at startup all four fetch in parallel so every
    // sparkline shows up within ~1s instead of waiting for a sequential walk.
    // Threads stagger by 200ms each to keep the initial burst within the rate
    // limit, and retry quickly (30s) on failure instead of waiting the full
    // refresh interval (15 min) — that's what was leaving sparklines blank
    // when the first fetch happened to coincide with rate-limited windows
    // (e.g. right after manual icon refresh).
    for (idx, coin) in COINS.iter().enumerate() {
        let charts = charts.clone();
        let dirty = dirty.clone();
        let coin_id = coin.id.to_string();
        let stagger_ms = (idx as u64) * 200;
        std::thread::spawn(move || {
            if stagger_ms > 0 {
                std::thread::sleep(Duration::from_millis(stagger_ms));
            }
            loop {
                if let Some(points) = fetch_chart(&coin_id) {
                    if let Ok(mut c) = charts.lock() {
                        c.insert(coin_id.clone(), points);
                    }
                    dirty.store(true, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_secs(900));
                } else {
                    std::thread::sleep(Duration::from_secs(30));
                }
            }
        });
    }
}

fn draw_line(
    dib: &mut [u32],
    dib_w: usize,
    dib_h: usize,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
) {
    // Bresenham — 1-pixel line. Sparkline is small enough that AA isn't needed.
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut x = x0;
    let mut y = y0;
    loop {
        if x >= 0 && (x as usize) < dib_w && y >= 0 && (y as usize) < dib_h {
            dib[(y as usize) * dib_w + (x as usize)] = color;
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

fn draw_sparkline(
    dib: &mut [u32],
    dib_w: usize,
    dib_h: usize,
    points: &[f64],
    rect_x: i32,
    rect_y: i32,
    rect_w: i32,
    rect_h: i32,
    color: u32,
) {
    if points.len() < 2 || rect_w < 2 || rect_h < 1 {
        return;
    }
    let min = points.iter().copied().fold(f64::INFINITY, f64::min);
    let max = points.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let range = (max - min).max(1e-12);
    let n = points.len();
    let last_idx = (n - 1) as f64;

    let mut prev: Option<(i32, i32)> = None;
    for (i, &p) in points.iter().enumerate() {
        let t = i as f64 / last_idx.max(1.0);
        let x = rect_x + (t * (rect_w - 1) as f64).round() as i32;
        let y_norm = (p - min) / range; // 0 = min, 1 = max
        let y = rect_y + rect_h - 1 - (y_norm * (rect_h - 1) as f64).round() as i32;
        if let Some((px, py)) = prev {
            draw_line(dib, dib_w, dib_h, px, py, x, y, color);
        }
        prev = Some((x, y));
    }
}

// --- autostart (HKCU\...\Run) -----------------------------------------------

const AUTOSTART_VALUE: &str = "CryptoTray";

#[cfg(windows)]
fn autostart_subkey_w() -> Vec<u16> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    OsStr::new(r"Software\Microsoft\Windows\CurrentVersion\Run")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn autostart_value_w() -> Vec<u16> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    OsStr::new(AUTOSTART_VALUE)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn is_autostart_enabled() -> bool {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE,
    };

    let key_path = autostart_subkey_w();
    let value_name = autostart_value_w();

    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut::<core::ffi::c_void>() as HKEY;
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key_path.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut hkey,
        ) != 0
        {
            return false;
        }
        let result = RegQueryValueExW(
            hkey,
            value_name.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        RegCloseKey(hkey);
        result == 0
    }
}

#[cfg(windows)]
fn enable_autostart(interval_secs: u64) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ,
    };

    let exe = std::env::current_exe().map_err(|e| format!("Brak ścieżki .exe: {e}"))?;
    let exe_str = exe.to_string_lossy().into_owned();
    let cmd = format!("\"{}\" {}", exe_str, interval_secs);
    let cmd_w: Vec<u16> = OsStr::new(&cmd)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let key_path = autostart_subkey_w();
    let value_name = autostart_value_w();

    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut::<core::ffi::c_void>() as HKEY;
        let r = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key_path.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if r != 0 {
            return Err(format!("RegOpenKeyExW: błąd {r}"));
        }
        let r = RegSetValueExW(
            hkey,
            value_name.as_ptr(),
            0,
            REG_SZ,
            cmd_w.as_ptr() as *const u8,
            (cmd_w.len() * 2) as u32,
        );
        RegCloseKey(hkey);
        if r != 0 {
            return Err(format!("RegSetValueExW: błąd {r}"));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn disable_autostart() -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
    };

    let key_path = autostart_subkey_w();
    let value_name = autostart_value_w();

    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut::<core::ffi::c_void>() as HKEY;
        let r = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key_path.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        );
        if r != 0 {
            return Err(format!("RegOpenKeyExW: błąd {r}"));
        }
        let r = RegDeleteValueW(hkey, value_name.as_ptr());
        RegCloseKey(hkey);
        if r != 0 {
            return Err(format!("RegDeleteValueW: błąd {r}"));
        }
    }
    Ok(())
}

fn autostart_label(enabled: bool) -> &'static str {
    if enabled {
        "Usuń z autostartu"
    } else {
        "Zainstaluj w autostarcie"
    }
}

// --- widget rendering (GDI + Segoe UI, AA icon circles) ---------------------

fn rgb_to_colorref(packed: u32) -> u32 {
    // Theme stores 0x00RRGGBB. Win32 COLORREF is 0x00BBGGRR.
    let r = (packed >> 16) & 0xFF;
    let g = (packed >> 8) & 0xFF;
    let b = packed & 0xFF;
    (b << 16) | (g << 8) | r
}

fn blend_rgb(bg: u32, fg: u32, alpha: f32) -> u32 {
    let a = alpha.clamp(0.0, 1.0);
    let bg_r = ((bg >> 16) & 0xFF) as f32;
    let bg_g = ((bg >> 8) & 0xFF) as f32;
    let bg_b = (bg & 0xFF) as f32;
    let fg_r = ((fg >> 16) & 0xFF) as f32;
    let fg_g = ((fg >> 8) & 0xFF) as f32;
    let fg_b = (fg & 0xFF) as f32;

    let r = (bg_r * (1.0 - a) + fg_r * a) as u32;
    let g = (bg_g * (1.0 - a) + fg_g * a) as u32;
    let b = (bg_b * (1.0 - a) + fg_b * a) as u32;
    ((r & 0xFF) << 16) | ((g & 0xFF) << 8) | (b & 0xFF)
}

fn draw_circle_aa(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    cx: i32,
    cy: i32,
    radius: f32,
    color: u32,
) {
    let x_start = ((cx as f32 - radius - 1.0).floor() as i32).max(0);
    let x_end = ((cx as f32 + radius + 1.0).ceil() as i32).min(buf_w as i32 - 1);
    let y_start = ((cy as f32 - radius - 1.0).floor() as i32).max(0);
    let y_end = ((cy as f32 + radius + 1.0).ceil() as i32).min(buf_h as i32 - 1);

    let r_in = radius - 0.5;
    let r_out = radius + 0.5;

    for y in y_start..=y_end {
        for x in x_start..=x_end {
            let dx = (x - cx) as f32;
            let dy = (y - cy) as f32;
            let dist = (dx * dx + dy * dy).sqrt();
            let idx = (y as usize) * buf_w + (x as usize);

            if dist <= r_in {
                buf[idx] = color;
            } else if dist <= r_out {
                let coverage = (r_out - dist).clamp(0.0, 1.0);
                buf[idx] = blend_rgb(buf[idx], color, coverage);
            }
        }
    }
}

#[cfg(windows)]
unsafe fn draw_segment(
    dc: *mut core::ffi::c_void,
    x: i32,
    y: i32,
    text: &str,
    color: u32,
) -> i32 {
    use windows_sys::Win32::Foundation::SIZE;
    use windows_sys::Win32::Graphics::Gdi::{GetTextExtentPoint32W, SetTextColor, TextOutW};

    let text_w: Vec<u16> = text.encode_utf16().collect();
    SetTextColor(dc, rgb_to_colorref(color));
    TextOutW(dc, x, y, text_w.as_ptr(), text_w.len() as i32);
    let mut sz: SIZE = std::mem::zeroed();
    GetTextExtentPoint32W(dc, text_w.as_ptr(), text_w.len() as i32, &mut sz);
    x + sz.cx
}

#[cfg(windows)]
unsafe fn measure_text(dc: *mut core::ffi::c_void, text: &str) -> i32 {
    use windows_sys::Win32::Foundation::SIZE;
    use windows_sys::Win32::Graphics::Gdi::GetTextExtentPoint32W;
    let text_w: Vec<u16> = text.encode_utf16().collect();
    let mut sz: SIZE = std::mem::zeroed();
    GetTextExtentPoint32W(dc, text_w.as_ptr(), text_w.len() as i32, &mut sz);
    sz.cx
}

#[cfg(windows)]
fn render_widget(
    buffer: &mut [u32],
    w: u32,
    h: u32,
    state: &AppState,
    enabled: &HashSet<String>,
    icons: &HashMap<String, IconImage>,
    charts: &ChartData,
    show_charts: bool,
    theme: Theme,
    is_dark: bool,
    font_h: i32,
) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{RECT, SIZE};
    use windows_sys::Win32::Graphics::Gdi::{
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
        CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
        DEFAULT_PITCH, DIB_RGB_COLORS, DeleteDC, DeleteObject, FF_SWISS, FW_NORMAL, FillRect,
        GetDC, GetTextExtentPoint32W, GetTextMetricsW, OUT_DEFAULT_PRECIS, RGBQUAD, ReleaseDC,
        SelectObject, SetBkMode, SetTextColor, TEXTMETRICW, TRANSPARENT, TextOutW,
    };

    unsafe {
        let screen_dc = GetDC(null_mut());
        let mem_dc = CreateCompatibleDC(screen_dc);
        ReleaseDC(null_mut(), screen_dc);

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB as u32,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbReserved: 0,
            }],
        };
        let mut dib_bits: *mut core::ffi::c_void = null_mut();
        let dib = CreateDIBSection(
            mem_dc,
            &bmi,
            DIB_RGB_COLORS,
            &mut dib_bits,
            null_mut(),
            0,
        );
        if dib.is_null() || dib_bits.is_null() {
            DeleteDC(mem_dc);
            let count = (w * h) as usize;
            if buffer.len() >= count {
                buffer[..count].fill(theme.bg);
            }
            return;
        }
        let old_dib = SelectObject(mem_dc, dib as _);

        // Background fill via GDI.
        let brush = CreateSolidBrush(rgb_to_colorref(theme.bg));
        let rect = RECT {
            left: 0,
            top: 0,
            right: w as i32,
            bottom: h as i32,
        };
        FillRect(mem_dc, &rect, brush);
        DeleteObject(brush as _);

        // Font
        let face_w: Vec<u16> = OsStr::new(FONT_FACE)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let font = CreateFontW(
            -font_h,
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_SWISS) as u32,
            face_w.as_ptr(),
        );
        let old_font = SelectObject(mem_dc, font as _);
        SetBkMode(mem_dc, TRANSPARENT as i32);

        let mut tm: TEXTMETRICW = std::mem::zeroed();
        GetTextMetricsW(mem_dc, &mut tm);
        let y_text = ((h as i32) - tm.tmHeight) / 2;

        // Pixel slice for AA circle drawing.
        let pixel_count = (w * h) as usize;
        let dib_mut = std::slice::from_raw_parts_mut(dib_bits as *mut u32, pixel_count);

        let data = state.data.lock().unwrap();
        let prices: Option<&Prices> = match data.as_ref() {
            Some(Ok(p)) => Some(p),
            _ => None,
        };

        let icon_d = ((h as i32) * 7 / 10).max(20);
        let icon_radius = (icon_d as f32) / 2.0;
        let cy = (h as i32) / 2;
        let chart_w: i32 = 50;
        let chart_h: i32 = ((h as i32) * 6 / 10).max(10);
        let chart_y: i32 = ((h as i32) - chart_h) / 2;

        let charts_lock = charts.lock().ok();

        let mut x = PAD_X;
        let space_w = measure_text(mem_dc, "    ");

        for coin in COINS.iter() {
            if !enabled.contains(coin.id) {
                continue;
            }

            if let Some(icon) = icons.get(coin.id) {
                // Real CoinGecko icon — alpha-blend onto DIB.
                let icon_top = cy - icon_d / 2;
                blit_icon_alpha(dib_mut, w as usize, h as usize, icon, x, icon_top);
            } else {
                // Fallback: AA circle + brand letter (used when icon download
                // failed or cache is missing on first run with no network).
                let brand = coin.color(is_dark);
                let cx_circle = x + icon_d / 2;
                draw_circle_aa(
                    dib_mut,
                    w as usize,
                    h as usize,
                    cx_circle,
                    cy,
                    icon_radius,
                    brand,
                );
                let letter_w: Vec<u16> = coin.letter.encode_utf16().collect();
                let mut sz: SIZE = std::mem::zeroed();
                GetTextExtentPoint32W(
                    mem_dc,
                    letter_w.as_ptr(),
                    letter_w.len() as i32,
                    &mut sz,
                );
                SetTextColor(mem_dc, rgb_to_colorref(0x00_FF_FF_FF));
                TextOutW(
                    mem_dc,
                    cx_circle - sz.cx / 2,
                    cy - sz.cy / 2,
                    letter_w.as_ptr(),
                    letter_w.len() as i32,
                );
            }

            x += icon_d + 6;

            // Price + 24h change.
            if let Some(p) = prices {
                if let Some(c) = p.get(coin.id) {
                    x = draw_segment(
                        mem_dc,
                        x,
                        y_text,
                        &format!("${}", fmt_price(c.usd)),
                        theme.text,
                    );
                    if let Some(v) = c.usd_24h_change {
                        let color = if v > 0.0 {
                            theme.up
                        } else if v < 0.0 {
                            theme.down
                        } else {
                            theme.dim
                        };
                        x = draw_segment(mem_dc, x, y_text, &format!("  {:+.2}%", v), color);
                    }
                }
            } else {
                x = draw_segment(mem_dc, x, y_text, "...", theme.dim);
            }

            // Sparkline of last hour's prices (if enabled & data is loaded).
            if show_charts {
                x += 8;
                if let Some(charts_map) = charts_lock.as_ref() {
                    if let Some(points) = charts_map.get(coin.id) {
                        let line_color = match (points.first(), points.last()) {
                            (Some(&first), Some(&last)) if last > first => theme.up,
                            (Some(&first), Some(&last)) if last < first => theme.down,
                            _ => theme.dim,
                        };
                        draw_sparkline(
                            dib_mut,
                            w as usize,
                            h as usize,
                            points,
                            x,
                            chart_y,
                            chart_w,
                            chart_h,
                            line_color,
                        );
                    }
                }
                x += chart_w;
            }

            x += space_w;
        }

        // Copy DIB to softbuffer.
        let dib_slice = std::slice::from_raw_parts(dib_bits as *const u32, pixel_count);
        if buffer.len() >= pixel_count {
            buffer[..pixel_count].copy_from_slice(dib_slice);
        }

        SelectObject(mem_dc, old_font);
        SelectObject(mem_dc, old_dib);
        DeleteObject(font as _);
        DeleteObject(dib as _);
        DeleteDC(mem_dc);
    }
}

// ----------------------------------------------------------------------------

fn main() {
    let interval_secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .filter(|v| *v >= 5)
        .unwrap_or(DEFAULT_INTERVAL_SECS);

    let state = AppState::default();
    let mut enabled_coins: HashSet<String> = load_enabled_coins();

    let (refresh_tx, refresh_rx) = mpsc::channel::<()>();

    {
        let state = state.clone();
        std::thread::spawn(move || loop {
            let result = fetch_price();
            {
                let mut d = state.data.lock().unwrap();
                *d = Some(result);
                let mut t = state.last_update.lock().unwrap();
                *t = Some(Instant::now());
            }
            let _ = refresh_rx.recv_timeout(Duration::from_secs(interval_secs));
            while refresh_rx.try_recv().is_ok() {}
        });
    }

    let event_loop = EventLoopBuilder::new().build();

    // --- system metrics ---------------------------------------------------
    let taskbar_thickness = get_taskbar_thickness();
    let is_dark = is_dark_theme();
    let theme = if is_dark { THEME_DARK } else { THEME_LIGHT };

    let widget_height_px: u32 =
        (taskbar_thickness - HEIGHT_REDUCTION_PX).max(20) as u32;
    let font_h: i32 = ((widget_height_px as f32) * 0.50)
        .round()
        .clamp(11.0, 24.0) as i32;
    let mut show_charts: bool = load_show_charts();
    let widget_width_px: u32 = compute_widget_width(enabled_coins.len(), show_charts);
    let icon_d_px = ((widget_height_px as i32) * 7 / 10).max(20) as u32;

    // Load real coin icons (cached in %APPDATA%\CryptoTray\icons; fetched
    // from CoinGecko on first run or whenever cache is missing).
    let mut icons = load_icons(icon_d_px);

    // 1-hour price history for sparklines, refreshed in the background.
    let charts: ChartData = Arc::new(Mutex::new(HashMap::new()));
    let charts_dirty: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    spawn_chart_thread(charts.clone(), charts_dirty.clone());

    // --- tray icon + menu --------------------------------------------------
    let menu = Menu::new();
    let item_show = MenuItem::new("Pokaż kursy", true, None);
    let item_refresh = MenuItem::new("Odśwież teraz", true, None);
    let sep1 = PredefinedMenuItem::separator();

    let item_btc = CheckMenuItem::new("BTC", true, enabled_coins.contains("bitcoin"), None);
    let item_eth = CheckMenuItem::new("ETH", true, enabled_coins.contains("ethereum"), None);
    let item_xmr = CheckMenuItem::new("XMR", true, enabled_coins.contains("monero"), None);
    let item_kas = CheckMenuItem::new("KAS", true, enabled_coins.contains("kaspa"), None);

    let sep2 = PredefinedMenuItem::separator();
    let item_charts = CheckMenuItem::new("Wykresy", true, show_charts, None);
    let item_refresh_icons = MenuItem::new("Odśwież ikony", true, None);
    let sep3 = PredefinedMenuItem::separator();
    let item_about = MenuItem::new("O programie...", true, None);
    let item_autostart =
        MenuItem::new(autostart_label(is_autostart_enabled()), true, None);
    let sep4 = PredefinedMenuItem::separator();
    let item_quit = MenuItem::new("Zakończ", true, None);

    menu.append_items(&[
        &item_show,
        &item_refresh,
        &sep1,
        &item_btc,
        &item_eth,
        &item_xmr,
        &item_kas,
        &sep2,
        &item_charts,
        &item_refresh_icons,
        &sep3,
        &item_about,
        &item_autostart,
        &sep4,
        &item_quit,
    ])
    .expect("Failed to build menu");

    let id_show = item_show.id().clone();
    let id_refresh = item_refresh.id().clone();
    let id_btc = item_btc.id().clone();
    let id_eth = item_eth.id().clone();
    let id_xmr = item_xmr.id().clone();
    let id_kas = item_kas.id().clone();
    let id_charts = item_charts.id().clone();
    let id_refresh_icons = item_refresh_icons.id().clone();
    let id_about = item_about.id().clone();
    let id_autostart = item_autostart.id().clone();
    let id_quit = item_quit.id().clone();

    let menu_for_widget = menu.clone();

    let mut tray: Option<TrayIcon> = Some(
        TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(format!("{APP_NAME} – pobieranie..."))
            .with_icon(create_tray_icon())
            .build()
            .expect("Failed to build tray icon"),
    );

    // --- widget window -----------------------------------------------------
    let widget = Arc::new(
        WindowBuilder::new()
            .with_title(APP_NAME)
            .with_inner_size(PhysicalSize::new(widget_width_px, widget_height_px))
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top(true)
            .with_skip_taskbar(true)
            .with_undecorated_shadow(false)
            .with_focused(false)
            .with_visible(true)
            .build(&event_loop)
            .expect("Failed to build widget window"),
    );

    if let Some(monitor) = widget.current_monitor() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
        };
        let mp = monitor.position();
        let ms = monitor.size();
        let screen_right = mp.x + ms.width as i32;
        let screen_bottom = mp.y + ms.height as i32;
        let x = screen_right - widget_width_px as i32 - RIGHT_MARGIN_PX;
        let y = screen_bottom - widget_height_px as i32;
        let hwnd = widget.hwnd();
        unsafe {
            SetWindowPos(
                hwnd as *mut std::ffi::c_void,
                std::ptr::null_mut(),
                x,
                y,
                widget_width_px as i32,
                widget_height_px as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        let _ = widget.set_outer_position(PhysicalPosition::new(x, y));
    }

    let context = softbuffer::Context::new(widget.clone()).expect("softbuffer context");
    let mut surface =
        softbuffer::Surface::new(&context, widget.clone()).expect("softbuffer surface");

    let menu_channel = MenuEvent::receiver();
    let tray_channel = TrayIconEvent::receiver();

    let mut last_rendered: Option<Instant> = None;
    let mut last_raise = Instant::now() - Duration::from_secs(10);
    let mut last_left_press: Option<Instant> = None;
    let widget_id = widget.id();

    // Helper closure-ish: refresh tooltip text from current state + selection.
    // Inlined where used because tray is moved into the closure.

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(200));

        if last_raise.elapsed() >= Duration::from_millis(500) {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                HWND_TOPMOST, SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
            };
            unsafe {
                SetWindowPos(
                    widget.hwnd() as *mut std::ffi::c_void,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
            last_raise = Instant::now();
        }

        match &event {
            Event::WindowEvent {
                event: window_event,
                window_id,
                ..
            } if *window_id == widget_id => match window_event {
                WindowEvent::MouseInput {
                    state: btn_state,
                    button,
                    ..
                } => {
                    if *btn_state == ElementState::Pressed {
                        match button {
                            TaoMouseButton::Left => {
                                let now = Instant::now();
                                let is_double = last_left_press
                                    .map(|t| {
                                        now.duration_since(t).as_millis() < DOUBLE_CLICK_MS
                                    })
                                    .unwrap_or(false);
                                if is_double {
                                    last_left_press = None;
                                    let msg = format_price_message(
                                        &state,
                                        &enabled_coins,
                                        interval_secs,
                                    );
                                    show_message(&format!("{APP_NAME} – kursy"), &msg);
                                } else {
                                    last_left_press = Some(now);
                                    let _ = widget.drag_window();
                                }
                            }
                            TaoMouseButton::Right => {
                                use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
                                let hwnd = widget.hwnd();
                                unsafe {
                                    SetForegroundWindow(hwnd as *mut std::ffi::c_void);
                                    menu_for_widget
                                        .show_context_menu_for_hwnd(hwnd as isize, None);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                WindowEvent::CloseRequested => {}
                _ => {}
            },
            Event::RedrawRequested(window_id) if *window_id == widget_id => {
                let size = widget.inner_size();
                let w = size.width.max(1);
                let h = size.height.max(1);
                if let (Some(nz_w), Some(nz_h)) = (NonZeroU32::new(w), NonZeroU32::new(h)) {
                    if surface.resize(nz_w, nz_h).is_ok() {
                        if let Ok(mut buffer) = surface.buffer_mut() {
                            render_widget(
                                &mut buffer,
                                w,
                                h,
                                &state,
                                &enabled_coins,
                                &icons,
                                &charts,
                                show_charts,
                                theme,
                                is_dark,
                                font_h,
                            );
                            let _ = buffer.present();
                        }
                    }
                }
            }
            _ => {}
        }

        let current_update = *state.last_update.lock().unwrap();
        if current_update != last_rendered {
            if let Some(t) = tray.as_ref() {
                let _ = t.set_tooltip(Some(format_tooltip(&state, &enabled_coins)));
            }
            widget.request_redraw();
            last_rendered = current_update;
        }

        while let Ok(ev) = menu_channel.try_recv() {
            // Resize widget to match current selection — keep right edge fixed
            // so user-dragged position is roughly preserved.
            let resize_to_fit = |new_width: u32| {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
                };
                let cur_pos = widget
                    .outer_position()
                    .unwrap_or(PhysicalPosition::new(0, 0));
                let cur_size = widget.outer_size();
                let right = cur_pos.x + cur_size.width as i32;
                let new_x = right - new_width as i32;
                unsafe {
                    SetWindowPos(
                        widget.hwnd() as *mut std::ffi::c_void,
                        std::ptr::null_mut(),
                        new_x,
                        cur_pos.y,
                        new_width as i32,
                        cur_size.height as i32,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            };

            // Helper: handle a coin checkbox toggle.
            let mut toggle_coin = |id: &str, item: &CheckMenuItem| {
                let was = enabled_coins.contains(id);
                if was {
                    enabled_coins.remove(id);
                } else {
                    enabled_coins.insert(id.to_string());
                }
                let new_state = !was;
                item.set_checked(new_state);
                save_enabled_coins(&enabled_coins);
                if let Some(t) = tray.as_ref() {
                    let _ = t.set_tooltip(Some(format_tooltip(&state, &enabled_coins)));
                }
                resize_to_fit(compute_widget_width(enabled_coins.len(), show_charts));
                widget.request_redraw();
            };

            if ev.id == id_show {
                let msg = format_price_message(&state, &enabled_coins, interval_secs);
                show_message(&format!("{APP_NAME} – kursy"), &msg);
            } else if ev.id == id_refresh {
                let _ = refresh_tx.send(());
                show_message(
                    APP_NAME,
                    "Wymuszono odświeżenie kursów.\nNowe dane pojawią się za chwilę.",
                );
            } else if ev.id == id_btc {
                toggle_coin("bitcoin", &item_btc);
            } else if ev.id == id_eth {
                toggle_coin("ethereum", &item_eth);
            } else if ev.id == id_xmr {
                toggle_coin("monero", &item_xmr);
            } else if ev.id == id_kas {
                toggle_coin("kaspa", &item_kas);
            } else if ev.id == id_charts {
                show_charts = !show_charts;
                item_charts.set_checked(show_charts);
                save_show_charts(show_charts);
                resize_to_fit(compute_widget_width(enabled_coins.len(), show_charts));
                widget.request_redraw();
            } else if ev.id == id_refresh_icons {
                // Force-refetch (bypass cache). Merge new icons into existing
                // map so coins whose fetch failed keep their previous icon
                // instead of regressing to the circle+letter fallback.
                let new_icons = force_refetch_icons(icon_d_px);
                let count = new_icons.len();
                for (k, v) in new_icons {
                    icons.insert(k, v);
                }
                widget.request_redraw();
                show_message(
                    APP_NAME,
                    &format!(
                        "Pobrano {} z {} ikon z CoinGecko.",
                        count,
                        COINS.len()
                    ),
                );
            } else if ev.id == id_autostart {
                let was_enabled = is_autostart_enabled();
                let result = if was_enabled {
                    disable_autostart()
                } else {
                    enable_autostart(interval_secs)
                };
                match result {
                    Ok(()) => {
                        let now_enabled = is_autostart_enabled();
                        item_autostart.set_text(autostart_label(now_enabled));
                        show_message(
                            APP_NAME,
                            if now_enabled {
                                "Aplikacja będzie uruchamiana przy starcie systemu."
                            } else {
                                "Aplikacja została usunięta z autostartu."
                            },
                        );
                    }
                    Err(e) => {
                        show_message(
                            APP_NAME,
                            &format!("Nie udało się zmienić autostartu:\n\n{e}"),
                        );
                    }
                }
            } else if ev.id == id_about {
                let about = format!(
                    "{APP_NAME}\n\
                     Wersja {APP_VERSION}\n\n\
                     Mała aplikacja pokazująca aktualne kursy kryptowalut.\n\
                     Obsługiwane: BTC, ETH, XMR, KAS\n\
                     (zaznacz w menu, które pokazywać)\n\n\
                     • Pływający widżet – stale widoczny nad paskiem zadań\n\
                     • Lewy klik widżetu i przeciągnij – zmień pozycję\n\
                     • Podwójny klik widżetu – pełne informacje\n\
                     • Prawy klik widżetu – menu kontekstowe\n\
                     • Ikona w trayu – tooltip z kursami, prawy klik = menu\n\
                     • Interwał odświeżania: {interval_secs} s\n\n\
                     Źródło danych: api.coingecko.com (publiczne, darmowe API)\n\
                     Napisane w języku Rust."
                );
                show_message(&format!("O programie – {APP_NAME}"), &about);
            } else if ev.id == id_quit {
                tray.take();
                *control_flow = ControlFlow::Exit;
            }
        }

        while let Ok(ev) = tray_channel.try_recv() {
            if let TrayIconEvent::Click {
                button,
                button_state,
                ..
            } = ev
            {
                if button == MouseButton::Left && button_state == MouseButtonState::Up {
                    let msg = format_price_message(&state, &enabled_coins, interval_secs);
                    show_message(&format!("{APP_NAME} – kursy"), &msg);
                }
            }
        }
    });
}
