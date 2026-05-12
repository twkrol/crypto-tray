#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
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

/// Build the `/simple/price` URL covering every coin in `COINS`. We always
/// fetch the full set even if only a subset is enabled — one HTTP request
/// keeps the rate-limit budget tiny, and CoinGecko's response is still
/// only ~5 KB even for 30 coins.
fn coingecko_simple_price_url() -> String {
    let ids = COINS
        .iter()
        .map(|c| c.id)
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "https://api.coingecko.com/api/v3/simple/price\
         ?ids={ids}&vs_currencies=usd,pln&include_24hr_change=true"
    )
}
const APP_NAME: &str = "Crypto Tray";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

// --- localization (PL/EN, auto-detected from Windows UI language) ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Lang {
    Pl,
    #[default]
    En,
}

static LANG: OnceLock<Lang> = OnceLock::new();

fn lang() -> Lang {
    LANG.get().copied().unwrap_or_default()
}

/// Detect the Windows UI language. Returns Lang::Pl when the system's
/// primary UI language is Polish, Lang::En otherwise (Polish is the only
/// translated language so far; everything else falls back to English).
#[cfg(windows)]
fn detect_lang() -> Lang {
    use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
    const LANG_POLISH: u16 = 0x15; // PRIMARYLANGID for Polish
    let lcid = unsafe { GetUserDefaultUILanguage() };
    if (lcid & 0x3FF) == LANG_POLISH {
        Lang::Pl
    } else {
        Lang::En
    }
}
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

/// Curated list of supported cryptocurrencies. The first four (BTC, ETH, XMR,
/// KAS) are the application's default favourites; everything else can be
/// opted into via the picker window. Colours use brand identity where I knew
/// them; fallbacks are deliberately varied so the AA circle + letter icon
/// stays distinguishable when a real CoinGecko icon hasn't downloaded yet.
const COINS: &[CoinMeta] = &[
    // --- Default favourites ----------------------------------------------
    CoinMeta { id: "bitcoin",         ticker: "BTC",  name: "Bitcoin",          letter: "B", color_dark: 0x00_F7_93_1A, color_light: 0x00_C7_72_0A },
    CoinMeta { id: "ethereum",        ticker: "ETH",  name: "Ethereum",         letter: "E", color_dark: 0x00_62_7E_EA, color_light: 0x00_42_51_A0 },
    CoinMeta { id: "monero",          ticker: "XMR",  name: "Monero",           letter: "M", color_dark: 0x00_FF_66_00, color_light: 0x00_CC_55_00 },
    CoinMeta { id: "kaspa",           ticker: "KAS",  name: "Kaspa",            letter: "K", color_dark: 0x00_70_C7_BA, color_light: 0x00_2A_8A_7E },
    // --- Other supported coins (not enabled by default) ------------------
    CoinMeta { id: "tether",          ticker: "USDT", name: "Tether",           letter: "T", color_dark: 0x00_26_A1_7B, color_light: 0x00_1E_82_5F },
    CoinMeta { id: "binancecoin",     ticker: "BNB",  name: "BNB",              letter: "B", color_dark: 0x00_F3_BA_2F, color_light: 0x00_C2_94_21 },
    CoinMeta { id: "solana",          ticker: "SOL",  name: "Solana",           letter: "S", color_dark: 0x00_9B_4D_FC, color_light: 0x00_7A_3D_C9 },
    CoinMeta { id: "ripple",          ticker: "XRP",  name: "XRP",              letter: "X", color_dark: 0x00_22_99_CD, color_light: 0x00_1A_7A_A4 },
    CoinMeta { id: "usd-coin",        ticker: "USDC", name: "USD Coin",         letter: "U", color_dark: 0x00_27_77_C9, color_light: 0x00_1E_5F_A1 },
    CoinMeta { id: "cardano",         ticker: "ADA",  name: "Cardano",          letter: "A", color_dark: 0x00_0D_33_AB, color_light: 0x00_0A_27_88 },
    CoinMeta { id: "dogecoin",        ticker: "DOGE", name: "Dogecoin",         letter: "D", color_dark: 0x00_C2_A6_33, color_light: 0x00_9B_85_29 },
    CoinMeta { id: "tron",            ticker: "TRX",  name: "TRON",             letter: "T", color_dark: 0x00_EB_00_29, color_light: 0x00_BC_00_21 },
    CoinMeta { id: "avalanche-2",     ticker: "AVAX", name: "Avalanche",        letter: "A", color_dark: 0x00_E8_41_42, color_light: 0x00_BA_34_35 },
    CoinMeta { id: "chainlink",       ticker: "LINK", name: "Chainlink",        letter: "L", color_dark: 0x00_24_5A_E2, color_light: 0x00_1B_48_B5 },
    CoinMeta { id: "polkadot",        ticker: "DOT",  name: "Polkadot",         letter: "D", color_dark: 0x00_E6_00_7A, color_light: 0x00_B8_00_62 },
    CoinMeta { id: "litecoin",        ticker: "LTC",  name: "Litecoin",         letter: "L", color_dark: 0x00_BF_BB_BB, color_light: 0x00_88_88_88 },
    CoinMeta { id: "bitcoin-cash",    ticker: "BCH",  name: "Bitcoin Cash",     letter: "B", color_dark: 0x00_8D_C3_51, color_light: 0x00_70_9C_41 },
    CoinMeta { id: "internet-computer", ticker: "ICP", name: "Internet Computer", letter: "I", color_dark: 0x00_29_AB_E2, color_light: 0x00_21_88_B5 },
    CoinMeta { id: "near",            ticker: "NEAR", name: "NEAR Protocol",    letter: "N", color_dark: 0x00_42_85_F4, color_light: 0x00_34_6A_C3 },
    CoinMeta { id: "uniswap",         ticker: "UNI",  name: "Uniswap",          letter: "U", color_dark: 0x00_FF_00_7A, color_light: 0x00_CC_00_62 },
    CoinMeta { id: "ethereum-classic", ticker: "ETC", name: "Ethereum Classic", letter: "E", color_dark: 0x00_32_8E_30, color_light: 0x00_28_71_27 },
    CoinMeta { id: "stellar",         ticker: "XLM",  name: "Stellar",          letter: "X", color_dark: 0x00_47_4A_6B, color_light: 0x00_2A_2B_3C },
    CoinMeta { id: "cosmos",          ticker: "ATOM", name: "Cosmos",           letter: "A", color_dark: 0x00_6F_7A_F0, color_light: 0x00_4C_57_BD },
    CoinMeta { id: "filecoin",        ticker: "FIL",  name: "Filecoin",         letter: "F", color_dark: 0x00_00_90_FF, color_light: 0x00_00_73_CC },
    CoinMeta { id: "vechain",         ticker: "VET",  name: "VeChain",          letter: "V", color_dark: 0x00_15_BD_FF, color_light: 0x00_10_97_CC },
    CoinMeta { id: "the-graph",       ticker: "GRT",  name: "The Graph",        letter: "G", color_dark: 0x00_6F_47_FF, color_light: 0x00_5A_3A_D0 },
    CoinMeta { id: "aave",            ticker: "AAVE", name: "Aave",             letter: "A", color_dark: 0x00_B6_50_9E, color_light: 0x00_92_3F_7E },
    CoinMeta { id: "maker",           ticker: "MKR",  name: "Maker",            letter: "M", color_dark: 0x00_1A_AB_9B, color_light: 0x00_14_88_7C },
    CoinMeta { id: "algorand",        ticker: "ALGO", name: "Algorand",         letter: "A", color_dark: 0x00_70_75_8F, color_light: 0x00_4A_4F_6A },
    CoinMeta { id: "tezos",           ticker: "XTZ",  name: "Tezos",            letter: "T", color_dark: 0x00_2C_7D_F7, color_light: 0x00_22_64_C5 },
];

