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
//!
//! ## Memory strategy
//! egui software-rasterizes glyphs itself (ab_glyph), so it must have the raw
//! font bytes — we can't defer to a system text API. But a CJK font is often
//! tens of MB (Microsoft YaHei's `msyh.ttc` is ~40MB), and reading the whole
//! file into a heap `Vec` just to touch a few hundred glyphs is wasteful.
//! Instead we **memory-map** the font file: the kernel maps it into our
//! address space and pages glyphs in on demand, so only the glyphs we actually
//! render touch RSS. The mapping is leaked to live for the whole process (the
//! font is needed for the app's lifetime anyway). If mmap is unavailable for
//! some reason we fall back to a plain read.

use egui::{FontData, FontDefinitions, FontFamily, FontTweak};
use memmap2::{Mmap, MmapOptions};
use std::path::PathBuf;

/// The logical font name we register our CJK face under, so we can push it
/// onto both the Proportional and Monospace fallback lists.
const CJK_NAME: &str = "pyman_cjk";

/// Install our CJK fallback font into the given context. Idempotent and
/// failure-tolerant: if no system font is found it logs a note and returns.
pub fn install(ctx: &egui::Context) {
    // Prefer a memory-mapped font (zero-copy from the page cache). If mmap
    // fails for this path we transparently fall back to reading the file,
    // so the only consequence is higher memory use, not a broken UI.
    let data = match load_cjk_font_mmap() {
        Some(bytes) => FontData::from_static(bytes),
        None => match load_cjk_font_read() {
            Some(bytes) => FontData::from_owned(bytes),
            None => {
                eprintln!("[pyman] no CJK system font found; Chinese may render as boxes");
                return;
            }
        },
    };

    let mut fonts = FontDefinitions::default();

    // FontTweak with a small y offset to nudge CJK glyphs onto the Latin
    // baseline; otherwise CJK fonts often sit a touch high. We use the same
    // bytes for both families (egui keeps an Arc internally).
    let data = data.tweak(FontTweak {
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

/// Locate the first available CJK system font and **memory-map** it, returning
/// a `&'static [u8]` view over the mapping.
///
/// Returns `None` if no candidate font exists on disk, or if the file cannot
/// be opened/mapped (in which case the caller falls back to a plain read).
fn load_cjk_font_mmap() -> Option<&'static [u8]> {
    for candidate in candidate_paths() {
        let file = match std::fs::File::open(&candidate) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        // Sanity floor: a real CJK font is megabytes; a tiny/zero-byte file
        // means the font isn't really present.
        if len <= 100_000 {
            continue;
        }
        // SAFETY: we map the file read-only. The only soundness hazard with a
        // memmap is another process mutating the underlying file while we hold
        // the mapping (the bytes could change under us). System font files in
        // the OS font directory are effectively immutable for the lifetime of
        // a running app, and we treat the bytes as read-only glyph data fed to
        // ab_glyph, so even a pathological change could at worst corrupt a
        // rendered glyph — never memory-unsafe. This matches how eframe itself
        // maps image/icon assets.
        let mmap = match unsafe { MmapOptions::new().map(&file) } {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "[pyman] mmap failed for {}, will try read: {e}",
                    candidate.display()
                );
                continue;
            }
        };
        eprintln!("[pyman] mapped CJK font: {}", candidate.display());
        // Leak the mapping so it lives for the whole process: the font must
        // outlive the egui context (which holds it for the app's lifetime),
        // and we never want to unmap it. `Box::leak` returns a `&'static Mmap`;
        // we then deref that to a `&'static [u8]` for egui's `from_static`.
        // There is nothing to free and no destructor we care to run.
        let leaked: &'static Mmap = Box::leak(Box::new(mmap));
        // `&'static Mmap` -> `&'static [u8]` via Deref. The mapping (and thus
        // the slice) is valid for the program's lifetime, so the static
        // lifetime on the slice is sound.
        return Some(&**leaked);
    }
    None
}

/// Fallback: read the first available CJK system font into a heap `Vec`. Used
/// only when mmap is unavailable. Same selection/ordering as the mmap path.
fn load_cjk_font_read() -> Option<Vec<u8>> {
    for candidate in candidate_paths() {
        match std::fs::read(&candidate) {
            Ok(bytes) if bytes.len() > 100_000 => {
                eprintln!("[pyman] loaded CJK font (read): {}", candidate.display());
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
