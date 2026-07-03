//! The PyMan logo / app icon: a gear (⚙) with a play triangle (▶) in its
//! center — "manage" + "run". Generated procedurally so the binary needs no
//! bundled image asset and the icon is always crisp at any size.
//!
//! We render into a fixed-size RGBA grid (LOGO_SIZE) of "distance field"-ish
//! samples: for each pixel we test whether it's inside the gear ring / teeth
//! or inside the play triangle, then color accordingly. The same pixel buffer
//! backs both the window icon (`icon_data`) and the in-UI logo texture.

use egui::{Color32, ColorImage, IconData, TextureHandle};

/// Edge length of the generated icon, in pixels. 64 is plenty for a window
/// icon and a small heading logo; egui scales it for display.
pub const LOGO_SIZE: usize = 64;

/// Build the RGBA pixel buffer once.
fn render() -> Vec<Color32> {
    let n = LOGO_SIZE;
    let mut px = vec![Color32::TRANSPARENT; n * n];

    // Geometry, in a unit square [0, n). Center and radii tuned by eye.
    let cx = n as f32 * 0.5;
    let cy = n as f32 * 0.5;
    let outer_r = n as f32 * 0.46; // gear tip radius (incl. teeth)
    let body_r = n as f32 * 0.34; // gear body (ring outer) radius
    let inner_r = n as f32 * 0.20; // ring inner radius (the hole)
    let teeth = 8.0;

    // Colors.
    let gear = Color32::from_rgb(96, 165, 250); // blue-400
    let gear_dark = Color32::from_rgb(59, 130, 246); // blue-500 (tooth edges)
    let play = Color32::from_rgb(34, 197, 94); // green-500

    // Play triangle: tip pointing right, centered. Vertices in pixel space.
    let tri_h = n as f32 * 0.20; // half-height of the triangle's base
    let tri_left = cx - n as f32 * 0.10;
    let tri_right = cx + n as f32 * 0.16;

    for y in 0..n {
        for x in 0..n {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let dx = fx - cx;
            let dy = fy - cy;
            let dist = (dx * dx + dy * dy).sqrt();

            // Tooth modulation: angle-based. A tooth exists when the angle
            // falls in the outer band; between teeth the gear edge is body_r.
            let ang = dy.atan2(dx); // -pi..pi
            let tooth_phase = (ang * teeth).sin(); // -1..1, 8 humps over full circle
            let edge_r = body_r + (outer_r - body_r) * (tooth_phase * 0.5 + 0.5).clamp(0.0, 1.0);

            // Gear ring pixel: within edge_r but outside the inner hole.
            let is_gear = dist <= edge_r && dist >= inner_r;
            // Use a darker shade on the tooth tips for a subtle bevel.
            let gear_color = if dist > body_r { gear_dark } else { gear };

            // Play triangle test via barycentric-style edge functions.
            let in_tri = point_in_triangle(
                (fx, fy),
                (tri_left, cy - tri_h), // top-left
                (tri_left, cy + tri_h), // bottom-left
                (tri_right, cy),        // right tip
            );

            // Draw order: gear ring first, play triangle on top (so the
            // triangle's color wins where they overlap inside the hole).
            if is_gear {
                px[y * n + x] = gear_color;
            }
            if in_tri {
                px[y * n + x] = play;
            }
        }
    }

    px
}

/// Signed-area test for a point inside a triangle (counterclockwise or
/// clockwise vertices both handled via consistent <= / >= comparison).
fn point_in_triangle(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let (px, py) = p;
    let d1 = sign(px, py, a, b);
    let d2 = sign(px, py, b, c);
    let d3 = sign(px, py, c, a);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

fn sign(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    (px - b.0) * (a.1 - b.1) - (a.0 - b.0) * (py - b.1)
}

/// Window / taskbar icon data for `ViewportBuilder::with_icon`.
pub fn icon_data() -> IconData {
    let pixels = render();
    let mut rgba = Vec::with_capacity(pixels.len() * 4);
    for c in pixels {
        rgba.extend_from_slice(&c.to_array()); // [r, g, b, a]
    }
    IconData {
        rgba,
        width: LOGO_SIZE as u32,
        height: LOGO_SIZE as u32,
    }
}

/// Upload the logo as a texture for drawing in the UI. Caller caches the
/// returned handle for the app's lifetime.
pub fn logo_texture(ctx: &egui::Context) -> TextureHandle {
    let pixels = render();
    let image = ColorImage {
        size: [LOGO_SIZE, LOGO_SIZE],
        pixels,
    };
    ctx.load_texture("pyman-logo", image, Default::default())
}