/// CoinGecko ids of the default-favourite coins (used when there's no
/// `coins.txt` config file yet, e.g. on first run).
const DEFAULT_FAVOURITES: &[&str] = &["bitcoin", "ethereum", "monero", "kaspa"];

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

/// CoinGecko `/simple/price` returns a JSON object keyed by coin id. Map
/// directly to a HashMap so we don't have to name every supported coin in
/// a struct.
type Prices = HashMap<String, CoinData>;

#[derive(Clone, Default)]
struct AppState {
    data: Arc<Mutex<Option<Result<Prices, String>>>>,
    last_update: Arc<Mutex<Option<Instant>>>,
}

fn fetch_price() -> Result<Prices, String> {
    let url = coingecko_simple_price_url();
    let resp = ureq::get(&url)
        .set("User-Agent", concat!("crypto-tray/", env!("CARGO_PKG_VERSION")))
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(15))
        .call()
        .map_err(|e| match lang() {
            Lang::Pl => format!("Błąd połączenia: {e}"),
            Lang::En => format!("Connection error: {e}"),
        })?;
    let parsed: Prices = resp.into_json().map_err(|e| match lang() {
        Lang::Pl => format!("Błąd parsowania JSON: {e}"),
        Lang::En => format!("JSON parse error: {e}"),
    })?;
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
    let ago = match lang() {
        Lang::Pl => "temu",
        Lang::En => "ago",
    };
    if secs < 60 {
        format!("{secs} s {ago}")
    } else if secs < 3600 {
        format!("{} min {} s {ago}", secs / 60, secs % 60)
    } else {
        format!("{} h {} min {ago}", secs / 3600, (secs % 3600) / 60)
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
        None => match lang() {
            Lang::Pl => "nigdy".to_string(),
            Lang::En => "never".to_string(),
        },
    };

    match data.as_ref() {
        Some(Ok(p)) => {
            let blocks: Vec<String> = COINS
                .iter()
                .filter(|c| enabled.contains(c.id))
                .filter_map(|c| p.get(c.id).map(|d| format_coin_block(c.name, c.ticker, d)))
                .collect();
            if blocks.is_empty() {
                match lang() {
                    Lang::Pl => {
                        "Brak wybranych kryptowalut.\nZaznacz przynajmniej jedną w menu."
                            .to_string()
                    }
                    Lang::En => {
                        "No cryptocurrencies selected.\nPick at least one from the menu."
                            .to_string()
                    }
                }
            } else {
                match lang() {
                    Lang::Pl => format!(
                        "Aktualne kursy kryptowalut:\n\n{}\n\n\
                         Ostatnia aktualizacja: {}\n\
                         Interwał odświeżania: {} s\n\
                         Źródło: CoinGecko",
                        blocks.join("\n\n"),
                        last_str,
                        interval_secs,
                    ),
                    Lang::En => format!(
                        "Current cryptocurrency prices:\n\n{}\n\n\
                         Last update: {}\n\
                         Refresh interval: {} s\n\
                         Source: CoinGecko",
                        blocks.join("\n\n"),
                        last_str,
                        interval_secs,
                    ),
                }
            }
        }
        Some(Err(e)) => match lang() {
            Lang::Pl => format!(
                "Nie udało się pobrać kursów.\n\n{e}\n\nOstatnia próba: {last_str}"
            ),
            Lang::En => format!(
                "Failed to fetch prices.\n\n{e}\n\nLast attempt: {last_str}"
            ),
        },
        None => match lang() {
            Lang::Pl => "Pobieranie kursów...\nSpróbuj ponownie za chwilę.".to_string(),
            Lang::En => "Loading prices...\nTry again shortly.".to_string(),
        },
    }
}

