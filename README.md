# weather_shark

A dark, minimal radar viewer for tracking tornadoes, written in Rust with egui.
It reads raw NEXRAD Level II data and shows products side by side in synced
panes (same view, same crosshair), so a velocity couplet lines up with the
hook echo and any debris signature in CC.

Everything that's a matter of taste lives in a Lua file: colours, colour
tables, panes, alert rules and sounds.

**[Live demo](https://tuke.gg/demos/weather_shark/)** (the same code compiled to WebAssembly)

## Features

- Real data from NOAA's public NEXRAD archive on AWS (live, or any past scan back to the 2000s)
- Products: reflectivity, velocity, correlation coefficient, ZDR, spectrum width, PHI
- 1–4 split panes with shared pan/zoom and a crosshair that reads out the value under it in every pane
- Click any radar site on the map to switch to it, or search by name
- US state and county outlines
- Live NWS warnings drawn on the map, with a sound for each kind (a siren for a confirmed tornado by default)
- Bookmarks for past events (Moore, El Reno, Joplin, Mayfield, Rolling Fork)
- Lua config that hot-reloads on save, with an in-app editor

## Running

```sh
cargo run --release                               # live, starting at the site in your config
cargo run --release -- KTLX "2013-05-20 20:08"    # a past scan (UTC)
```

On Linux, sound needs ALSA (`libasound` headers to build).

### Keys

| key | |
|---|---|
| drag / scroll | pan / zoom (double-click zooms in) |
| ← → | previous / next scan |
| ↑ ↓ | tilt |
| 1–4 | number of panes |
| L | follow live |
| R | recentre on the radar |

## Config

The config lives at `~/.config/weather_shark/config.lua` and is written on first run. You can also
get a fresh copy with `weather_shark default-config`. It reloads when you save it.
`weather_shark check` validates it without opening a window.

The script returns a table. Anything you leave out falls back to the default, so a config can be
as small as:

```lua
return {
    site  = "KINX",
    panes = { "REF", "VEL" },
    theme = { accent = "#ff6b6b" },
}
```

Alert rules are checked in order and the first match wins:

```lua
alerts = {
    radius_km = 300,
    rules = {
        { event = "Tornado Warning", damage = "catastrophic", sound = "siren", repeats = 5 },
        { event = "Tornado Warning", tornado = "observed",    sound = "siren", repeats = 3 },
        { event = "Tornado Warning",                          sound = "alarm" },
        { event = "Severe Thunderstorm Warning", hail = 2.0,  sound = "beep"  },
        { event = "Severe Thunderstorm Warning",              file  = "storm.wav" },
    },
},
```

Built-in sounds are `siren`, `alarm` (EAS-style two-tone), `beep` and `chime`. `file` plays a
.wav file next to the config (native only). The global `platform` is `"native"` or `"web"` if you
want the two to differ. See [`assets/default_config.lua`](assets/default_config.lua) for every option.

## Web build

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130   # match Cargo.lock
./web/build.sh [output dir]                         # defaults to ../portfolio/demos/weather_shark
```

On the web, the config is edited in the app (the `lua` button) and kept in browser storage.
Opening the page with `#KTLX 2013-05-20 20:08` in the URL starts on that scan.

## How it works

- `level2.rs`: Archive II decoder. It reads the volume header, unpacks the bzip2 LDM records and parses message 31 radials into sweeps.
- `render.rs`: rasterizes a sweep to a texture once, using an azimuth lookup table and a colour lookup per raw byte. Pan and zoom are then just GPU work.
- `config.rs`: runs the Lua in [piccolo](https://github.com/kyren/piccolo), a Lua VM written in pure Rust. That's what lets the config work in WebAssembly too.
- `alerts.rs`, `sound.rs`: NWS alert matching. Sounds are synthesized in Rust and played with rodio on native and Web Audio in the browser.
- `assets/`: state and county outlines (US Census 2010, 1:20m), packed as delta-encoded polylines, plus the WSR-88D site list.
