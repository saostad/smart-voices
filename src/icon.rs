//! Tray icon drawn in code (no image files): a 32×32 RGBA speaker with
//! sound waves.
//!
//! The glyph is white with alpha: on macOS the icon is installed as a template
//! image so the system recolors it for light/dark menu bars; on Windows/Linux
//! the white glyph suits the usual dark trays.

/// Icon edge length in pixels.
pub const SIZE: u32 = 32;

/// RGBA bytes for the speaker icon.
pub fn speaker() -> Vec<u8> {
    draw()
}

/// RGBA bytes for the reading icon: a speaker plus `waves` sound arcs
/// (1–3; values above 3 are treated as 3, 0 draws no arcs).
pub fn speaking(waves: u8) -> Vec<u8> {
    let waves = waves.min(3);
    let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
    let center = 15.5f64;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let alpha = speaking_alpha(x, y, center, waves);
            let i = ((y * SIZE + x) * 4) as usize;
            pixels[i] = 255;
            pixels[i + 1] = 255;
            pixels[i + 2] = 255;
            pixels[i + 3] = alpha;
        }
    }
    pixels
}

/// Anti-aliased coverage (0.0–1.0) of an axis-aligned box, with a 1 px
/// feather on every edge.
fn box_coverage(x: f64, y: f64, x0: f64, x1: f64, y0: f64, y1: f64) -> f64 {
    let dx = (x0 - x).max(x - x1);
    let dy = (y0 - y).max(y - y1);
    let sd = if dx > 0.0 && dy > 0.0 {
        dx.hypot(dy)
    } else {
        dx.max(dy)
    };
    (0.5 - sd).clamp(0.0, 1.0)
}

/// Coverage (0–255) of the reading glyph at a pixel.
fn speaking_alpha(x: u32, y: u32, center: f64, waves: u8) -> u8 {
    let (x, y) = (f64::from(x), f64::from(y));
    let ady = (y - center).abs();
    // Speaker body: bar on the left, overlapping the cone base.
    let body = box_coverage(x, y, 3.0, 8.5, center - 3.5, center + 3.5);
    // Speaker cone: widening from x 8 (half-height 3.5) to x 14
    // (half-height 8); the slanted edge is divided by the line normal
    // (hypot(1, 0.75) = 1.25) for a true distance.
    let half = 3.5 + (x - 8.0) * (4.5 / 6.0);
    let cone = (8.0 - x).max(x - 14.0).max((ady - half) / 1.25);
    let cone = (0.5 - cone).clamp(0.0, 1.0);
    // Sound arcs opening right, centered on the cone tip, ±50° from
    // horizontal: radii 4.5, 9, 13.5 with thickness 1.6 and a 1 px feather.
    let dx = x - 14.0;
    let dist = dx.hypot(ady);
    let angle = ady.atan2(dx).to_degrees();
    let mut arcs = 0.0f64;
    for i in 0..waves {
        let radius = 4.5 + 4.5 * f64::from(i);
        let radial = (1.3 - (dist - radius).abs()).clamp(0.0, 1.0);
        let angular = ((52.5 - angle) / 5.0).clamp(0.0, 1.0);
        arcs = arcs.max(radial * angular);
    }
    let coverage = body.max(cone).max(arcs);
    (coverage * 255.0).round() as u8
}

fn draw() -> Vec<u8> {
    let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
    let center = 15.5f64;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let alpha = glyph_alpha(x, y, center);
            let i = ((y * SIZE + x) * 4) as usize;
            pixels[i] = 255;
            pixels[i + 1] = 255;
            pixels[i + 2] = 255;
            pixels[i + 3] = alpha;
        }
    }
    pixels
}

