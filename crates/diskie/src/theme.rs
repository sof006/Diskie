//! Design tokens from the mock-up, dark and light. Colour only carries meaning: the UI is warm
//! neutral greys, file types get an OKLCH palette of even lightness, and amber marks focus and
//! selection.

use diskie_model::{KIND_COUNT, treemap::Palette};

#[derive(Clone, Copy, PartialEq)]
pub struct Theme {
    pub dark: bool,
    pub chrome: u32,
    pub chrome_line: u32,
    pub canvas: u32,
    pub panel: u32,
    pub line: u32,
    pub text: u32,
    pub muted: u32,
    pub faint: u32,
    pub map_bg: u32,
    pub groups: [u32; 3],
    pub head: u32,
    pub track: u32,
    pub btn_line: u32,
    /// 0xRRGGBBAA
    pub hover: u32,
    pub side_active: u32,
    pub danger: u32,
    pub accent: u32,
    /// 0xRRGGBBAA selection wash.
    pub selection: u32,
    pub ring: u32,
}

/// (L, C, h) per file type, as in the mock-up; the same in both themes.
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

impl Theme {
    pub const DARK: Theme = Theme {
        dark: true,
        chrome: 0x22201e,
        chrome_line: 0x302d2a,
        canvas: 0x1a1917,
        panel: 0x1d1c1a,
        line: 0x2c2a27,
        text: 0xebe7e1,
        muted: 0xa19b93,
        faint: 0x6f6a63,
        map_bg: 0x121110,
        groups: [0x2b2826, 0x252321, 0x201e1c],
        head: 0xb3aca3,
        track: 0x302d2a,
        btn_line: 0x3a3632,
        hover: 0xffffff0d,
        side_active: 0xffffff10,
        danger: 0xe9a08c,
        accent: 0xe3a64c,
        selection: 0xe3a64c29,
        ring: 0xf2b964,
    };

    pub const LIGHT: Theme = Theme {
        dark: false,
        chrome: 0xebe9e4,
        chrome_line: 0xd9d5cd,
        canvas: 0xfbfaf7,
        panel: 0xf5f4f0,
        line: 0xe5e2db,
        text: 0x1e1c19,
        muted: 0x5f5a53,
        faint: 0x8e887f,
        map_bg: 0xcfcac1,
        groups: [0xe3dfd8, 0xebe8e2, 0xf2f0eb],
        head: 0x4a453f,
        track: 0xe4e0d9,
        btn_line: 0xcfcac2,
        hover: 0x00000009,
        side_active: 0x0000000f,
        danger: 0x9e3b24,
        accent: 0x9a5d0a,
        selection: 0xb2701421,
        ring: 0x8a5208,
    };

    pub fn kind(&self, k: u8) -> u32 {
        kind_rgb(k)
    }

    /// Whether text on this file type's colour should be dark.
    pub fn kind_dark_ink(&self, k: u8) -> bool {
        KIND_OKLCH[k as usize % KIND_COUNT].0 >= 0.58
    }

    pub fn palette(&self) -> Palette {
        let mut kinds = [0u32; KIND_COUNT];
        for (i, c) in kinds.iter_mut().enumerate() {
            *c = kind_rgb(i as u8);
        }
        Palette { background: self.map_bg, groups: self.groups, kinds }
    }
}

pub fn kind_rgb(kind: u8) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; KIND_COUNT]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| KIND_OKLCH.map(|(l, c, h)| oklch_to_rgb(l, c, h)))[kind as usize % KIND_COUNT]
}

fn oklch_to_rgb(l: f32, c: f32, h_deg: f32) -> u32 {
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
