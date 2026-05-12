# Crypto Tray

Mała aplikacja na Windows pokazująca aktualne kursy kryptowalut (BTC, ETH, XMR, KAS) jako pływający widżet stale widoczny nad paskiem zadań, plus ikonę w trayu.

Napisana w Rust, kompiluje się do natywnego x86-64 .exe (~2 MB).

## Funkcje

- Pływający widżet z kursem USD i zmianą 24h dla wybranych monet
- Mini-wykres cen z ostatniej godziny (sparkline) przy każdej monecie
- Ikony krypto pobierane z CoinGecko (cache na dysku, można podmienić własnymi)
- Ikona w trayu z tooltipem zawierającym kursy i menu kontekstowym
- Automatyczna detekcja motywu Windows (jasny/ciemny) — widżet dopasowuje tło
- Auto-pozycjonowanie nad paskiem zadań, dynamiczna szerokość zależna od liczby włączonych monet
- Przeciągany myszką (lewy klik + drag)
- Opcjonalna instalacja w autostarcie systemu

## Wymagania

- Windows 10 lub 11 (x86-64)
- Do zbudowania: Rust toolchain (stable)

## Build

```bash
cargo build --release
```

Wynik: `target/release/crypto-tray.exe`.

## Uruchomienie

```bash
crypto-tray.exe          # domyślny interwał odświeżania ceny (60 s)
crypto-tray.exe 30       # odświeżanie co 30 s (minimum 5 s)
```

Po uruchomieniu na ekranie pojawi się widżet umieszczony domyślnie po prawej stronie paska zadań, plus ikona w systemowym trayu.

## Interakcja

| Akcja | Efekt |
|---|---|
| Lewy klik widżetu + przeciągnięcie | zmiana pozycji na ekranie |
| Podwójny klik widżetu | popup z pełnymi szczegółami (USD i PLN, 24h zmiana) |
| Prawy klik widżetu | menu kontekstowe (te same opcje co w trayu) |
| Lewy klik ikony w trayu | popup z pełnymi szczegółami |
| Najechanie na ikonę w trayu | tooltip z aktualnymi kursami |
| Prawy klik ikony w trayu | menu (toggle monet, wykresy, autostart, odśwież ikony, info) |

## Konfiguracja użytkownika

Ustawienia zapisują się w `%APPDATA%\CryptoTray\`:

- `coins.txt` — lista włączonych monet (po jednym CoinGecko id w linii)
- `charts_off` — flaga, obecność pliku = wykresy wyłączone
- `icons/<id>.png` — cache ikon. Możesz wrzucić tu własny PNG (kwadratowy, transparent bg) o nazwie `bitcoin.png`, `ethereum.png`, `monero.png` lub `kaspa.png`, żeby zastąpić wersję pobraną z CoinGecko.

Autostart: wpis pod `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\CryptoTray`. Włączany/wyłączany z menu (nie wymaga praw administratora).

## Źródła danych

[CoinGecko Public API v3](https://www.coingecko.com/en/api) — free tier, bez klucza:

- `/simple/price` — bieżące kursy (USD, PLN, 24h zmiana)
- `/coins/markets` — URL-e do logo monet
- `/coins/{id}/market_chart/range` — historia cen z ostatniej godziny do sparkline'a

Logo monet są własnością ich projektów.

## Stack

- [`tao`](https://crates.io/crates/tao) — okna i pętla zdarzeń
- [`tray-icon`](https://crates.io/crates/tray-icon) + [`muda`](https://crates.io/crates/muda) — ikona w trayu i menu
- [`softbuffer`](https://crates.io/crates/softbuffer) — bufor pikseli do okna widżetu
- [`png`](https://crates.io/crates/png) — dekodowanie pobranych ikon
- [`ureq`](https://crates.io/crates/ureq) — synchroniczne HTTP/JSON (rustls)
- [`windows-sys`](https://crates.io/crates/windows-sys) — Win32 API (GDI do renderowania tekstu Segoe UI, rejestr, taskbar, SetWindowPos)

## Licencja

[MIT](LICENSE)
