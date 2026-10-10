use std::error::Error;

use fontdue::Font;
use image::{DynamicImage, RgbImage};

use crate::config::{self, Preset, Shape};
use crate::lights::State;
use crate::media::Status;
use crate::Dial;

pub const KEY: u32 = 120;
pub const ZONE_WIDTH: u32 = 200;
pub const ZONE_HEIGHT: u32 = 100;

const BACKGROUND: [u8; 3] = [0x11, 0x11, 0x11];
const PANEL: [u8; 3] = [0x00, 0x00, 0x00];
const GUTTER: [u8; 3] = [0x00, 0x00, 0x00];
const TEXT: [u8; 3] = [0xf0, 0xf0, 0xf2];
const FAINT: [u8; 3] = [0x78, 0x78, 0x86];
const TRACK: [u8; 3] = [0x30, 0x30, 0x30];
const ACCENT: [u8; 3] = [0x36, 0x86, 0xc4];
const GLOW: [u8; 3] = [0xdb, 0xee, 0xff];
const DEAD: [u8; 3] = [0x3a, 0x3a, 0x44];
const RED: [u8; 3] = [0xe0, 0x55, 0x55];

const FONTS: [&str; 5] = [
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/liberation-sans-fonts/LiberationSans-Bold.ttf",
    "/usr/share/fonts/google-noto-vf/NotoSans[wght].ttf",
];

#[derive(Clone, Copy)]
pub enum Align {
    Left,
    Centre,
    Right,
}

pub struct Renderer {
    font: Font,
}

