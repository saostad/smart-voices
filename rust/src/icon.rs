//! Tray icons drawn in code (no image files): a 32×32 RGBA speaker with
//! sound waves, plus a variant with a "new copy" badge dot.
//!
//! The glyph is white with alpha: on macOS the icon is installed as a template
//! image so the system recolors it for light/dark menu bars; on Windows/Linux
//! the white glyph suits the usual dark trays.

/// Icon edge length in pixels.
pub const SIZE: u32 = 32;

/// RGBA bytes for the plain speaker icon.
pub fn speaker() -> Vec<u8> {
    draw(false)
}

/// RGBA bytes for the speaker icon with a "new copy" badge dot.
pub fn speaker_badged() -> Vec<u8> {
    draw(true)
}

fn draw(badged: bool) -> Vec<u8> {
    let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
    let center = 15.5f64;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let alpha = glyph_alpha(x, y, center, badged);
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
fn glyph_alpha(x: u32, y: u32, center: f64, badged: bool) -> u8 {
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
    // Badge: filled dot at the bottom right.
    let in_badge = badged && (x - 25.0).hypot(y - 25.0) <= 4.0;
    u8::from(in_body || in_cone || in_arc || in_badge) * 255
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(pixels: &[u8], x: u32, y: u32) -> u8 {
        pixels[((y * SIZE + x) * 4 + 3) as usize]
    }

    fn opaque_count(pixels: &[u8]) -> usize {
        pixels
            .iter()
            .skip(3)
            .step_by(4)
            .filter(|alpha| **alpha > 0)
            .count()
    }

    #[test]
    fn icons_have_the_right_size() {
        for icon in [speaker(), speaker_badged()] {
            assert_eq!(icon.len(), (SIZE * SIZE * 4) as usize);
        }
    }

    #[test]
    fn speaker_body_and_waves_are_opaque() {
        let icon = speaker();
        assert_eq!(alpha_at(&icon, 7, 16), 255, "speaker body");
        assert_eq!(alpha_at(&icon, 14, 16), 255, "speaker cone");
        assert_eq!(alpha_at(&icon, 17, 16), 255, "inner wave");
        assert_eq!(alpha_at(&icon, 21, 16), 255, "outer wave");
        assert_eq!(alpha_at(&icon, 0, 0), 0, "corner stays transparent");
        assert_eq!(alpha_at(&icon, 25, 25), 0, "no badge in the plain icon");
    }

    #[test]
    fn plain_icon_is_vertically_symmetric() {
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

    #[test]
    fn badge_only_adds_the_dot() {
        let plain = speaker();
        let badged = speaker_badged();
        // The badge dot is fully inside this box; everything outside it is
        // identical between the two icons.
        for y in 0..SIZE {
            for x in 0..SIZE {
                let inside = (20..=30).contains(&x) && (20..=30).contains(&y);
                if inside {
                    continue;
                }
                assert_eq!(
                    alpha_at(&plain, x, y),
                    alpha_at(&badged, x, y),
                    "pixel ({x}, {y}) outside the badge box"
                );
            }
        }
        assert_eq!(alpha_at(&badged, 25, 25), 255, "badge dot center");
        assert!(
            opaque_count(&badged) > opaque_count(&plain),
            "the badge adds opaque pixels"
        );
    }
}
