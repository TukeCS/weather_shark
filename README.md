# weather_shark

A dark, minimal radar viewer for tracking tornadoes, written in Rust with egui.
It reads raw NEXRAD Level II data and shows products side by side in synced
panes (same view, same crosshair), so a velocity couplet lines up with the
hook echo and any debris signature in CC.

Everything that's a matter of taste lives in a Lua file: colours, colour
tables, panes, alert rules and sounds.

**[Live demo](https://tuke.gg/demos/weather_shark/)** (the same code compiled to WebAssembly)

![weather_shark showing the 2013 Moore tornado supercell over Oklahoma City, with interstates, highway numbers and town names](screenshots/moore.webp)

## Why

i got tired of paying subscriptions

## Features

- Real data from NOAA's public NEXRAD archive on AWS (live, or any past scan back to the 2000s)
- Products: reflectivity, velocity, correlation coefficient, ZDR, spectrum width, PHI
- Starts with one pane; each pane you add splits the last one in a fibonacci spiral (like dwm/trwm),
  up to 6. Every pane shares pan/zoom and a crosshair that reads out the value under it
- A chase map: roads down to street level, interstate and US highway numbers, and town names from
  OpenStreetMap, plus state and county lines. Every layer can be toggled
- Click any radar site on the map to switch to it, or search by name
- Live NWS warnings drawn as their storm polygons; watches drawn as the counties they cover.
  Each kind can play its own sound (a siren for a confirmed tornado by default)
- NOAA Weather Radio: live streams from the transmitters nearest your radar
- Bookmarks for past events (Moore, El Reno, Joplin, Mayfield, Rolling Fork)
- Lua config that hot-reloads on save, with an in-app editor

## Running

```sh
cargo run --release                               # live, starting at the site in your config
cargo run --release -- KTLX "2013-05-20 20:08"    # a past scan (UTC)
```

On Linux, sound goes through PulseAudio/PipeWire when it's running (falling back to ALSA), and
building needs the `libasound` headers. To check your speakers:

```sh
weather_shark test-sound siren      # or alarm, beep, chime
weather_shark test-sound radio      # 15 s of the Oklahoma City NOAA Weather Radio stream
```

### Keys

| key | |
|---|---|
| drag / scroll | pan / zoom (double-click zooms in) |
| ← → | previous / next scan |
| ↑ ↓ | tilt |
| n | add a pane |
| x | close the pane under the mouse |
| m / c | toggle roads / city names |
| L | follow live |
| R | recentre on the radar (zoom from `view_km`) |

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

Layout and map layers:

```lua
panes  = { "REF" },                         -- what opens on start
layout = { mode = "spiral", ratio = 0.5 },  -- spiral, dwindle, columns, grid
map    = { roads = true, cities = true, road_labels = true },
```

Built-in sounds are `siren`, `alarm` (EAS-style two-tone), `beep` and `chime`. `file` plays a
.wav file next to the config (native only). The global `platform` is `"native"` or `"web"` if you
want the two to differ. See [`assets/default_config.lua`](assets/default_config.lua) for every option.

## NOAA Weather Radio

The `radio` panel lists live NOAA Weather Radio streams sorted by distance from the radar you're
viewing. Click one to listen. The streams are run by volunteers at [wxradio.org](https://wxradio.org)
and matched to transmitter locations from the NWS station list. Their terms allow direct listening
but not relaying, and they block clients that open several streams at once. So weather_shark only
ever plays one stream, straight from their server.

Streams can lag 30 seconds or more, so don't rely on them for warnings: use a real NWR receiver.

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
- `mvt.rs`, `tiles.rs`: a small vector-tile decoder and tile cache for roads and places. While sharper tiles load, already-loaded parent tiles fill in.
- `app/layout.rs`: pane tiling (spiral, dwindle, columns, grid). Panes animate between layouts.
- `radio.rs`: wxradio.org stream list plus playback, with rodio on native and an `<audio>` element on the web.
- `assets/`: state and county outlines (US Census 2010, 1:20m) packed as delta-encoded polylines with FIPS codes (`tools/geo.py`), the WSR-88D site list and the NWR transmitter list.

Map data © [OpenStreetMap](https://www.openstreetmap.org/copyright) contributors, served by [OpenFreeMap](https://openfreemap.org).