impl Renderer {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        for path in FONTS {
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(font) = Font::from_bytes(bytes, fontdue::FontSettings::default()) {
                    return Ok(Renderer { font });
                }
            }
        }
        Err("no usable font found; install dejavu-sans-fonts".into())
    }

    pub fn media_key(&self, index: u8, status: &Status) -> DynamicImage {
        let mut canvas = RgbImage::from_pixel(KEY, KEY, image::Rgb(BACKGROUND));
        let live = status.present;
        let ink = if live { TEXT } else { DEAD };

        match index {
            0 => {
                bar(&mut canvas, 36, 38, 7, 44, ink);
                triangle(&mut canvas, 84, 38, -38, 44, ink);
            }
            1 => {
                let colour = if live { ACCENT } else { DEAD };
                if status.playing {
                    bar(&mut canvas, 42, 36, 11, 48, colour);
                    bar(&mut canvas, 67, 36, 11, 48, colour);
                } else {
                    triangle(&mut canvas, 44, 36, 36, 48, colour);
                }
            }
            2 => {
                triangle(&mut canvas, 36, 38, 38, 44, ink);
                bar(&mut canvas, 77, 38, 7, 44, ink);
            }
            _ => {
                let colour = if status.muted { FAINT } else { TEXT };
                // Body runs 2 px under the cone so only its left corners show.
                panel(&mut canvas, 41.5, 60.0, 14.0, 17.0, 3.0, 0.0, colour);
                cone(&mut canvas, 46, 60, 16, 9, 25, colour);
                if status.muted {
                    stroke(&mut canvas, 33, 38, 75, 82, 6, RED);
                } else {
                    arc(&mut canvas, 58, 60, 13, 5, 0.85, colour);
                    arc(&mut canvas, 58, 60, 24, 5, 0.85, colour);
                }
            }
        }

        DynamicImage::ImageRgb8(canvas)
    }

    pub fn light_key(&self, name: &str, shape: Shape, state: &State, online: bool) -> DynamicImage {
        let lit = state.on && online;
        let mut canvas = RgbImage::from_pixel(KEY, KEY, image::Rgb(BACKGROUND));

        let ink = if !online {
            DEAD
        } else if lit {
            ACCENT
        } else {
            FAINT
        };

        self.text(&mut canvas, name, 19.0, 60, 28, if online { TEXT } else { DEAD }, Align::Centre);
        keylight(&mut canvas, 60, 60, shape, ink, if lit { Some(GLOW) } else { None }, BACKGROUND);

        DynamicImage::ImageRgb8(canvas)
    }

    pub fn all_key(&self, any_on: bool) -> DynamicImage {
        let mut canvas = RgbImage::from_pixel(KEY, KEY, image::Rgb(BACKGROUND));
        let ink = if any_on { ACCENT } else { FAINT };

        self.text(&mut canvas, "ALL", 19.0, 60, 28, TEXT, Align::Centre);
        power(&mut canvas, 60, 76, 21, 7, ink);

        DynamicImage::ImageRgb8(canvas)
    }

    /// A preset in the picker: its name over a bulb glowing the colour and
    /// intensity the preset sets, or unlit if it only turns lights off.
    pub fn preset_key(&self, preset: &Preset) -> DynamicImage {
        let mut canvas = RgbImage::from_pixel(KEY, KEY, image::Rgb(BACKGROUND));

        let mut size = 19.0;
        while size > 11.0 && self.width(&preset.name, size) > (KEY - 10) as f32 {
            size -= 1.0;
        }
        self.text(&mut canvas, &preset.name, size, 60, 28, TEXT, Align::Centre);

        let glass = appearance(preset).map(|(brightness, temperature)| {
            let colour = tint(coolness(temperature));
            let level = 0.3 + 0.7 * (brightness as f32 / config::BRIGHTNESS_MAX as f32).sqrt();
            mix(BACKGROUND, colour, level)
        });
        // Outline and base in the same grey as the light keys' icons.
        bulb(&mut canvas, 60, 60, glass, FAINT, BACKGROUND);

        DynamicImage::ImageRgb8(canvas)
    }

    /// A key with no light configured behind it.
    pub fn empty_key(&self) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(KEY, KEY, image::Rgb(BACKGROUND)))
    }

    /// A zone whose dial has no light configured behind it.
    pub fn empty_zone(&self) -> DynamicImage {
        let mut canvas = RgbImage::from_pixel(ZONE_WIDTH, ZONE_HEIGHT, image::Rgb(GUTTER));
        bar(&mut canvas, 3, 3, ZONE_WIDTH as i32 - 6, ZONE_HEIGHT as i32 - 6, PANEL);
        DynamicImage::ImageRgb8(canvas)
    }

    pub fn zone(&self, label: &str, mode: Dial, state: State, online: bool) -> DynamicImage {
        let mut canvas = RgbImage::from_pixel(ZONE_WIDTH, ZONE_HEIGHT, image::Rgb(GUTTER));
        bar(&mut canvas, 3, 3, ZONE_WIDTH as i32 - 6, ZONE_HEIGHT as i32 - 6, PANEL);

        self.text(&mut canvas, label, 20.0, 12, 26, if online { TEXT } else { DEAD }, Align::Left);
        self.text(
            &mut canvas,
            match mode {
                Dial::Brightness => "BRIGHTNESS",
                Dial::Temperature => "TEMPERATURE",
            },
            14.0,
            188,
            24,
            FAINT,
            Align::Right,
        );

        if !online {
            self.text(&mut canvas, "offline", 26.0, 12, 70, RED, Align::Left);
            return DynamicImage::ImageRgb8(canvas);
        }

        let (value, fraction, colour) = match mode {
            Dial::Brightness => {
                let fraction = state.brightness as f32 / config::BRIGHTNESS_MAX as f32;
                (format!("{}%", state.brightness), fraction, ACCENT)
            }
            Dial::Temperature => {
                let fraction = coolness(state.temperature);
                (format!("{}K", config::kelvin(state.temperature)), fraction, tint(fraction))
            }
        };

        let ink = if state.on { colour } else { FAINT };
        self.text(&mut canvas, &value, 34.0, 12, 72, ink, Align::Left);

        bar(&mut canvas, 12, 82, 176, 8, TRACK);
        let filled = (176.0 * fraction).round() as i32;
        if filled > 0 {
            bar(&mut canvas, 12, 82, filled, 8, ink);
        }

        DynamicImage::ImageRgb8(canvas)
    }

    fn width(&self, s: &str, size: f32) -> f32 {
        s.chars().map(|c| self.font.metrics(c, size).advance_width).sum()
    }

    fn text(&self, canvas: &mut RgbImage, s: &str, size: f32, x: i32, baseline: i32, colour: [u8; 3], align: Align) {
        let width = self.width(s, size);

        let mut pen = match align {
            Align::Left => x as f32,
            Align::Centre => x as f32 - width / 2.0,
            Align::Right => x as f32 - width,
        };

        for c in s.chars() {
            let (metrics, coverage) = self.font.rasterize(c, size);
            let left = pen.round() as i32 + metrics.xmin;
            let top = baseline - metrics.height as i32 - metrics.ymin;

            for row in 0..metrics.height {
                for column in 0..metrics.width {
                    let alpha = coverage[row * metrics.width + column];
                    if alpha > 0 {
                        blend(canvas, left + column as i32, top + row as i32, colour, alpha);
                    }
                }
            }

            pen += metrics.advance_width;
        }
    }
}

