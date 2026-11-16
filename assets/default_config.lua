-- weather_shark config
--
-- Native: ~/.config/weather_shark/config.lua (`weather_shark default-config > ~/.config/weather_shark/config.lua`).
-- It reloads automatically when you save. If it has an error, weather_shark keeps
-- the old config and shows the error in the top bar.
-- Web: open the settings panel (the ⚙ button) and edit it there. It is stored in your browser.
--
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
    tilt            = 0.5,      -- elevation angle in degrees
    view_km         = 160,      -- how far around the radar to show on start (R resets to this)
    velocity_units  = "kt",     -- "kt", "mph" or "ms" (the VEL palette below uses these units)

    -- panes to open with. Add more with the + button (or n), close one with
    -- its x (or x over it). Each new pane splits the last one, like a tiling WM.
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
    },

    map = {
        states       = "#c9c9d299",
        counties     = "#8e8e9533",
        range_rings  = "#a78bfa30", -- every 50 km; set to false to hide
        sites        = col.accent,  -- radar site dots
        site_labels  = true,
        crosshair    = "#ffffffcc",
        warnings     = true,        -- draw warning polygons / watch counties

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

    -- NOAA Weather Radio -------------------------------------------------------
    -- Live streams from wxradio.org (volunteer-run, one stream at a time).
    -- They can lag 30 s or more: never rely on them instead of a real NWR receiver.
    radio = {
        enabled = true,
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
    -- Active NWS warnings are checked every refresh_seconds. Each new alert
    -- uses the first rule that matches it.
    --
    -- rule fields (all optional except event):
    --   event    NWS event name, e.g. "Tornado Warning"
    --   tornado  "observed" (confirmed) or "radar indicated"
    --   damage   "considerable" (PDS) or "catastrophic" (tornado emergency)
    --   hail     minimum hail size in inches
    --   sound    "siren", "alarm", "beep", "chime" or "none"
    --   file     a .wav file to play instead (native only)
    --   repeats  how many times to play the sound
    --   volume   0..1
    --   color    outline colour on the map and in the list
    --
    -- Warnings come with a storm-based polygon. Watches don't, so they are
    -- drawn as the outlines of the counties they cover (dashed).

    alerts = {
        enabled    = true,
        radius_km  = 300,   -- only alerts this close to the radar (0 = whole country)
        volume     = 0.8,
        on_startup = false, -- also sound alerts that were already active when the app opened
        rules = {
            { event = "Tornado Warning", damage = "catastrophic", sound = "siren", repeats = 5, color = "#ff2dd4" },
            { event = "Tornado Warning", tornado = "observed",    sound = "siren", repeats = 3, color = col.red },
            { event = "Tornado Warning",                          sound = "alarm", repeats = 2, color = "#ff6b6b" },
            { event = "Severe Thunderstorm Warning", hail = 2.0,  sound = "beep",  color = col.orange },
            { event = "Severe Thunderstorm Warning",              sound = "chime", color = col.yellow },
            { event = "Tornado Watch",                            sound = "beep",  color = "#ff6b6b88" },
            { event = "Severe Thunderstorm Watch",                sound = "none",  color = "#ffd60a66" },
        },
    },

    -- past events (click one in the ★ menu to load it) --------------------------

    bookmarks = {
        { name = "Moore EF5",        site = "KTLX", time = "2013-05-20 20:08" },
        { name = "El Reno",          site = "KTLX", time = "2013-05-31 23:10" },
        { name = "Joplin",           site = "KSGF", time = "2011-05-22 22:40" },
        { name = "Mayfield",         site = "KPAH", time = "2021-12-11 03:30" },
        { name = "Rolling Fork",     site = "KDGX", time = "2023-03-25 01:05" },
    },
}