/// Coverage (0–255) of the glyph at a pixel.
fn glyph_alpha(x: u32, y: u32, center: f64) -> u8 {
    let (x, y) = (f64::from(x), f64::from(y));
    // Speaker body: vertical bar on the left.
    let in_body = (4.0..10.0).contains(&x) && (y - center).abs() <= 3.5;
    // Speaker cone: triangle widening to the right.
    let in_cone =
        (10.0..17.0).contains(&x) && (y - center).abs() <= 3.0 + (x - 10.0) * (4.5 / 7.0) + 0.5;
    // Sound waves: two arcs opening to the right, centered on the cone tip.
    let dx = x - 11.0;
    let dy = y - center;
    let dist = dx.hypot(dy);
    let in_arc = dx > 1.0
        && dy.abs() <= dx * 1.3 + 1.0
        && ((dist - 5.5).abs() <= 1.2 || (dist - 10.0).abs() <= 1.2);
    u8::from(in_body || in_cone || in_arc) * 255
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(pixels: &[u8], x: u32, y: u32) -> u8 {
        pixels[((y * SIZE + x) * 4 + 3) as usize]
    }

    #[test]
    fn icon_has_the_right_size() {
        assert_eq!(speaker().len(), (SIZE * SIZE * 4) as usize);
    }

    #[test]
    fn speaker_body_and_waves_are_opaque() {
        let icon = speaker();
        assert_eq!(alpha_at(&icon, 7, 16), 255, "speaker body");
        assert_eq!(alpha_at(&icon, 14, 16), 255, "speaker cone");
        assert_eq!(alpha_at(&icon, 17, 16), 255, "inner wave");
        assert_eq!(alpha_at(&icon, 21, 16), 255, "outer wave");
        assert_eq!(alpha_at(&icon, 0, 0), 0, "corner stays transparent");
    }

    #[test]
    fn icon_is_vertically_symmetric() {
        let icon = speaker();
        for y in 0..SIZE / 2 {
            for x in 0..SIZE {
                assert_eq!(
                    alpha_at(&icon, x, y),
                    alpha_at(&icon, x, SIZE - 1 - y),
                    "pixel ({x}, {y})"
                );
            }
        }
    }

    /// Render an icon as ASCII (`#` for alpha ≥ 128), for visual review.
    fn ascii_art(pixels: &[u8]) -> String {
        let mut out = String::new();
        for y in 0..SIZE {
            for x in 0..SIZE {
                out.push(if alpha_at(pixels, x, y) >= 128 {
                    '#'
                } else {
                    '.'
                });
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn speaking_icon_has_the_right_size() {
        assert_eq!(speaking(3).len(), (SIZE * SIZE * 4) as usize);
    }

    #[test]
    fn speaking_frames_show_one_two_three_arcs() {
        // Arc peaks on the center row (radii 4.5, 9, 13.5 from x = 14).
        let one = speaking(1);
        assert!(alpha_at(&one, 18, 16) >= 128, "inner arc present");
        assert_eq!(alpha_at(&one, 23, 16), 0, "middle arc absent");
        assert_eq!(alpha_at(&one, 27, 16), 0, "outer arc absent");
        let two = speaking(2);
        assert!(alpha_at(&two, 18, 16) >= 128, "inner arc present");
        assert!(alpha_at(&two, 23, 16) >= 128, "middle arc present");
        assert_eq!(alpha_at(&two, 27, 16), 0, "outer arc absent");
        let three = speaking(3);
        assert!(alpha_at(&three, 18, 16) >= 128, "inner arc present");
        assert!(alpha_at(&three, 23, 16) >= 128, "middle arc present");
        assert!(alpha_at(&three, 27, 16) >= 128, "outer arc present");
    }

    #[test]
    fn speaking_arcs_keep_transparent_gaps() {
        let three = speaking(3);
        assert_eq!(alpha_at(&three, 16, 16), 0, "gap between cone and arc 1");
        assert_eq!(alpha_at(&three, 21, 16), 0, "gap between arcs 1 and 2");
        assert_eq!(alpha_at(&three, 25, 16), 0, "gap between arcs 2 and 3");
    }

    #[test]
    fn speaking_icon_is_vertically_symmetric() {
        for waves in 0..=3 {
            let icon = speaking(waves);
            for y in 0..SIZE / 2 {
                for x in 0..SIZE {
                    assert_eq!(
                        alpha_at(&icon, x, y),
                        alpha_at(&icon, x, SIZE - 1 - y),
                        "waves {waves} pixel ({x}, {y})"
                    );
                }
            }
        }
    }

    #[test]
    fn speaking_clamps_out_of_range_wave_counts() {
        assert_eq!(speaking(0).len(), (SIZE * SIZE * 4) as usize);
        assert_eq!(speaking(9), speaking(3));
    }

    #[test]
    fn print_speaking_ascii_for_review() {
        println!("speaking(3):\n{}", ascii_art(&speaking(3)));
    }
}