fn blend(canvas: &mut RgbImage, x: i32, y: i32, colour: [u8; 3], alpha: u8) {
    if x < 0 || y < 0 || x as u32 >= canvas.width() || y as u32 >= canvas.height() {
        return;
    }

    let pixel = canvas.get_pixel_mut(x as u32, y as u32);
    let a = alpha as u32;
    for channel in 0..3 {
        pixel[channel] = ((colour[channel] as u32 * a + pixel[channel] as u32 * (255 - a)) / 255) as u8;
    }
}

fn bar(canvas: &mut RgbImage, x: i32, y: i32, width: i32, height: i32, colour: [u8; 3]) {
    for row in 0..height {
        for column in 0..width {
            blend(canvas, x + column, y + row, colour, 255);
        }
    }
}

/// Positive width points right, negative points left from x.
fn triangle(canvas: &mut RgbImage, x: i32, y: i32, width: i32, height: i32, colour: [u8; 3]) {
    let span = width.abs();
    for row in 0..height {
        let distance = (row - height / 2).abs() as f32 / (height as f32 / 2.0);
        let length = ((1.0 - distance) * span as f32).round() as i32;
        for column in 0..length {
            let px = if width >= 0 { x + column } else { x - column };
            blend(canvas, px, y + row, colour, 255);
        }
    }
}

/// Rounded rectangle, antialiased at the corners.
fn rounded(canvas: &mut RgbImage, x: i32, y: i32, width: i32, height: i32, radius: i32, colour: [u8; 3]) {
    let radius = radius.min(width / 2).min(height / 2).max(1) as f32;

    for row in 0..height {
        for column in 0..width {
            let px = column as f32;
            let py = row as f32;
            let nx = px.clamp(radius, ((width - 1) as f32 - radius).max(radius));
            let ny = py.clamp(radius, ((height - 1) as f32 - radius).max(radius));
            let distance = ((px - nx).powi(2) + (py - ny).powi(2)).sqrt();
            let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend(canvas, x + column, y + row, colour, (coverage * 255.0) as u8);
            }
        }
    }
}

/// Rounded rectangle about a centre, rotatable, antialiased via a box SDF.
fn panel(canvas: &mut RgbImage, cx: f32, cy: f32, width: f32, height: f32, radius: f32, angle: f32, colour: [u8; 3]) {
    let (sin, cos) = angle.sin_cos();
    let reach = ((width * width + height * height).sqrt() / 2.0).ceil() as i32 + 2;

    for row in -reach..=reach {
        for column in -reach..=reach {
            let dx = column as f32;
            let dy = row as f32;
            let lx = dx * cos + dy * sin;
            let ly = -dx * sin + dy * cos;

            let qx = lx.abs() - (width / 2.0 - radius);
            let qy = ly.abs() - (height / 2.0 - radius);
            let distance =
                (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius;

            let coverage = (0.5 - distance).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend(canvas, cx as i32 + column, cy as i32 + row, colour, (coverage * 255.0) as u8);
            }
        }
    }
}

/// An Elgato Key Light: diffuser panel in a bezel, on its pole.
fn keylight(canvas: &mut RgbImage, cx: i32, cy: i32, shape: Shape, colour: [u8; 3], face: Option<[u8; 3]>, behind: [u8; 3]) {
    let (width, height, radius) = match shape {
        Shape::Panel => (68.0, 46.0, 17.0),
        Shape::Air => (48.0, 46.0, 17.0),
    };
    let bezel = 5.0;
    let pole = 6;
    let foot = KEY as i32 - 18;

    bar(canvas, cx - pole / 2, cy, pole, foot - cy, colour);
    rounded(canvas, cx - (width * 0.20) as i32, foot, (width * 0.40) as i32, pole + 2, 2, colour);

    panel(canvas, cx as f32, cy as f32, width, height, radius, 0.0, colour);
    panel(
        canvas,
        cx as f32,
        cy as f32,
        width - bezel * 2.0,
        height - bezel * 2.0,
        radius - bezel,
        0.0,
        face.unwrap_or(behind),
    );
}

