//! The look of the live window.
//!
//! `Theme::calm()` is the window's theme: quiet, one accent colour, one
//! typeface throughout (pipeline text included, no monospace), square
//! corners, and lines rather than filled areas to set things apart; status
//! in grey italic text. `Theme::neutral()` is the look of the recordings.

pub type Rgba = [f32; 4];

#[derive(Clone, Debug)]
pub struct Theme {
    pub background: Rgba,
    pub text: Rgba,
    pub muted: Rgba,
    /// Small grey headings and labels.
    pub kicker: Rgba,
    pub line: Rgba,
    pub accent: Rgba,
    /// Status, in text only.
    pub ok: Rgba,
    pub error: Rgba,
    /// Status text in grey italic, whatever it says (off: coloured).
    pub status_italic: bool,
    /// The overlay for stats for nerds.
    pub dark_background: Rgba,
    pub dark_text: Rgba,
    pub dark_muted: Rgba,
    pub dark_accent: Rgba,
    /// Typefaces in order of preference; the first one installed is used.
    pub fonts: &'static [&'static str],
    /// The typeface for pipeline text: `None` means the text typeface.
    pub code_font: Option<&'static str>,
    pub radius: f32,
    pub selection: Rgba,
    /// A tinted panel behind the controls, or none (a rule separates them).
    pub panel: Option<Rgba>,
    /// A soft glow around the focused field (off: the border says it).
    pub focus_glow: bool,
    /// Active buttons filled with the text colour (off: an accent border).
    pub filled_buttons: bool,
}

const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

impl Theme {
    pub fn calm() -> Self {
        let accent = rgb(0x00, 0x99, 0xCC);
        Theme {
            background: rgb(0xFF, 0xFF, 0xFF),
            text: rgb(0x00, 0x00, 0x00),
            muted: rgb(0x59, 0x59, 0x59),
            kicker: rgb(0x94, 0x94, 0x94),
            line: rgb(0xB8, 0xB8, 0xB8),
            accent,
            ok: rgb(0x59, 0x59, 0x59),
            error: rgb(0x59, 0x59, 0x59),
            status_italic: true,
            // Translucent, so the chart shows through stats for nerds.
            dark_background: [0.0, 0x25 as f32 / 255.0, 0x32 as f32 / 255.0, 0.72],
            dark_text: rgb(0xFF, 0xFF, 0xFF),
            dark_muted: rgb(0xB8, 0xB8, 0xB8),
            dark_accent: rgb(0x33, 0xB5, 0xE0),
            fonts: &["Segoe UI", "Helvetica Neue", "Arial", "sans-serif"],
            code_font: None,
            radius: 0.0,
            selection: [accent[0], accent[1], accent[2], 0.22],
            panel: None,
            focus_glow: false,
            filled_buttons: false,
        }
    }

    pub fn neutral() -> Self {
        Theme {
            background: [1.0; 4],
            text: lidar_common::INK,
            muted: lidar_common::MUTED,
            kicker: lidar_common::MUTED,
            line: [0.78, 0.80, 0.84, 1.0],
            accent: [0.20, 0.47, 0.95, 1.0],
            ok: [0.23, 0.55, 0.30, 1.0],
            error: [0.70, 0.10, 0.08, 1.0],
            status_italic: false,
            dark_background: [0.08, 0.09, 0.11, 0.93],
            dark_text: [0.92, 0.93, 0.95, 1.0],
            dark_muted: [0.62, 0.65, 0.70, 1.0],
            dark_accent: [0.55, 0.78, 1.0, 1.0],
            fonts: &["sans-serif"],
            code_font: Some("monospace"),
            radius: 6.0,
            selection: [0.74, 0.84, 1.0, 1.0],
            panel: Some([0.975, 0.978, 0.985, 1.0]),
            focus_glow: true,
            filled_buttons: true,
        }
    }
}
