-- weather_shark config
--
-- Where it lives: ~/.config/weather_shark/config.lua on Linux,
-- ~/Library/Application Support/weather_shark/ on macOS, %APPDATA%\weather_shark\ on
-- Windows (`weather_shark --help` prints the exact path). It's written on first run
-- and reloads automatically when you save. If it has an error, weather_shark keeps
-- the old config and shows the error.
-- In the app (and on the web), the `lua` button (or , key) opens this file in an editor.
--
-- Almost everything is set here: leave out anything you don't care about and the
-- default is used. Set a feature's `enabled = false` to remove it completely.
-- `platform` is "native" or "web" in case you want them to differ.
-- Colours are "#rrggbb" or "#rrggbbaa".

-- palette (matches tuke.dev) --------------------------------------------------

local col = {
    bg     = "#111113",
    card   = "#18181b",
    line   = "#26262b",
    fg     = "#e7e7ea",
    muted  = "#8e8e95",
    accent = "#a78bfa",
    red    = "#ff375f",
    orange = "#ff9f0a",
    yellow = "#ffd60a",
}

return {
    -- radar ---------------------------------------------------------------------

    site            = "KTLX",   -- radar to open on start (click any dot on the map to switch)
    live            = true,     -- follow the newest scan
    refresh_seconds = 60,       -- how often to look for a new scan / new alerts
    realtime        = true,     -- live: stream each scan as the radar sends it (seconds behind)
                                -- instead of waiting for the finished file (5-10 min behind)
    sweep_line      = true,     -- live: a rotating line over the tilt being scanned right now
    loop_minutes    = 15,       -- the loop (P) plays the scans of the last this-many minutes,
                                -- every SAILS rescan included (also picked next to "loop")
    loop_frame_ms   = 400,      -- how long each frame of the loop shows (400 = 2.5 frames a second)
    loop_hold_ms    = 1600,     -- how long the newest frame holds before the loop starts over
    loop_rock       = false,    -- true: play forward then backward instead of jumping to the start
    loop_rescans    = true,     -- false: one frame per scan, skipping the SAILS rescans in between
                                -- (all of these can be changed in the loop menu next to "loop")
    tilt            = 0.5,      -- elevation angle in degrees
    view_km         = 160,      -- how far around the radar to show on start (R resets to this)
    velocity_units  = "kt",     -- "kt", "mph", "kmh" or "ms": the units the VEL palette below is in

    -- panes to open with. Add more with the n key (or "panes" in the toolbar),
    -- close one with its x (or x over it). Each new pane splits the last one,
    -- like a tiling WM.
    -- products: REF (reflectivity), VEL (velocity), CC (correlation coeff),
    --           ZDR (differential reflectivity), SW (spectrum width), PHI
    panes           = { "REF" },

    layout = {
        mode  = "spiral",   -- "spiral" (fibonacci), "dwindle", "columns" or "grid"
        ratio = 0.5,        -- how much of the space each split gives the older pane
        gap   = 8,
    },

    smooth          = false,    -- true = blur between gates, false = crisp radar pixels
    image_size      = 2048,     -- radar image resolution per pane (bigger = sharper when zoomed in)

    -- look ----------------------------------------------------------------------

    theme = {
        bg        = col.bg,
        panel     = col.card,
        line      = col.line,
        fg        = col.fg,
        muted     = col.muted,
        accent    = col.accent,
        radius    = 10,         -- corner rounding of panes
        font_size = 13,
        syntax = {              -- colours in the built-in Lua editor
            keyword = col.accent,
            string  = "#9ece6a",
            number  = "#ff9e64",
            comment = col.muted,
            field   = "#7dcfff",
            punct   = col.muted,
        },
    },

    -- interface -----------------------------------------------------------------

    ui = {
        -- top bar buttons, left to right. Delete any you don't want; what they do
        -- is also in this file (and has a key below).
        --   "site"    radar picker               "time"   scan time, back / forward, live
        --   "tilt"    elevation angle            "panes"  + pane
        --   "map"     map layer switches         "events" bookmarks, go to a time, open a file
        --   "alerts"  warnings list              "radio"  NOAA Weather Radio
        --   "lua"     this file, in an editor
        toolbar   = { "site", "time", "tilt", "map", "events", "alerts", "radio", "lua" },
        header    = true,   -- product picker + title on each pane
        legend    = true,   -- colour scale
        readout   = true,   -- read the value under the cursor
        cursor_value = true, -- show it right next to the cursor in every pane
        speed_units  = "kt", -- speeds on screen: "kt", "mph", "kmh" or "ms" (also in the map
                             -- menu). Only the numbers change; colours follow velocity_units
        crosshair = true,   -- linked cursor across panes
        toasts    = true,   -- error messages
        panel_width = 320,  -- alerts / radio side panel (the Lua editor is 1.5× this)
    },

    -- keys: any egui key name ("N", "F2", "Space", "Comma", "ArrowLeft" ...),
    -- or false to unbind.
    keys = {
        prev_scan  = "ArrowLeft",
        next_scan  = "ArrowRight",
        tilt_up    = "ArrowUp",
        tilt_down  = "ArrowDown",
        add_pane   = "N",
        close_pane = "X",       -- the pane under the mouse
        live       = "L",
        recenter   = "R",
        roads      = "M",
        cities     = "C",
        warnings   = "W",
        watches    = "A",
        outlook    = "S",       -- SPC outlook on / off (pick the day and kind in "map")
        settings   = "Comma",   -- open this file in the editor
        open       = "O",       -- open a radar file (you can also drop one on the window)
        radio      = "Space",   -- play / stop the nearest NOAA Weather Radio stream
        play       = "P",       -- play / stop a loop of the last few scans
    },

    map = {
        states       = "#c9c9d299",
        counties     = "#8e8e9533",
        range_rings  = "#a78bfa30", -- every 50 km; set to false to hide
        sites        = col.accent,  -- radar site dots
        site_labels  = true,
        -- every layer can be turned off with false (states, counties, range_rings, roads ...)
        crosshair    = "#ffffffcc",
        warnings     = true,        -- draw warning polygons
        watches      = true,        -- draw tornado / severe thunderstorm watches (dashed counties)


        -- roads and towns come from OpenStreetMap vector tiles (OpenFreeMap).
        -- Any OpenMapTiles-schema TileJSON URL works here.
        tiles        = "https://tiles.openfreemap.org/planet",
        roads        = true,
        road_labels  = true,        -- I-35, US 77 ... shields
        cities       = true,
        city_color   = col.fg,
        road_colors  = {
            motorway  = "#f0b45acc",
            trunk     = "#c99a5e90",
            primary   = "#c9c9d299",
            secondary = "#a8a8b277",
            tertiary  = "#8e8e9566",
            minor     = "#8e8e9540", -- only drawn when zoomed in close
        },
    },

    -- SPC convective outlook ---------------------------------------------------
    -- The Storm Prediction Center's risk areas, drawn on the map. Pick the day and
    -- kind in the "map" menu, or turn it on / off with the outlook key (S).
    -- Refreshed every 10 minutes. It is hidden over archived scans from before it
    -- was issued, so it never shows today's risk over a past event.
    outlook = {
        show  = false,  -- on start: false, or "categorical", "tornado", "wind",
                        -- "hail" (days 1-2) or "severe" (days 3-8)
        day   = 1,      -- 1 to 8 (days 4-8 are only 15% / 30% severe areas)
        width = 1.6,    -- outline width; 0 = no outlines
        fill  = 0,      -- fill opacity 0..1 using SPC's colours (try 0.15); 0 = no fill
        fill_under_radar = true,  -- false draws the fill over the radar echoes
        labels     = true,        -- "ENH", "10%" ... on each area
        label_size = 11,

        -- per-level colours, overriding SPC's. Keys are the level names:
        --   categorical: TSTM MRGL SLGT ENH MDT HIGH
        --   probability: ["2%"] ["5%"] ["10%"] ["15%"] ["30%"] ["45%"] ["60%"]
        --   significant (hatched) area: SIG
        -- Each one can be
        --   "#rrggbb" or "#rrggbbaa"            outline colour (fill uses it too, at `fill` opacity)
        --   { line = "#..", fill = "#..aa" }    separate outline and fill; false for none
        --   false                               don't draw that level at all
        -- "#00000000" is fully transparent.
        colors = {
            -- TSTM = false,                                -- hide general thunder
            -- HIGH = { line = "#ff00ff", fill = "#ff00ff40" },
            -- ["10%"] = "#ffe066",
        },
    },

    -- NOAA Weather Radio -------------------------------------------------------
    -- Live streams from wxradio.org (volunteer-run, one stream at a time).
    -- They can lag 30 s or more: never rely on them instead of a real NWR receiver.
    radio = {
        enabled = true,     -- false removes the radio button, panel and key
        volume  = 0.8,
    },

    -- colour tables: { value, colour } stops, blended in between.
    -- step = true gives hard bands instead of blending. Values under the
    -- first stop are not drawn. mask_ref = 15 only draws gates where the
    -- reflectivity is at least 15 dBZ (good for cleaning up CC and ZDR).
    palettes = {
        REF = {
            { 5,  "#24304a" },
            { 15, "#3d6d8f" },
            { 20, "#2fae5f" },
            { 30, "#17772f" },
            { 35, "#e8d84a" },
            { 40, "#f2a33a" },
            { 45, "#f2672e" },
            { 50, "#d62d2d" },
            { 55, "#9c1238" },
            { 60, "#e04fd0" },
            { 65, "#a78bfa" },
            { 70, "#ffffff" },
        },
        VEL = { -- knots, negative = toward the radar
            range_folded = "#5b3a8a",
            { -90, "#e8fff3" },
            { -64, "#00ff7f" },
            { -40, "#00b84a" },
            { -20, "#127a38" },
            { -3,  "#273530" },
            { 0,   "#333336" },
            { 3,   "#3a2a2c" },
            { 20,  "#8a1818" },
            { 40,  "#c41e1e" },
            { 64,  "#ff4040" },
            { 90,  "#ffe3e3" },
        },
        CC = { -- low CC inside a velocity couplet = lofted debris (TDS)
            mask_ref = 15, -- only draw where reflectivity is at least this (hides clear-air noise)
            { 0.20, "#2b1a4a" },
            { 0.45, "#5b3fd6" },
            { 0.60, "#3f8ad6" },
            { 0.70, "#38d6c4" },
            { 0.80, "#4fd67a" },
            { 0.85, "#c9e04f" },
            { 0.90, "#f2c23a" },
            { 0.93, "#f27a2e" },
            { 0.96, "#d62d55" },
            { 0.98, "#b0205a" },
            { 1.00, "#c8509a" },
            { 1.05, "#ffffff" },
        },
        ZDR = {
            mask_ref = 15,
            { -4,  "#404048" },
            { 0,   "#8a8a9a" },
            { 0.5, "#4f7ad6" },
            { 1,   "#38b3d6" },
            { 2,   "#4fd67a" },
            { 3,   "#f2c23a" },
            { 4,   "#f27a2e" },
            { 5,   "#d62d2d" },
            { 6,   "#e04fd0" },
            { 8,   "#ffffff" },
        },
        SW = { -- knots
            { 0,  "#2a2a30" },
            { 5,  "#4a4a6a" },
            { 10, "#3f6fd6" },
            { 15, "#4fd67a" },
            { 20, "#f2c23a" },
            { 25, "#d62d2d" },
            { 30, "#ffffff" },
        },
        PHI = {
            { 0,   "#3f6fd6" },
            { 90,  "#4fd67a" },
            { 180, "#f2c23a" },
            { 270, "#d62d2d" },
            { 360, "#e04fd0" },
        },
    },

    -- alerts --------------------------------------------------------------------
    -- Active NWS alerts are checked every refresh_seconds. ONLY the events that
    -- have a rule below are fetched and shown, so this list is also your filter:
    -- delete the watch lines to only see warnings, or add
    --   { event = "Flash Flood Warning", sound = "none", color = "#4fd6ff" },
    -- to see flash floods too. Each alert uses the first rule that matches it.
    --
    -- rule fields (all optional except event):
    --   event    NWS event name, e.g. "Tornado Warning"
    --   tornado  "observed" (confirmed), "radar indicated", or "possible"
    --            (a severe thunderstorm warning that says a tornado is possible)
    --   damage   "considerable" (PDS), "catastrophic" (tornado / flash flood
    --            emergency) or "destructive" (severe thunderstorm)
    --   hail     minimum hail size in inches
    --   sound    "eas"    the TV / radio Emergency Alert System tone (853 + 960 Hz, 8 s)
    --            "wea"    the phone emergency alert (same tones, 2 s + 1 s + 1 s, twice)
    --            "siren"  an outdoor warning siren
    --            "alarm"  2 s of the EAS tone, "beep", "chime", or "none"
    --            Left out: "eas" for anything with a tornado, "beep" otherwise.
    --   file     a .wav file to play instead (native only)
    --   repeats  how many times to play the sound
    --   volume   0..1
    --   color    outline colour on the map and in the list
    --   map      false = don't draw it on the map
    --   list     false = don't show it in the alerts list
    --
    -- Storm warnings come with a storm-based polygon. Watches, winter and tropical
    -- alerts don't, so they are drawn as the outlines of the counties they cover
    -- (dashed). Marine alerts other than Special Marine Warnings have no outline,
    -- so they aren't listed here. Delete any line you don't want.

    alerts = {
        enabled    = true,  -- false turns off alerts completely (no polygons, list or sounds)
        sounds     = true,  -- false keeps the polygons and list but never plays a sound
        radius_km  = 300,   -- only alerts this close to the radar play a sound (0 = anywhere);
                            -- every alert in the country is drawn on the map and listed
        volume     = 0.8,
        on_startup = false, -- also sound alerts that were already active when the app opened
        rules = {
            { event = "Tornado Warning", damage = "catastrophic", sound = "eas",   repeats = 3, color = "#ff2dd4" },
            { event = "Tornado Warning", tornado = "observed",    sound = "eas",   repeats = 2, color = col.red },
            { event = "Tornado Warning",                          sound = "eas",   color = "#ff6b6b" },
            { event = "Extreme Wind Warning",                     sound = "eas",   repeats = 3, color = "#ff8c00" },
            { event = "Severe Thunderstorm Warning", damage = "destructive", sound = "alarm", color = "#ff5e3a" },
            { event = "Severe Thunderstorm Warning", tornado = "possible", sound = "eas", color = "#ffb347" },
            { event = "Severe Thunderstorm Warning", hail = 2.0,  sound = "beep",  color = col.orange },
            { event = "Severe Thunderstorm Warning",              sound = "chime", color = col.yellow },
            { event = "Flash Flood Warning", damage = "catastrophic", sound = "alarm", repeats = 2, color = "#3dffb0" },
            { event = "Flash Flood Warning",                      sound = "chime", color = "#2fc98a" },
            { event = "Snow Squall Warning",                      sound = "beep",  color = "#c77dff" },
            { event = "Dust Storm Warning",                       sound = "beep",  color = "#d2b48c" },
            { event = "Special Marine Warning",                   sound = "none",  color = "#ffa94d" },

            -- watches (dashed county outlines; the "watches" switch / A key)
            { event = "Tornado Watch",                            sound = "eas",   color = "#ff6b6b88" },
            { event = "Severe Thunderstorm Watch",                sound = "none",  color = "#ffd60a66" },
            { event = "Flash Flood Watch",                        sound = "none",  color = "#4fd67a66" },

            -- winter
            { event = "Blizzard Warning",                         sound = "beep",  color = "#e8f4ff" },
            { event = "Ice Storm Warning",                        sound = "beep",  color = "#a78bfa" },
            { event = "Winter Storm Warning",                     sound = "none",  color = "#ff69b4" },
            { event = "Lake Effect Snow Warning",                 sound = "none",  color = "#2dd4d4" },
            { event = "Extreme Cold Warning",                     sound = "none",  color = "#5aa9ff" },
            { event = "Winter Storm Watch",                       sound = "none",  color = "#7aa7d988" },
            { event = "Winter Weather Advisory",                  sound = "none",  color = "#9d8cf255", map = false },

            -- tropical
            { event = "Hurricane Warning",                        sound = "alarm", color = "#ff2d55" },
            { event = "Storm Surge Warning",                      sound = "alarm", color = "#b524f7" },
            { event = "Tropical Storm Warning",                   sound = "beep",  color = "#e0565b" },
            { event = "Hurricane Watch",                          sound = "none",  color = "#ff2dd488" },
            { event = "Tropical Storm Watch",                     sound = "none",  color = "#f0808088" },

            -- other
            { event = "High Wind Warning",                        sound = "none",  color = "#daa520" },
            { event = "Red Flag Warning",                         sound = "none",  color = "#ff1493", map = false },
            { event = "Extreme Heat Warning",                     sound = "none",  color = "#c71585", map = false },
            { event = "Tsunami Warning",                          sound = "eas",   color = "#fd6347" },
        },
    },

    -- past events (in the "events" menu) -----------------------------------------

    bookmarks = {
        { name = "Moore EF5",        site = "KTLX", time = "2013-05-20 20:08" },
        { name = "El Reno",          site = "KTLX", time = "2013-05-31 23:10" },
        { name = "Joplin",           site = "KSGF", time = "2011-05-22 22:40" },
        { name = "Mayfield",         site = "KPAH", time = "2021-12-11 03:30" },
        { name = "Rolling Fork",     site = "KDGX", time = "2023-03-25 01:05" },
    },
}