/// Sound wave: part of a ring, opening to the right, `spread` radians either
/// side of horizontal.
fn arc(canvas: &mut RgbImage, cx: i32, cy: i32, radius: i32, thickness: i32, spread: f32, colour: [u8; 3]) {
    let outer = radius + thickness;
    let half = thickness as f32 / 2.0;

    for row in -outer..=outer {
        for column in 0..=outer {
            let dx = column as f32;
            let dy = row as f32;
            if dy.atan2(dx).abs() > spread {
                continue;
            }
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = (half + 0.5 - (distance - radius as f32).abs()).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend(canvas, cx + column, cy + row, colour, (coverage * 255.0) as u8);
            }
        }
    }
}

/// IEC 5009 standby mark: broken ring with a bar through the gap.
fn power(canvas: &mut RgbImage, cx: i32, cy: i32, radius: i32, thickness: i32, colour: [u8; 3]) {
    let outer = radius + thickness;
    let half = thickness as f32 / 2.0;
    let gap = 0.42;

    for row in -outer..=outer {
        for column in -outer..=outer {
            let dx = column as f32;
            let dy = row as f32;
            let distance = (dx * dx + dy * dy).sqrt();
            let coverage = (half + 0.5 - (distance - radius as f32).abs()).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            if dx.atan2(-dy).abs() < gap {
                continue;
            }
            blend(canvas, cx + column, cy + row, colour, (coverage * 255.0) as u8);
        }
    }

    let top = cy - radius - thickness - 3;
    let length = cy - (radius as f32 * 0.10) as i32 - top;
    rounded(canvas, cx - thickness / 2, top, thickness, length, thickness / 2, colour);
}

/// Speaker cone: flares from `near` to `far` half-height across `width`.
fn cone(canvas: &mut RgbImage, x: i32, centre: i32, width: i32, near: i32, far: i32, colour: [u8; 3]) {
    for column in 0..width {
        let t = column as f32 / (width - 1).max(1) as f32;
        let half = (near as f32 + (far - near) as f32 * t).round() as i32;
        for row in -half..=half {
            blend(canvas, x + column, centre + row, colour, 255);
        }
    }
}

fn stroke(canvas: &mut RgbImage, x0: i32, y0: i32, x1: i32, y1: i32, weight: i32, colour: [u8; 3]) {
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        let x = x0 + ((x1 - x0) as f32 * t).round() as i32;
        let y = y0 + ((y1 - y0) as f32 * t).round() as i32;
        bar(canvas, x - weight / 2, y - weight / 2, weight, weight, colour);
    }
}

/// What one bulb should show for a whole preset: the brightest light it turns
/// on, at the brightness-weighted colour of those lights. None if it turns no
/// light on. Unspecified values are taken as middling.
fn appearance(preset: &Preset) -> Option<(i32, i32)> {
    let lit: Vec<(i32, i32)> = preset
        .settings
        .iter()
        .filter(|(_, setting)| setting.on)
        .map(|(_, setting)| (setting.brightness.unwrap_or(50), setting.temperature.unwrap_or(230)))
        .collect();

    let brightest = lit.iter().map(|(brightness, _)| *brightness).max()?;
    let weight: i32 = lit.iter().map(|(brightness, _)| brightness + 1).sum();
    let temperature = lit.iter().map(|(brightness, temperature)| (brightness + 1) * temperature).sum::<i32>() / weight;

    Some((brightest, temperature))
}

/// Where a temperature sits between the warmest and coolest a light can do,
/// 0.0 warm to 1.0 cool, evenly in Kelvin as the display reads.
fn coolness(temperature: i32) -> f32 {
    let warm = config::kelvin(config::TEMPERATURE_MAX) as f32;
    let cool = config::kelvin(config::TEMPERATURE_MIN) as f32;
    let now = config::kelvin(temperature) as f32;
    ((now - warm) / (cool - warm)).clamp(0.0, 1.0)
}

fn mix(from: [u8; 3], to: [u8; 3], amount: f32) -> [u8; 3] {
    let mut out = [0u8; 3];
    for channel in 0..3 {
        out[channel] = (from[channel] as f32 + (to[channel] as f32 - from[channel] as f32) * amount).round() as u8;
    }
    out
}