fn prices_title() -> String {
    match lang() {
        Lang::Pl => format!("{APP_NAME} – kursy"),
        Lang::En => format!("{APP_NAME} – prices"),
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
                match lang() {
                    Lang::Pl => format!("{APP_NAME} – brak wybranych monet"),
                    Lang::En => format!("{APP_NAME} – no coins selected"),
                }
            } else {
                lines.join("\n")
            }
        }
        Some(Err(_)) => match lang() {
            Lang::Pl => format!("{APP_NAME} – brak danych"),
            Lang::En => format!("{APP_NAME} – no data"),
        },
        None => match lang() {
            Lang::Pl => format!("{APP_NAME} – pobieranie..."),
            Lang::En => format!("{APP_NAME} – loading..."),
        },
    }
}

/// Tray icon — uses the same stock-chart design as the .exe icon.
/// PNG is bundled at compile time via `include_bytes!` so the running
/// binary is self-contained (no need to find icon.ico on disk).
fn create_tray_icon() -> tray_icon::Icon {
    const TRAY_PNG: &[u8] = include_bytes!("../assets/icon-tray.png");

    let (w, h, rgba) = decode_png_rgba(TRAY_PNG)
        .expect("bundled tray icon PNG should decode");
    tray_icon::Icon::from_rgba(rgba, w, h).expect("tray icon from rgba")
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
    // First-run default — only the four built-in favourites.
    DEFAULT_FAVOURITES.iter().map(|s| s.to_string()).collect()
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
    let ids = COINS
        .iter()
        .map(|c| c.id)
        .collect::<Vec<_>>()
        .join(",");
    let url = format!(
        "https://api.coingecko.com/api/v3/coins/markets?vs_currency=usd&ids={ids}"
    );
    let mut map = HashMap::new();
    if let Ok(resp) = ureq::get(&url)
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
    // One thread per coin. Layered stagger:
    //   - The four default favourites start almost immediately (0/200/400/600 ms)
    //     so the visible widget gets its sparklines populated within ~1 s,
    //     same as the previous 4-coin behaviour.
    //   - The remaining coins start at 30 s, 60 s, 90 s… so the initial burst
    //     stays under CoinGecko's per-minute rate cap and a freshly-enabled
    //     coin gets its sparkline within ~15 min at worst (often much sooner
    //     because its thread has already cycled past its initial wait).
    // After the initial wait each thread runs its own 15-min cycle and falls
    // back to a 30 s retry on failure (network blip / rate limit).
    let default_count = DEFAULT_FAVOURITES.len();
    for (idx, coin) in COINS.iter().enumerate() {
        let charts = charts.clone();
        let dirty = dirty.clone();
        let coin_id = coin.id.to_string();
        let initial_wait = if idx < default_count {
            Duration::from_millis((idx as u64) * 200)
        } else {
            Duration::from_secs(((idx - default_count) as u64 + 1) * 30)
        };
        std::thread::spawn(move || {
            std::thread::sleep(initial_wait);
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

// --- update check (GitHub Releases API) -------------------------------------

const GITHUB_REPO: &str = "twkrol/crypto-tray";

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
}

/// Compare two dotted version strings (e.g. "1.2.3" vs "1.2.10").
/// Returns Ordering between numeric components.
fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let parts_a: Vec<u32> = a.split('.').map(|s| s.parse().unwrap_or(0)).collect();
    let parts_b: Vec<u32> = b.split('.').map(|s| s.parse().unwrap_or(0)).collect();
    let n = parts_a.len().max(parts_b.len());
    for i in 0..n {
        let va = parts_a.get(i).copied().unwrap_or(0);
        let vb = parts_b.get(i).copied().unwrap_or(0);
        match va.cmp(&vb) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// Query the GitHub API for the latest release. Returns formatted message.
fn check_update() -> String {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );
    let resp = match ureq::get(&url)
        .set(
            "User-Agent",
            concat!("crypto-tray/", env!("CARGO_PKG_VERSION")),
        )
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(10))
        .call()
    {
        Ok(r) => r,
        Err(e) => {
            return match lang() {
                Lang::Pl => format!("Nie udało się sprawdzić aktualizacji.\n\n{e}"),
                Lang::En => format!("Failed to check for updates.\n\n{e}"),
            };
        }
    };
    let release: GitHubRelease = match resp.into_json() {
        Ok(r) => r,
        Err(e) => {
            return match lang() {
                Lang::Pl => format!("Nie udało się przetworzyć odpowiedzi GitHub.\n\n{e}"),
                Lang::En => format!("Failed to parse GitHub response.\n\n{e}"),
            };
        }
    };
    let latest = release.tag_name.trim_start_matches('v').to_string();
    let current = env!("CARGO_PKG_VERSION");
    match (version_cmp(&latest, current), lang()) {
        (std::cmp::Ordering::Greater, Lang::Pl) => format!(
            "Dostępna jest nowsza wersja!\n\n\
             Aktualna:   {current}\n\
             Najnowsza:  {latest}\n\n\
             Pobierz: {}",
            release.html_url
        ),
        (std::cmp::Ordering::Greater, Lang::En) => format!(
            "A newer version is available!\n\n\
             Current:  {current}\n\
             Latest:   {latest}\n\n\
             Download: {}",
            release.html_url
        ),
        (std::cmp::Ordering::Equal, Lang::Pl) => {
            format!("Masz najnowszą wersję ({current}).")
        }
        (std::cmp::Ordering::Equal, Lang::En) => {
            format!("You have the latest version ({current}).")
        }
        (std::cmp::Ordering::Less, Lang::Pl) => format!(
            "Twoja wersja ({current}) jest nowsza niż ostatni release ({latest})."
        ),
        (std::cmp::Ordering::Less, Lang::En) => format!(
            "Your version ({current}) is newer than the latest release ({latest})."
        ),
    }
}

/// Run the update check on a background thread and show the result via
/// MessageBox so the main event loop isn't blocked while we hit the API.
fn spawn_update_check() {
    std::thread::spawn(|| {
        let msg = check_update();
        let title = match lang() {
            Lang::Pl => format!("{APP_NAME} – aktualizacje"),
            Lang::En => format!("{APP_NAME} – updates"),
        };
        show_message(&title, &msg);
    });
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

    let exe = std::env::current_exe().map_err(|e| match lang() {
        Lang::Pl => format!("Brak ścieżki .exe: {e}"),
        Lang::En => format!("Cannot resolve .exe path: {e}"),
    })?;
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
            return Err(match lang() {
                Lang::Pl => format!("RegOpenKeyExW: błąd {r}"),
                Lang::En => format!("RegOpenKeyExW: error {r}"),
            });
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
            return Err(match lang() {
                Lang::Pl => format!("RegOpenKeyExW: błąd {r}"),
                Lang::En => format!("RegOpenKeyExW: error {r}"),
            });
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
    match (lang(), enabled) {
        (Lang::Pl, true) => "Usuń z autostartu",
        (Lang::Pl, false) => "Zainstaluj w autostarcie",
        (Lang::En, true) => "Remove from startup",
        (Lang::En, false) => "Add to startup",
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

// --- favourites picker window ------------------------------------------------

/// Layout constants for the picker window. Logical pixels (the window is
/// built with `LogicalSize` so values scale on hi-DPI monitors).
const PICKER_WIDTH: u32 = 400;
const PICKER_HEIGHT: u32 = 540;
const PICKER_HEADER_H: i32 = 44;
const PICKER_FOOTER_H: i32 = 56;
const PICKER_ROW_H: i32 = 30;
const PICKER_PAD_X: i32 = 14;
const PICKER_CHECKBOX: i32 = 18;
const PICKER_TICKER_W: i32 = 60;

/// Result of a picker open/close cycle, shared between the picker window and
/// the main event loop. When the user clicks "OK" the new HashSet lands in
/// the Mutex; the main loop polls it once per tick, applies it as the new
/// `enabled_coins`, and writes it to disk.
type PickerResult = Arc<Mutex<Option<HashSet<String>>>>;

struct Picker {
    window: Arc<tao::window::Window>,
    surface: softbuffer::Surface<Arc<tao::window::Window>, Arc<tao::window::Window>>,
    /// Draft selection — mutated by clicks before being committed on OK.
    selected: HashSet<String>,
    scroll: i32,
    hover_index: Option<usize>,
    visible: bool,
    pending_result: PickerResult,
}

impl Picker {
    fn new(
        event_loop: &tao::event_loop::EventLoop<()>,
        pending_result: PickerResult,
    ) -> Self {
        let title = match lang() {
            Lang::Pl => "Wybierz kryptowaluty",
            Lang::En => "Choose cryptocurrencies",
        };
        let window = Arc::new(
            WindowBuilder::new()
                .with_title(title)
                .with_inner_size(tao::dpi::LogicalSize::new(
                    PICKER_WIDTH as f64,
                    PICKER_HEIGHT as f64,
                ))
                .with_resizable(false)
                .with_visible(false)
                .with_skip_taskbar(false)
                .build(event_loop)
                .expect("failed to build picker window"),
        );
        let context =
            softbuffer::Context::new(window.clone()).expect("picker softbuffer ctx");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("picker surface");
        Picker {
            window,
            surface,
            selected: HashSet::new(),
            scroll: 0,
            hover_index: None,
            visible: false,
            pending_result,
        }
    }

    fn id(&self) -> tao::window::WindowId {
        self.window.id()
    }

    fn open(&mut self, current_enabled: &HashSet<String>) {
        self.selected = current_enabled.clone();
        self.scroll = 0;
        self.hover_index = None;
        self.window.set_visible(true);
        self.visible = true;
        // Pull to front in case it's already on screen behind something.
        let _ = self.window.set_focus();
        self.window.request_redraw();
    }

    fn close_save(&mut self) {
        if let Ok(mut slot) = self.pending_result.lock() {
            *slot = Some(self.selected.clone());
        }
        self.window.set_visible(false);
        self.visible = false;
    }

    fn close_cancel(&mut self) {
        // Just hide — discard the draft.
        self.window.set_visible(false);
        self.visible = false;
    }

    fn visible_rows(&self) -> i32 {
        let size = self.window.inner_size();
        let avail = size.height as i32 - PICKER_HEADER_H - PICKER_FOOTER_H;
        (avail / PICKER_ROW_H).max(1)
    }

    fn max_scroll(&self) -> i32 {
        let total = COINS.len() as i32;
        (total - self.visible_rows()).max(0)
    }

    fn hit_row(&self, _x: i32, y: i32) -> Option<usize> {
        if y < PICKER_HEADER_H {
            return None;
        }
        let size = self.window.inner_size();
        let list_bottom = size.height as i32 - PICKER_FOOTER_H;
        if y >= list_bottom {
            return None;
        }
        let row = ((y - PICKER_HEADER_H) / PICKER_ROW_H) as usize;
        let idx = row as i32 + self.scroll;
        if idx >= 0 && (idx as usize) < COINS.len() {
            Some(idx as usize)
        } else {
            None
        }
    }

    fn hit_ok(&self, x: i32, y: i32) -> bool {
        let size = self.window.inner_size();
        let w = size.width as i32;
        let h = size.height as i32;
        let btn_w = 100;
        let btn_h = 32;
        let btn_x = w - PICKER_PAD_X - btn_w;
        let btn_y = h - PICKER_FOOTER_H + (PICKER_FOOTER_H - btn_h) / 2;
        x >= btn_x && x < btn_x + btn_w && y >= btn_y && y < btn_y + btn_h
    }

    fn handle_click(&mut self, x: i32, y: i32) {
        if self.hit_ok(x, y) {
            self.close_save();
            return;
        }
        if let Some(idx) = self.hit_row(x, y) {
            let id = COINS[idx].id.to_string();
            if self.selected.contains(&id) {
                self.selected.remove(&id);
            } else {
                self.selected.insert(id);
            }
            self.window.request_redraw();
        }
    }

    fn handle_hover(&mut self, x: i32, y: i32) {
        let new_hover = self.hit_row(x, y);
        if new_hover != self.hover_index {
            self.hover_index = new_hover;
            self.window.request_redraw();
        }
    }

    fn handle_scroll(&mut self, delta_lines: i32) {
        let new = (self.scroll - delta_lines).clamp(0, self.max_scroll());
        if new != self.scroll {
            self.scroll = new;
            self.window.request_redraw();
        }
    }

    fn render(&mut self, theme: Theme, is_dark: bool, font_h: i32) {
        let size = self.window.inner_size();
        let w = size.width.max(1);
        let h = size.height.max(1);
        let (Some(nz_w), Some(nz_h)) = (NonZeroU32::new(w), NonZeroU32::new(h)) else {
            return;
        };
        if self.surface.resize(nz_w, nz_h).is_err() {
            return;
        }
        let Ok(mut buf) = self.surface.buffer_mut() else {
            return;
        };
        render_picker_dib(
            &mut buf,
            w,
            h,
            &self.selected,
            self.scroll,
            self.hover_index,
            theme,
            is_dark,
            font_h,
        );
        let _ = buf.present();
    }
}

#[cfg(windows)]
fn render_picker_dib(
    buffer: &mut [u32],
    w: u32,
    h: u32,
    selected: &HashSet<String>,
    scroll: i32,
    hover_index: Option<usize>,
    theme: Theme,
    is_dark: bool,
    font_h: i32,
) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
        CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
        DEFAULT_PITCH, DIB_RGB_COLORS, DeleteDC, DeleteObject, FF_SWISS, FW_BOLD, FW_NORMAL,
        FillRect, GetDC, GetTextMetricsW, OUT_DEFAULT_PRECIS, RGBQUAD, ReleaseDC, SelectObject,
        SetBkMode, TEXTMETRICW, TRANSPARENT,
    };

    let wu = w as usize;
    let hu = h as usize;
    let pixel_count = wu * hu;

    // Hover background: lighten by a hair, blends with theme.bg.
    let hover_bg = blend_rgb(theme.bg, theme.text, 0.08);
    let row_text = theme.text;
    let dim = theme.dim;
    let checked_fill = theme.up;
    let border = blend_rgb(theme.bg, theme.text, 0.35);

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
            buffer[..pixel_count].fill(theme.bg);
            return;
        }
        let old_dib = SelectObject(mem_dc, dib as _);

        // Background fill.
        let brush_bg = CreateSolidBrush(rgb_to_colorref(theme.bg));
        let full = RECT {
            left: 0,
            top: 0,
            right: w as i32,
            bottom: h as i32,
        };
        FillRect(mem_dc, &full, brush_bg);
        DeleteObject(brush_bg as _);

        // Hover row highlight (drawn BEFORE text so text sits on top).
        if let Some(idx) = hover_index {
            let row_idx_in_view = idx as i32 - scroll;
            if row_idx_in_view >= 0 {
                let y = PICKER_HEADER_H + row_idx_in_view * PICKER_ROW_H;
                if y + PICKER_ROW_H <= h as i32 - PICKER_FOOTER_H {
                    let hover_brush = CreateSolidBrush(rgb_to_colorref(hover_bg));
                    let r = RECT {
                        left: 0,
                        top: y,
                        right: w as i32,
                        bottom: y + PICKER_ROW_H,
                    };
                    FillRect(mem_dc, &r, hover_brush);
                    DeleteObject(hover_brush as _);
                }
            }
        }

        // Top header separator (1 px line).
        let sep_brush = CreateSolidBrush(rgb_to_colorref(border));
        let sep_top = RECT {
            left: 0,
            top: PICKER_HEADER_H - 1,
            right: w as i32,
            bottom: PICKER_HEADER_H,
        };
        FillRect(mem_dc, &sep_top, sep_brush);
        let sep_bot_y = h as i32 - PICKER_FOOTER_H;
        let sep_bot = RECT {
            left: 0,
            top: sep_bot_y,
            right: w as i32,
            bottom: sep_bot_y + 1,
        };
        FillRect(mem_dc, &sep_bot, sep_brush);
        DeleteObject(sep_brush as _);

        // OK button background.
        let btn_w = 100;
        let btn_h = 32;
        let btn_x = w as i32 - PICKER_PAD_X - btn_w;
        let btn_y = sep_bot_y + (PICKER_FOOTER_H - btn_h) / 2;
        let ok_brush = CreateSolidBrush(rgb_to_colorref(theme.up));
        let ok_rect = RECT {
            left: btn_x,
            top: btn_y,
            right: btn_x + btn_w,
            bottom: btn_y + btn_h,
        };
        FillRect(mem_dc, &ok_rect, ok_brush);
        DeleteObject(ok_brush as _);

        // Checkboxes: drawn as filled or empty squares. Drawn directly to DIB.
        let dib_mut =
            std::slice::from_raw_parts_mut(dib_bits as *mut u32, pixel_count);
        let visible_rows = ((h as i32 - PICKER_HEADER_H - PICKER_FOOTER_H) / PICKER_ROW_H).max(0);
        for row in 0..visible_rows {
            let coin_idx = scroll + row;
            if coin_idx < 0 || (coin_idx as usize) >= COINS.len() {
                break;
            }
            let coin = &COINS[coin_idx as usize];
            let y_row = PICKER_HEADER_H + row * PICKER_ROW_H;
            let cb_x = PICKER_PAD_X;
            let cb_y = y_row + (PICKER_ROW_H - PICKER_CHECKBOX) / 2;
            let checked = selected.contains(coin.id);

            // Border (1 px rect)
            for px in cb_x..(cb_x + PICKER_CHECKBOX) {
                for &py in &[cb_y, cb_y + PICKER_CHECKBOX - 1] {
                    if px >= 0 && (px as usize) < wu && py >= 0 && (py as usize) < hu {
                        dib_mut[py as usize * wu + px as usize] = border;
                    }
                }
            }
            for py in cb_y..(cb_y + PICKER_CHECKBOX) {
                for &px in &[cb_x, cb_x + PICKER_CHECKBOX - 1] {
                    if px >= 0 && (px as usize) < wu && py >= 0 && (py as usize) < hu {
                        dib_mut[py as usize * wu + px as usize] = border;
                    }
                }
            }
            if checked {
                // Fill interior with the brand colour of the coin (gives the
                // picker a little visual texture instead of all-green ticks).
                let fill = coin.color(is_dark);
                for py in (cb_y + 2)..(cb_y + PICKER_CHECKBOX - 2) {
                    for px in (cb_x + 2)..(cb_x + PICKER_CHECKBOX - 2) {
                        if px >= 0 && (px as usize) < wu && py >= 0 && (py as usize) < hu {
                            dib_mut[py as usize * wu + px as usize] = fill;
                        }
                    }
                }
                // Diagonal check mark in white over the brand colour.
                let cx0 = cb_x + 4;
                let cy0 = cb_y + PICKER_CHECKBOX / 2;
                let cx1 = cb_x + PICKER_CHECKBOX / 2 - 1;
                let cy1 = cb_y + PICKER_CHECKBOX - 5;
                let cx2 = cb_x + PICKER_CHECKBOX - 4;
                let cy2 = cb_y + 4;
                draw_line(dib_mut, wu, hu, cx0, cy0, cx1, cy1, 0x00_FF_FF_FF);
                draw_line(dib_mut, wu, hu, cx0, cy0 + 1, cx1, cy1 + 1, 0x00_FF_FF_FF);
                draw_line(dib_mut, wu, hu, cx1, cy1, cx2, cy2, 0x00_FF_FF_FF);
                draw_line(dib_mut, wu, hu, cx1, cy1 + 1, cx2, cy2 + 1, 0x00_FF_FF_FF);
            }
        }

        // Font for the body text (ticker + name).
        let face_w: Vec<u16> = OsStr::new(FONT_FACE)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let body_font = CreateFontW(
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
        let old_font = SelectObject(mem_dc, body_font as _);
        SetBkMode(mem_dc, TRANSPARENT as i32);

        let mut tm: TEXTMETRICW = std::mem::zeroed();
        GetTextMetricsW(mem_dc, &mut tm);

        // Header title.
        let header_font = CreateFontW(
            -((font_h * 12) / 10), // ~20% larger for header
            0,
            0,
            0,
            FW_BOLD as i32,
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
        let _prev = SelectObject(mem_dc, header_font as _);
        let mut tm_header: TEXTMETRICW = std::mem::zeroed();
        GetTextMetricsW(mem_dc, &mut tm_header);
        let header_text = match lang() {
            Lang::Pl => "Wybierz kryptowaluty",
            Lang::En => "Choose cryptocurrencies",
        };
        let header_y = (PICKER_HEADER_H - tm_header.tmHeight) / 2;
        draw_segment(mem_dc, PICKER_PAD_X, header_y, header_text, row_text);
        SelectObject(mem_dc, body_font as _);
        DeleteObject(header_font as _);

        // List rows: ticker + name. Checkboxes already drawn above.
        for row in 0..visible_rows {
            let coin_idx = scroll + row;
            if coin_idx < 0 || (coin_idx as usize) >= COINS.len() {
                break;
            }
            let coin = &COINS[coin_idx as usize];
            let y_row = PICKER_HEADER_H + row * PICKER_ROW_H;
            let text_y = y_row + (PICKER_ROW_H - tm.tmHeight) / 2;

            let ticker_x = PICKER_PAD_X + PICKER_CHECKBOX + 14;
            // Ticker in the coin's brand colour, so the row is visually tagged
            // even before icons are downloaded.
            draw_segment(mem_dc, ticker_x, text_y, coin.ticker, coin.color(is_dark));
            let name_x = ticker_x + PICKER_TICKER_W;
            draw_segment(mem_dc, name_x, text_y, coin.name, dim);
        }

        // OK button label.
        let ok_text = "OK";
        let mut ok_sz: windows_sys::Win32::Foundation::SIZE = std::mem::zeroed();
        let ok_text_w: Vec<u16> = ok_text.encode_utf16().collect();
        windows_sys::Win32::Graphics::Gdi::GetTextExtentPoint32W(
            mem_dc,
            ok_text_w.as_ptr(),
            ok_text_w.len() as i32,
            &mut ok_sz,
        );
        let ok_text_x = btn_x + (btn_w - ok_sz.cx) / 2;
        let ok_text_y = btn_y + (btn_h - ok_sz.cy) / 2;
        draw_segment(mem_dc, ok_text_x, ok_text_y, ok_text, 0x00_FF_FF_FF);

        // Hint near OK button: how many selected.
        let hint = match lang() {
            Lang::Pl => format!("Zaznaczone: {}", selected.len()),
            Lang::En => format!("Selected: {}", selected.len()),
        };
        let hint_y = btn_y + (btn_h - tm.tmHeight) / 2;
        draw_segment(mem_dc, PICKER_PAD_X, hint_y, &hint, dim);
        let _ = checked_fill; // reserved for future use (currently brand colour used instead)

        // Copy DIB → softbuffer.
        let dib_slice =
            std::slice::from_raw_parts(dib_bits as *const u32, pixel_count);
        if buffer.len() >= pixel_count {
            buffer[..pixel_count].copy_from_slice(dib_slice);
        }

        SelectObject(mem_dc, old_font);
        SelectObject(mem_dc, old_dib);
        DeleteObject(body_font as _);
        DeleteObject(dib as _);
        DeleteDC(mem_dc);
    }
}

// ----------------------------------------------------------------------------

fn main() {
    // Resolve UI language once at startup — every translatable string later
    // calls lang() and matches against the detected value.
    let _ = LANG.set(detect_lang());

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
    // Static menu labels, all translatable.
    let label_show = match lang() {
        Lang::Pl => "Pokaż kursy",
        Lang::En => "Show prices",
    };
    let label_refresh = match lang() {
        Lang::Pl => "Odśwież teraz",
        Lang::En => "Refresh now",
    };
    let label_charts = match lang() {
        Lang::Pl => "Wykresy",
        Lang::En => "Charts",
    };
    let label_refresh_icons = match lang() {
        Lang::Pl => "Odśwież ikony",
        Lang::En => "Refresh icons",
    };
    let label_check_update = match lang() {
        Lang::Pl => "Sprawdź aktualizacje",
        Lang::En => "Check for updates",
    };
    let label_about = match lang() {
        Lang::Pl => "O programie...",
        Lang::En => "About...",
    };
    let label_quit = match lang() {
        Lang::Pl => "Zakończ",
        Lang::En => "Quit",
    };

    let menu = Menu::new();
    let item_show = MenuItem::new(label_show, true, None);
    let item_refresh = MenuItem::new(label_refresh, true, None);
    let sep1 = PredefinedMenuItem::separator();

    let label_choose_coins = match lang() {
        Lang::Pl => "Wybierz kryptowaluty...",
        Lang::En => "Choose cryptocurrencies...",
    };
    let item_choose_coins = MenuItem::new(label_choose_coins, true, None);

    let sep2 = PredefinedMenuItem::separator();
    let item_charts = CheckMenuItem::new(label_charts, true, show_charts, None);
    let item_refresh_icons = MenuItem::new(label_refresh_icons, true, None);
    let item_check_update = MenuItem::new(label_check_update, true, None);
    let sep3 = PredefinedMenuItem::separator();
    let item_about = MenuItem::new(label_about, true, None);
    let item_autostart =
        MenuItem::new(autostart_label(is_autostart_enabled()), true, None);
    let sep4 = PredefinedMenuItem::separator();
    let item_quit = MenuItem::new(label_quit, true, None);

    menu.append_items(&[
        &item_show,
        &item_refresh,
        &sep1,
        &item_choose_coins,
        &sep2,
        &item_charts,
        &item_refresh_icons,
        &item_check_update,
        &sep3,
        &item_about,
        &item_autostart,
        &sep4,
        &item_quit,
    ])
    .expect("Failed to build menu");

    let id_show = item_show.id().clone();
    let id_refresh = item_refresh.id().clone();
    let id_choose_coins = item_choose_coins.id().clone();
    let id_charts = item_charts.id().clone();
    let id_refresh_icons = item_refresh_icons.id().clone();
    let id_check_update = item_check_update.id().clone();
    let id_about = item_about.id().clone();
    let id_autostart = item_autostart.id().clone();
    let id_quit = item_quit.id().clone();

    let menu_for_widget = menu.clone();

    let initial_tooltip = match lang() {
        Lang::Pl => format!("{APP_NAME} – pobieranie..."),
        Lang::En => format!("{APP_NAME} – loading..."),
    };
    let mut tray: Option<TrayIcon> = Some(
        TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(initial_tooltip)
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

    // Favourites picker — built but hidden; opened by the menu item.
    let picker_result: PickerResult = Arc::new(Mutex::new(None));
    let mut picker = Picker::new(&event_loop, picker_result.clone());
    let picker_id = picker.id();
    // Cursor position bookkeeping for picker click hit-testing (tao gives us
    // CursorMoved events with position, and MouseInput events without — we
    // remember the last cursor position to know where the click happened).
    let mut picker_cursor: PhysicalPosition<f64> = PhysicalPosition::new(0.0, 0.0);

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
                                    show_message(&prices_title(), &msg);
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
            // --- picker window events ----------------------------------------
            Event::WindowEvent {
                event: window_event,
                window_id,
                ..
            } if *window_id == picker_id => match window_event {
                WindowEvent::CursorMoved { position, .. } => {
                    picker_cursor = *position;
                    picker.handle_hover(position.x as i32, position.y as i32);
                }
                WindowEvent::MouseInput {
                    state: btn_state,
                    button: TaoMouseButton::Left,
                    ..
                } if *btn_state == ElementState::Pressed => {
                    picker.handle_click(
                        picker_cursor.x as i32,
                        picker_cursor.y as i32,
                    );
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    use tao::event::MouseScrollDelta;
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, dy) => *dy as i32,
                        MouseScrollDelta::PixelDelta(p) => (p.y as i32) / 28,
                        _ => 0,
                    };
                    if lines != 0 {
                        picker.handle_scroll(lines);
                    }
                }
                WindowEvent::CloseRequested => {
                    // Treat the X button as Cancel (don't commit selection).
                    picker.close_cancel();
                }
                _ => {}
            },
            Event::RedrawRequested(window_id) if *window_id == picker_id => {
                picker.render(theme, is_dark, font_h);
            }
            _ => {}
        }

        // Apply a picker save: replace enabled_coins, refresh widget + tray.
        if let Some(new_selection) = picker_result.lock().ok().and_then(|mut g| g.take())
        {
            enabled_coins = new_selection;
            save_enabled_coins(&enabled_coins);
            if let Some(t) = tray.as_ref() {
                let _ = t.set_tooltip(Some(format_tooltip(&state, &enabled_coins)));
            }
            // Resize widget for new coin count and redraw.
            let resize_after_picker = |new_width: u32| {
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
            resize_after_picker(compute_widget_width(enabled_coins.len(), show_charts));
            widget.request_redraw();
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

            if ev.id == id_show {
                let msg = format_price_message(&state, &enabled_coins, interval_secs);
                show_message(&prices_title(), &msg);
            } else if ev.id == id_refresh {
                let _ = refresh_tx.send(());
                show_message(
                    APP_NAME,
                    match lang() {
                        Lang::Pl => {
                            "Wymuszono odświeżenie kursów.\nNowe dane pojawią się za chwilę."
                        }
                        Lang::En => {
                            "Manual price refresh triggered.\nNew data will appear shortly."
                        }
                    },
                );
            } else if ev.id == id_choose_coins {
                picker.open(&enabled_coins);
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
                    &match lang() {
                        Lang::Pl => format!(
                            "Pobrano {} z {} ikon z CoinGecko.",
                            count,
                            COINS.len()
                        ),
                        Lang::En => format!(
                            "Downloaded {} of {} icons from CoinGecko.",
                            count,
                            COINS.len()
                        ),
                    },
                );
            } else if ev.id == id_check_update {
                spawn_update_check();
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
                            match (lang(), now_enabled) {
                                (Lang::Pl, true) => {
                                    "Aplikacja będzie uruchamiana przy starcie systemu."
                                }
                                (Lang::Pl, false) => {
                                    "Aplikacja została usunięta z autostartu."
                                }
                                (Lang::En, true) => {
                                    "The app will start automatically with Windows."
                                }
                                (Lang::En, false) => {
                                    "The app was removed from startup."
                                }
                            },
                        );
                    }
                    Err(e) => {
                        show_message(
                            APP_NAME,
                            &match lang() {
                                Lang::Pl => {
                                    format!("Nie udało się zmienić autostartu:\n\n{e}")
                                }
                                Lang::En => {
                                    format!("Failed to change startup setting:\n\n{e}")
                                }
                            },
                        );
                    }
                }
            } else if ev.id == id_about {
                let about = match lang() {
                    Lang::Pl => format!(
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
                    ),
                    Lang::En => format!(
                        "{APP_NAME}\n\
                         Version {APP_VERSION}\n\n\
                         A small app showing live cryptocurrency prices.\n\
                         Supported: BTC, ETH, XMR, KAS\n\
                         (pick which to show from the menu)\n\n\
                         • Floating widget — always visible above the taskbar\n\
                         • Left-click widget + drag — move it across the screen\n\
                         • Double-click widget — full details\n\
                         • Right-click widget — context menu\n\
                         • Tray icon — tooltip with prices, right-click for menu\n\
                         • Refresh interval: {interval_secs} s\n\n\
                         Data source: api.coingecko.com (public, free API)\n\
                         Written in Rust."
                    ),
                };
                let about_title = match lang() {
                    Lang::Pl => format!("O programie – {APP_NAME}"),
                    Lang::En => format!("About – {APP_NAME}"),
                };
                show_message(&about_title, &about);
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
                    show_message(&prices_title(), &msg);
                }
            }
        }
    });
}
