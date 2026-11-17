//! The design-token palette from the mock-up, converted from OKLCH to sRGB.

pub const KIND_COUNT: usize = 11;

pub const KIND_NAMES: [&str; KIND_COUNT] = [
    "Video", "Images", "Audio", "Archives", "Disk images", "Apps", "Documents", "Code", "Caches",
    "System", "Other",
];

pub const EXT: [&str; KIND_COUNT] = [
    "mkv", "jpg", "flac", "zip", "vhdx", "dll", "pdf", "rs", "tmp", "sys", "dat",
];

/// (L, C, h) per kind, matching the mock-up's tokens.
const KIND_OKLCH: [(f32, f32, f32); KIND_COUNT] = [
    (0.64, 0.11, 38.0),
    (0.76, 0.09, 150.0),
    (0.62, 0.10, 305.0),
    (0.82, 0.10, 85.0),
    (0.60, 0.08, 245.0),
    (0.71, 0.08, 195.0),
    (0.86, 0.04, 70.0),
    (0.67, 0.10, 125.0),
    (0.63, 0.035, 265.0),
    (0.46, 0.02, 265.0),
    (0.39, 0.01, 265.0),
];

/// Dark-theme tokens (0xRRGGBB).
pub mod dark {
    pub const CANVAS: u32 = 0x1a1917;
    pub const CHROME: u32 = 0x22201e;
    pub const LINE: u32 = 0x2c2a27;
    pub const TEXT: u32 = 0xebe7e1;
    pub const MUTED: u32 = 0xa19b93;
    pub const MAP_BG: u32 = 0x121110;
    pub const GROUPS: [u32; 3] = [0x2b2826, 0x252321, 0x201e1c];
    pub const HEAD: u32 = 0xb3aca3;
    pub const TRACK: u32 = 0x302d2a;
    pub const ACCENT: u32 = 0xe3a64c;
    pub const SELECTION: u32 = 0x3a3022;
}

pub fn kind_rgb(kind: u8) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; KIND_COUNT]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| KIND_OKLCH.map(|(l, c, h)| oklch_to_rgb(l, c, h)))[kind as usize % KIND_COUNT]
}

/// Whether text on this kind's fill should be dark (true) or light.
pub fn kind_dark_ink(kind: u8) -> bool {
    KIND_OKLCH[kind as usize % KIND_COUNT].0 >= 0.58
}

pub fn oklch_to_rgb(l: f32, c: f32, h_deg: f32) -> u32 {
    let h = h_deg.to_radians();
    let (a, b) = (c * h.cos(), c * h.sin());
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3;
    let bl = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;
    let enc = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        let v = if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
        (v * 255.0).round() as u32
    };
    (enc(r) << 16) | (enc(g) << 8) | enc(bl)
}

/// Split 0xRRGGBB into 0..=1 floats.
pub fn rgb_f32(c: u32) -> [f32; 3] {
    [
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
    ]
}