/// Filled circle, antialiased.
fn disc(canvas: &mut RgbImage, cx: i32, cy: i32, radius: f32, colour: [u8; 3]) {
    let reach = radius.ceil() as i32 + 1;
    for row in -reach..=reach {
        for column in -reach..=reach {
            let distance = ((column * column + row * row) as f32).sqrt();
            let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend(canvas, cx + column, cy + row, colour, (coverage * 255.0) as u8);
            }
        }
    }
}

/// A light bulb: round glass narrowing to a neck, on a screw base. Outlined
/// and based in `metal`; the glass is filled with `glass` when lit, otherwise
/// left showing `behind`.
fn bulb(canvas: &mut RgbImage, cx: i32, cy: i32, glass: Option<[u8; 3]>, metal: [u8; 3], behind: [u8; 3]) {
    let top = cy + 8;
    let bottom = cy + 25;

    silhouette(canvas, cx, cy, top, bottom, 0.0, metal);
    // As thick as the bezel on the light keys' icons.
    silhouette(canvas, cx, cy, top, bottom, 5.0, glass.unwrap_or(behind));

    // Screw base: one solid block straight under the neck, with dark grooves
    // for the thread so it reads as a screw on any key colour, then the contact.
    let groove = mix(metal, [0, 0, 0], 0.55);
    rounded(canvas, cx - 8, bottom, 16, 12, 2, metal);
    bar(canvas, cx - 7, bottom + 4, 14, 1, groove);
    bar(canvas, cx - 7, bottom + 8, 14, 1, groove);
    rounded(canvas, cx - 4, bottom + 11, 8, 4, 2, metal);
}

/// The glass and neck of the bulb, shrunk by `inset` on every side.
fn silhouette(canvas: &mut RgbImage, cx: i32, cy: i32, top: i32, bottom: i32, inset: f32, colour: [u8; 3]) {
    let end = bottom - inset.round() as i32;
    for row in top..=end {
        let t = (row - top) as f32 / (bottom - top) as f32;
        let half = 12.0 - 4.5 * t - inset;
        for column in (-half.ceil() as i32)..=(half.ceil() as i32) {
            let coverage = (half + 0.5 - column.abs() as f32).clamp(0.0, 1.0);
            blend(canvas, cx + column, row, colour, (coverage * 255.0) as u8);
        }
    }
    disc(canvas, cx, cy, 18.0 - inset, colour);
}

