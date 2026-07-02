//! Font setup: egui's default fonts have no CJK glyphs, so Chinese text shows
//! as "tofu" boxes. We install a CJK-capable system font as a fallback so the
//! UI renders Chinese (and Latin) correctly, without bloating the binary.
//!
//! We try a list of well-known system font files and load the first one that
//! exists. If none is found we leave egui's defaults in place (ASCII still
//! works; CJK falls back to tofu) — the UI remains usable, just not localized.
//!
//! Candidate order favors clean, widely-bundled CJK fonts. Each path is
//! checked under the OS font directory.

use egui::{FontData, FontDefinitions, FontFamily, FontTweak};
use std::path::PathBuf;

/// The logical font name we register our CJK face under, so we can push it
/// onto both the Proportional and Monospace fallback lists.
const CJK_NAME: &str = "pyman_cjk";

/// Install our CJK fallback font into the given context. Idempotent and
/// failure-tolerant: if no system font is found it logs a note and returns.
pub fn install(ctx: &egui::Context) {
    let Some(bytes) = load_cjk_font() else {
        eprintln!("[pyman] no CJK system font found; Chinese may render as boxes");
        return;
    };

    let mut fonts = FontDefinitions::default();

    // FontTweak with a small y offset to nudge CJK glyphs onto the Latin
    // baseline; otherwise CJK fonts often sit a touch high. We use the same
    // bytes for both families (egui keeps an Arc internally).
    let data = FontData::from_owned(bytes).tweak(FontTweak {
        y_offset_factor: -0.06,
        ..Default::default()
    });

    fonts
        .font_data
        .insert(CJK_NAME.to_owned(), data);

    // Put our CJK face at the END of both fallback chains so Latin glyphs
    // still prefer egui's built-in fonts, and CJK only kicks in for code
    // points the Latin fonts lack. This keeps ASCII crisp.
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push(CJK_NAME.to_owned());
    }

    ctx.set_fonts(fonts);
}

/// Locate and read the first available CJK system font. Returns the raw TTF/TTC
/// bytes on success.
fn load_cjk_font() -> Option<Vec<u8>> {
    for candidate in candidate_paths() {
        match std::fs::read(&candidate) {
            Ok(bytes) if bytes.len() > 100_000 => {
                // Sanity floor: a real CJK font is megabytes; a 0-byte or tiny
                // stub file means the font isn't really present.
                eprintln!("[pyman] loaded CJK font: {}", candidate.display());
                return Some(bytes);
            }
            Ok(_) => eprintln!("[pyman] skipping tiny/truncated font: {}", candidate.display()),
            Err(_) => continue,
        }
    }
    None
}

/// Build a list of candidate font paths across Windows / macOS / Linux.
fn candidate_paths() -> Vec<PathBuf> {
    let names = [
        "msyh.ttc",      // Windows: Microsoft YaHei (TTC, index 0 = regular)
        "msyh.ttf",
        "simhei.ttf",    // Windows: SimHei (single TTF, very reliable)
        "simsun.ttc",    // Windows: SimSun
        "Deng.ttf",      // Windows: DengXian
        "SourceHanSansSC-Regular.otf", // Adobe Source Han Sans (win/macos/linux)
        "NotoSansCJK-Regular.ttc",     // Linux: Noto CJK
        "NotoSansSC-Regular.otf",
        "PingFang.ttc",  // macOS: PingFang
        "STHeiti Medium.ttc",          // macOS
    ];

    let dirs: Vec<PathBuf> = [
        // Windows
        std::env::var_os("WINDIR")
            .map(|w| PathBuf::from(w).join("Fonts")),
        // Linux
        Some(PathBuf::from("/usr/share/fonts")),
        Some(PathBuf::from("/usr/local/share/fonts")),
        dirs_from_env("XDG_DATA_HOME", "fonts"),
        // macOS
        home_fonts("Library/Fonts"),
        home_fonts(".fonts"),
    ]
    .into_iter()
    .flatten()
    .collect();

    let mut out = Vec::new();
    for dir in &dirs {
        for name in &names {
            out.push(dir.join(name));
        }
    }
    out
}

fn dirs_from_env(env: &str, sub: &str) -> Option<PathBuf> {
    std::env::var_os(env).map(|v| PathBuf::from(v).join(sub))
}

fn home_fonts(sub: &str) -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(sub))
}
