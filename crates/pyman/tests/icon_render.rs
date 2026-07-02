//! Verifies the procedurally-generated PyMan icon is a sensible gear+play
//! image: not blank, has a blue gear ring around the outside, a green play
//! triangle in the middle, and transparency in the corners.
//!
//! Also writes a PNG next to the test output so a human can eyeball it.

use pyman::icon;

#[test]
fn icon_renders_gear_and_play_triangle() {
    let data = icon::icon_data();
    assert_eq!(data.width as usize, icon::LOGO_SIZE);
    assert_eq!(data.height as usize, icon::LOGO_SIZE);
    assert_eq!(data.rgba.len(), icon::LOGO_SIZE * icon::LOGO_SIZE * 4);

    let n = icon::LOGO_SIZE;
    // Decode into Color32-ish (r,g,b,a) tuples for inspection.
    let px = |x: usize, y: usize| {
        let i = (y * n + x) * 4;
        (data.rgba[i], data.rgba[i + 1], data.rgba[i + 2], data.rgba[i + 3])
    };

    // 1) Corners should be transparent (the gear is a circle, not a square).
    for (x, y) in [(0, 0), (n - 1, 0), (0, n - 1), (n - 1, n - 1)] {
        let (_, _, _, a) = px(x, y);
        assert_eq!(a, 0, "corner ({x},{y}) should be transparent, alpha={a}");
    }

    // 2) A point on the gear body ring should be blue-ish. Sample several
    //    mid-radius points around the center; at least one must be blue+opaque
    //    (the ring is continuous at mid-radius even between teeth).
    let cx = n as f32 * 0.5;
    let cy = n as f32 * 0.5;
    let mid_r = n as f32 * 0.27; // between inner_r(0.20) and body_r(0.34)
    let mut found_blue = false;
    for k in 0..24 {
        let a = (k as f32) * std::f32::consts::TAU / 24.0;
        let gx = (cx + mid_r * a.cos()) as usize;
        let gy = (cy + mid_r * a.sin()) as usize;
        let p = px(gx.min(n - 1), gy.min(n - 1));
        if p.3 > 0 && p.2 > p.0 && p.2 > p.1 {
            found_blue = true;
            break;
        }
    }
    assert!(found_blue, "expected at least one blue gear-ring pixel");

    // 3) The center should be green (the play triangle covers the middle).
    let center = px(n / 2, n / 2);
    assert!(
        center.1 > center.0 && center.1 >= center.2,
        "center should be green (play triangle), got {:?}",
        center
    );

    // 4) Overall, the icon must have a meaningful amount of non-transparent
    //    pixels — guards against an all-transparent render.
    let opaque = data
        .rgba
        .chunks_exact(4)
        .filter(|c| c[3] > 0)
        .count();
    let total = n * n;
    let ratio = opaque as f32 / total as f32;
    assert!(
        ratio > 0.3 && ratio < 0.9,
        "opaque pixel ratio {ratio:.2} is out of the expected 0.3..0.9 band"
    );

    // Dump a PNG for manual review (path printed to stdout).
    let out = std::env::temp_dir().join("pyman_logo_test.png");
    image::save_buffer(
        &out,
        &data.rgba,
        data.width,
        data.height,
        image::ExtendedColorType::Rgba8,
    )
    .expect("write png");
    eprintln!("icon PNG written to: {}", out.display());
    println!("icon PNG written to: {}", out.display());
}