/// Approximate the light's own colour, warm through cool.
fn tint(fraction: f32) -> [u8; 3] {
    let warm = [255.0, 176.0, 102.0];
    let cool = [198.0, 222.0, 255.0];
    let mut out = [0u8; 3];
    for channel in 0..3 {
        out[channel] = (warm[channel] + (cool[channel] - warm[channel]) * fraction).round() as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renderer() -> Renderer {
        Renderer::new().expect("tests need one of the fonts in FONTS installed")
    }

    fn rgb(image: &DynamicImage) -> RgbImage {
        image.to_rgb8()
    }

    fn at(canvas: &RgbImage, x: u32, y: u32) -> [u8; 3] {
        canvas.get_pixel(x, y).0
    }

    fn count(canvas: &RgbImage, colour: [u8; 3]) -> usize {
        canvas.pixels().filter(|pixel| pixel.0 == colour).count()
    }

    fn blank() -> RgbImage {
        RgbImage::from_pixel(20, 20, image::Rgb(BACKGROUND))
    }

    const ON: State = State { on: true, brightness: 50, temperature: 200 };

    #[test]
    fn blend_mixes_and_clips() {
        let mut canvas = RgbImage::from_pixel(2, 2, image::Rgb([0, 0, 0]));
        blend(&mut canvas, 0, 0, [255, 255, 255], 255);
        blend(&mut canvas, 1, 0, [200, 100, 50], 0);
        blend(&mut canvas, 0, 1, [255, 255, 255], 51);
        blend(&mut canvas, -1, 0, [255, 255, 255], 255);
        blend(&mut canvas, 2, 5, [255, 255, 255], 255);

        assert_eq!(at(&canvas, 0, 0), [255, 255, 255]);
        assert_eq!(at(&canvas, 1, 0), [0, 0, 0]);
        assert_eq!(at(&canvas, 0, 1), [51, 51, 51]);
        assert_eq!(at(&canvas, 1, 1), [0, 0, 0]);
    }

    #[test]
    fn bar_fills_exactly_its_rectangle() {
        let mut canvas = blank();
        bar(&mut canvas, 2, 3, 4, 5, RED);
        assert_eq!(count(&canvas, RED), 20);
        assert_eq!(at(&canvas, 2, 3), RED);
        assert_eq!(at(&canvas, 5, 7), RED);
        assert_eq!(at(&canvas, 6, 7), BACKGROUND);
        assert_eq!(at(&canvas, 5, 8), BACKGROUND);
    }

    #[test]
    fn bar_off_the_edge_is_clipped() {
        let mut canvas = blank();
        bar(&mut canvas, 15, 15, 50, 50, RED);
        assert_eq!(count(&canvas, RED), 25);
    }

    #[test]
    fn triangle_points_by_sign_of_width() {
        let mut right = blank();
        triangle(&mut right, 2, 2, 10, 10, RED);
        assert_eq!(at(&right, 2, 7), RED);
        assert_eq!(at(&right, 10, 7), RED);
        assert_eq!(at(&right, 1, 7), BACKGROUND);

        let mut left = blank();
        triangle(&mut left, 17, 2, -10, 10, RED);
        assert_eq!(at(&left, 17, 7), RED);
        assert_eq!(at(&left, 9, 7), RED);
        assert_eq!(at(&left, 18, 7), BACKGROUND);

        assert_eq!(count(&right, RED), count(&left, RED));
    }

    #[test]
    fn rounded_softens_corners_only() {
        let mut canvas = blank();
        rounded(&mut canvas, 0, 0, 20, 20, 6, RED);
        assert_eq!(at(&canvas, 10, 10), RED);
        // Edges are antialiased too, so the outermost row is a half blend.
        assert_ne!(at(&canvas, 10, 0), BACKGROUND);
        assert_eq!(at(&canvas, 10, 1), RED);
        assert_eq!(at(&canvas, 0, 0), BACKGROUND);
        assert_eq!(at(&canvas, 19, 19), BACKGROUND);
    }

    #[test]
    fn rounded_copes_with_small_even_sizes() {
        let mut canvas = blank();
        rounded(&mut canvas, 2, 2, 16, 4, 2, RED);
        rounded(&mut canvas, 2, 8, 2, 2, 5, RED);
        rounded(&mut canvas, 2, 12, 1, 1, 1, RED);
        assert!(count(&canvas, RED) > 0);
    }

    #[test]
    fn tint_runs_warm_to_cool() {
        assert_eq!(tint(0.0), [255, 176, 102]);
        assert_eq!(tint(1.0), [198, 222, 255]);
        let middle = tint(0.5);
        assert!(middle[0] < 255 && middle[0] > 198);
        assert!(middle[2] > 102 && middle[2] < 255);
    }

    #[test]
    fn surfaces_have_hardware_dimensions() {
        let renderer = renderer();
        let status = Status { present: true, playing: false, muted: false };
        for index in 0..4 {
            let image = renderer.media_key(index, &status);
            assert_eq!((image.width(), image.height()), (KEY, KEY));
        }
        let image = renderer.light_key("Key", Shape::Panel, &ON, true);
        assert_eq!((image.width(), image.height()), (KEY, KEY));
        let image = renderer.all_key(true);
        assert_eq!((image.width(), image.height()), (KEY, KEY));
        let image = renderer.zone("Key", Dial::Brightness, ON, true);
        assert_eq!((image.width(), image.height()), (ZONE_WIDTH, ZONE_HEIGHT));
        assert_eq!(ZONE_WIDTH * 4, 800);
    }

    #[test]
    fn empty_surfaces_are_blank() {
        let renderer = renderer();
        let key = rgb(&renderer.empty_key());
        assert_eq!((key.width(), key.height()), (KEY, KEY));
        assert_eq!(count(&key, BACKGROUND), (KEY * KEY) as usize);

        let zone = rgb(&renderer.empty_zone());
        assert_eq!((zone.width(), zone.height()), (ZONE_WIDTH, ZONE_HEIGHT));
        assert!(zone.pixels().all(|pixel| pixel.0 == GUTTER || pixel.0 == PANEL));
    }

    #[test]
    fn preset_key_lights_its_bulb() {
        let renderer = renderer();
        let setting = config::Setting { on: true, brightness: Some(50), temperature: Some(200) };
        let mut preset = Preset { name: "Work".into(), settings: vec![("Key".into(), setting)] };

        let lit = rgb(&renderer.preset_key(&preset));
        assert_ne!(at(&lit, 60, 60), BACKGROUND);
        assert_eq!(at(&lit, 35, 80), BACKGROUND);

        // A preset that only turns lights off shows a hollow, unlit bulb.
        preset.settings[0].1 = config::Setting { on: false, brightness: None, temperature: None };
        let unlit = rgb(&renderer.preset_key(&preset));
        assert_eq!(at(&unlit, 60, 60), BACKGROUND);
        assert_eq!(at(&unlit, 60, 43), FAINT);
    }

    #[test]
    fn play_key_shows_state() {
        let renderer = renderer();
        let playing = rgb(&renderer.media_key(1, &Status { present: true, playing: true, muted: false }));
        let paused = rgb(&renderer.media_key(1, &Status { present: true, playing: false, muted: false }));
        let absent = rgb(&renderer.media_key(1, &Status { present: false, playing: false, muted: false }));

        assert_ne!(playing, paused);
        // The gap between the two pause bars.
        assert_eq!(at(&playing, 60, 60), BACKGROUND);
        assert_eq!(at(&paused, 60, 60), ACCENT);
        assert_eq!(count(&absent, ACCENT), 0);
        assert!(count(&absent, DEAD) > 0);
    }

    #[test]
    fn transport_keys_dim_without_a_player() {
        let renderer = renderer();
        let present = Status { present: true, playing: false, muted: false };
        let absent = Status::default();
        for index in [0, 2] {
            assert!(count(&rgb(&renderer.media_key(index, &present)), TEXT) > 0);
            assert_eq!(count(&rgb(&renderer.media_key(index, &absent)), TEXT), 0);
        }
    }

    #[test]
    fn previous_and_next_carry_the_same_ink() {
        let renderer = renderer();
        let status = Status { present: true, playing: false, muted: false };
        let previous = rgb(&renderer.media_key(0, &status));
        let next = rgb(&renderer.media_key(2, &status));
        assert_eq!(count(&previous, TEXT), count(&next, TEXT));
        assert_eq!(at(&previous, 38, 60), TEXT);
        assert_eq!(at(&next, 80, 60), TEXT);
    }

    #[test]
    fn mute_key_is_struck_through_when_muted() {
        let renderer = renderer();
        let loud = rgb(&renderer.media_key(3, &Status { present: false, playing: false, muted: false }));
        let muted = rgb(&renderer.media_key(3, &Status { present: false, playing: false, muted: true }));
        assert_eq!(count(&loud, RED), 0);
        assert!(count(&muted, RED) > 0);
        assert!(count(&loud, TEXT) > count(&muted, TEXT));

        // Sound waves only when there is sound: on the inner arc's line.
        assert_eq!(at(&loud, 71, 60), TEXT);
        assert_eq!(at(&loud, 82, 60), TEXT);
        assert_eq!(at(&muted, 82, 60), BACKGROUND);
    }

    #[test]
    fn light_key_glows_only_when_on_and_online() {
        let renderer = renderer();
        let off = State { on: false, ..ON };

        let lit = rgb(&renderer.light_key("Key", Shape::Panel, &ON, true));
        assert_eq!(at(&lit, 60, 60), GLOW);
        assert!(count(&lit, ACCENT) > 0);

        let dark = rgb(&renderer.light_key("Key", Shape::Panel, &off, true));
        assert_eq!(at(&dark, 60, 60), BACKGROUND);
        assert_eq!(count(&dark, ACCENT), 0);
        assert!(count(&dark, FAINT) > 0);

        let gone = rgb(&renderer.light_key("Key", Shape::Panel, &ON, false));
        assert_eq!(count(&gone, GLOW), 0);
        assert_eq!(count(&gone, TEXT), 0);
        assert!(count(&gone, DEAD) > 0);
    }

    #[test]
    fn air_is_narrower_than_panel() {
        let renderer = renderer();
        let panel = rgb(&renderer.light_key("Key", Shape::Panel, &ON, true));
        let air = rgb(&renderer.light_key("Key", Shape::Air, &ON, true));
        assert!(count(&panel, GLOW) > count(&air, GLOW));
        // Panel edge sits at 60 +/- 34; the Air's at 60 +/- 24.
        assert_ne!(at(&panel, 30, 60), BACKGROUND);
        assert_eq!(at(&air, 30, 60), BACKGROUND);
    }

    #[test]
    fn all_key_follows_any_on() {
        let renderer = renderer();
        assert!(count(&rgb(&renderer.all_key(true)), ACCENT) > 0);
        assert_eq!(count(&rgb(&renderer.all_key(false)), ACCENT), 0);
    }

    #[test]
    fn zone_bar_tracks_brightness() {
        let renderer = renderer();
        let fill = |brightness| {
            let zone = rgb(&renderer.zone("Key", Dial::Brightness, State { brightness, ..ON }, true));
            (12..188).filter(|&x| at(&zone, x, 86) == ACCENT).count()
        };
        assert_eq!(fill(0), 0);
        assert_eq!(fill(50), 88);
        assert_eq!(fill(100), 176);
    }

    #[test]
    fn zone_bar_is_faint_when_light_is_off() {
        let renderer = renderer();
        let zone = rgb(&renderer.zone("Key", Dial::Brightness, State { on: false, ..ON }, true));
        assert_eq!(at(&zone, 12, 86), FAINT);
        assert_eq!(count(&zone, ACCENT), 0);
    }

    #[test]
    fn zone_temperature_is_tinted() {
        let renderer = renderer();
        let warm = rgb(&renderer.zone("Key", Dial::Temperature, State { temperature: config::TEMPERATURE_MAX, ..ON }, true));
        let cool = rgb(&renderer.zone("Key", Dial::Temperature, State { temperature: config::TEMPERATURE_MIN, ..ON }, true));

        // Warmest is an empty bar, coolest a full one in the cool tint.
        assert_eq!(at(&warm, 12, 86), TRACK);
        assert_eq!(at(&cool, 12, 86), tint(1.0));
        assert_eq!(at(&cool, 187, 86), tint(1.0));
        assert_eq!(count(&cool, ACCENT), 0);
    }

    #[test]
    fn zone_offline_has_no_bar() {
        let renderer = renderer();
        let zone = rgb(&renderer.zone("Key", Dial::Brightness, ON, false));
        assert!(count(&zone, RED) > 0);
        assert_eq!(count(&zone, TRACK), 0);
        assert_eq!(count(&zone, ACCENT), 0);
    }

    #[test]
    fn zone_is_framed_by_the_gutter() {
        let renderer = renderer();
        let zone = rgb(&renderer.zone("Key", Dial::Brightness, ON, true));
        for x in 0..ZONE_WIDTH {
            assert_eq!(at(&zone, x, 0), GUTTER);
            assert_eq!(at(&zone, x, ZONE_HEIGHT - 1), GUTTER);
        }
    }

    #[test]
    fn mode_label_changes_with_dial() {
        let renderer = renderer();
        let brightness = rgb(&renderer.zone("Key", Dial::Brightness, ON, true));
        let temperature = rgb(&renderer.zone("Key", Dial::Temperature, ON, true));
        let label = |canvas: &RgbImage| (100..ZONE_WIDTH).filter(|&x| (8..28).any(|y| at(canvas, x, y) != PANEL)).count();
        assert!(label(&temperature) > label(&brightness));
    }

    #[test]
    fn text_alignment() {
        let renderer = renderer();
        let extent = |align| {
            let mut canvas = RgbImage::from_pixel(200, 40, image::Rgb(PANEL));
            renderer.text(&mut canvas, "MK.2", 20.0, 100, 30, TEXT, align);
            let columns: Vec<u32> = (0..200).filter(|&x| (0..40).any(|y| at(&canvas, x, y) != PANEL)).collect();
            (*columns.first().unwrap(), *columns.last().unwrap())
        };

        let (left, _) = extent(Align::Left);
        let (_, right) = extent(Align::Right);
        let (start, end) = extent(Align::Centre);
        assert!((99..=102).contains(&left));
        assert!((97..=101).contains(&right));
        assert!(((start + end) as i32 / 2 - 100).abs() <= 2);
    }
}
