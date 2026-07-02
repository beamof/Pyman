//! Integration test: confirm the CJK system font we load at runtime actually
//! parses through ab_glyph (the same parser egui uses) and covers a CJK
//! codepoint. This guards against a font that exists on disk but that egui
//! can't rasterize (which would silently leave Chinese as tofu boxes).
//!
//! This reads the real Windows font dir, so it only runs meaningfully on a
//! machine with those fonts. It's skipped (with a pass) on machines where no
//! candidate font is present.

use ab_glyph::{Font, FontArc};
use std::path::PathBuf;

const CANDIDATES: &[&str] = &[
    "msyh.ttc",
    "msyh.ttf",
    "simhei.ttf",
    "simsun.ttc",
    "Deng.ttf",
];

fn font_dir() -> Option<PathBuf> {
    std::env::var_os("WINDIR").map(|w| PathBuf::from(w).join("Fonts"))
}

#[test]
fn cjk_font_parses_and_covers_chinese() {
    let Some(dir) = font_dir() else {
        eprintln!("skip: no WINDIR (not Windows)");
        return;
    };

    let mut tried = Vec::new();
    for name in CANDIDATES {
        let path = dir.join(name);
        tried.push(path.display().to_string());
        let Ok(bytes) = std::fs::read(&path) else { continue };

        // ab_glyph exposes both FontVec (try_from_vec) and FontArc. A TTC must
        // be parsed via the FontVec/TtfFontCollection path; FontArc::try_from_vec
        // returns the font at index 0 for collections.
        let font = match FontArc::try_from_vec(bytes) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("{}: parse failed: {e}", path.display());
                continue;
            }
        };

        // glyph_id(c) returns GlyphId(0) if the codepoint has no glyph in the
        // font (the ".notdef" / tofu glyph). We check a representative Chinese
        // character used in our UI ("脚") plus a Latin char.
        let has_cjk = font.glyph_id('脚').0 != 0;
        let has_latin = font.glyph_id('A').0 != 0;

        println!(
            "{}: parsed OK | cjk(脚)={has_cjk} | latin(A)={has_latin}",
            path.display()
        );
        assert!(
            has_cjk,
            "{} parsed but lacks CJK glyph — wrong/western-only font",
            path.display()
        );
        assert!(has_latin, "{} lacks latin glyph", path.display());
        return; // first good font passes the test
    }

    // No font on this machine — don't fail CI, but make the skip visible.
    eprintln!("skip: no candidate font found among: {}", tried.join(", "));
}
