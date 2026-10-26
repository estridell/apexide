//! 128x40 monochrome OLED frames: drawing, text layout and image conversion.

use std::{fmt, io::Cursor};

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use embedded_graphics::{
    mono_font::{MonoFont, MonoTextStyle, ascii},
    pixelcolor::BinaryColor,
    prelude::*,
    text::{Baseline, Text},
};
use image::{AnimationDecoder, DynamicImage, GrayImage, ImageFormat, imageops::FilterType};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const WIDTH: u32 = 128;
pub const HEIGHT: u32 = 40;

/// One OLED frame, row-major, 1 bit per pixel, most significant bit leftmost.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame([u8; Frame::BYTES]);

impl Frame {
    pub const BYTES: usize = (WIDTH * HEIGHT / 8) as usize;

    pub fn new() -> Self {
        Self([0; Self::BYTES])
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        Some(Self(bytes.try_into().ok()?))
    }

    pub fn as_bytes(&self) -> &[u8; Self::BYTES] {
        &self.0
    }

    pub fn get(&self, x: u32, y: u32) -> bool {
        if x >= WIDTH || y >= HEIGHT {
            return false;
        }
        let i = (y * WIDTH + x) as usize;
        self.0[i / 8] & (0x80 >> (i % 8)) != 0
    }

    pub fn set(&mut self, x: u32, y: u32, on: bool) {
        if x >= WIDTH || y >= HEIGHT {
            return;
        }
        let i = (y * WIDTH + x) as usize;
        let mask = 0x80 >> (i % 8);
        if on {
            self.0[i / 8] |= mask;
        } else {
            self.0[i / 8] &= !mask;
        }
    }

    pub fn invert(&mut self) {
        self.0.iter_mut().for_each(|b| *b = !*b);
    }

    /// Render the frame as ASCII art (`#` on, `.` off), handy for previews.
    pub fn to_ascii(&self) -> String {
        let mut s = String::with_capacity(((WIDTH + 1) * HEIGHT) as usize);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                s.push(if self.get(x, y) { '#' } else { '.' });
            }
            s.push('\n');
        }
        s
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Frame({} px on)",
            self.0.iter().map(|b| b.count_ones()).sum::<u32>()
        )
    }
}

impl Serialize for Frame {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&B64.encode(self.0))
    }
}

impl<'de> Deserialize<'de> for Frame {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let bytes = B64.decode(s).map_err(serde::de::Error::custom)?;
        Frame::from_bytes(&bytes).ok_or_else(|| {
            serde::de::Error::custom(format!("frame must be {} bytes", Frame::BYTES))
        })
    }
}

impl OriginDimensions for Frame {
    fn size(&self) -> Size {
        Size::new(WIDTH, HEIGHT)
    }
}

impl DrawTarget for Frame {
    type Color = BinaryColor;
    type Error = std::convert::Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<BinaryColor>>>(
        &mut self,
        pixels: I,
    ) -> Result<(), Self::Error> {
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 {
                self.set(p.x as u32, p.y as u32, c.is_on());
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Text

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

/// Fonts from largest to smallest; text layout picks the largest that fits.
const FONTS: &[&MonoFont<'static>] = &[
    &ascii::FONT_10X20,
    &ascii::FONT_9X15,
    &ascii::FONT_7X13,
    &ascii::FONT_6X10,
    &ascii::FONT_5X8,
    &ascii::FONT_4X6,
];

/// Word-wrap `text` to at most `cols` columns. Explicit newlines are kept and
/// words longer than a line are hard-split.
fn wrap(text: &str, cols: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let mut word = word.to_string();
            while word.chars().count() > cols {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                let head: String = word.chars().take(cols).collect();
                word = word.chars().skip(cols).collect();
                lines.push(head);
            }
            let needed = if line.is_empty() {
                word.len()
            } else {
                line.len() + 1 + word.len()
            };
            if needed > cols {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        lines.push(line);
    }
    lines
}

/// Draw `text` into `frame` inside the box `(x, y, w, h)`, picking the
/// largest font at which the wrapped text fits. Lines that still do not fit
/// at the smallest font are cut off.
pub fn draw_text(frame: &mut Frame, text: &str, align: Align, area: (i32, i32, u32, u32)) {
    let (ax, ay, aw, ah) = area;
    let (font, lines) = FONTS
        .iter()
        .map(|f| {
            let cols = (aw / (f.character_size.width + f.character_spacing)).max(1) as usize;
            (*f, wrap(text, cols))
        })
        .find(|(f, lines)| lines.len() as u32 * f.character_size.height <= ah)
        .unwrap_or_else(|| {
            let f = *FONTS.last().unwrap();
            let cols = (aw / f.character_size.width).max(1) as usize;
            let rows = (ah / f.character_size.height).max(1) as usize;
            (f, wrap(text, cols).into_iter().take(rows).collect())
        });

    let style = MonoTextStyle::new(font, BinaryColor::On);
    let lh = font.character_size.height as i32;
    let top = ay + (ah as i32 - lines.len() as i32 * lh) / 2;
    for (i, line) in lines.iter().enumerate() {
        let w = line.chars().count() as i32
            * (font.character_size.width + font.character_spacing) as i32;
        let x = match align {
            Align::Left => ax,
            Align::Center => ax + (aw as i32 - w) / 2,
            Align::Right => ax + aw as i32 - w,
        };
        Text::with_baseline(
            line,
            Point::new(x, top + i as i32 * lh),
            style,
            Baseline::Top,
        )
        .draw(frame)
        .ok();
    }
}

impl Frame {
    /// A frame with `text` laid out over the whole screen.
    pub fn text(text: &str, align: Align) -> Self {
        let mut f = Frame::new();
        draw_text(&mut f, text, align, (0, 0, WIDTH, HEIGHT));
        f
    }
}

// ---------------------------------------------------------------------------
// Images

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    /// Scale to fit inside the screen, keeping aspect ratio.
    #[default]
    Contain,
    /// Scale to cover the screen, cropping the overflow.
    Cover,
    /// Stretch to exactly 128x40.
    Stretch,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageOptions {
    pub fit: Fit,
    /// Floyd-Steinberg dithering; otherwise a plain threshold.
    pub dither: bool,
    /// Luma threshold (0-255) used when not dithering.
    pub threshold: u8,
    pub invert: bool,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            fit: Fit::Contain,
            dither: true,
            threshold: 128,
            invert: false,
        }
    }
}

/// One frame of a (possibly animated) image.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnimFrame {
    pub frame: Frame,
    pub delay_ms: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("could not decode image: {0}")]
    Decode(#[from] image::ImageError),
    #[error("image has no frames")]
    Empty,
}

/// Decode an image (PNG, JPEG, BMP, or animated GIF) into OLED frames.
pub fn image_frames(bytes: &[u8], opts: ImageOptions) -> Result<Vec<AnimFrame>, ImageError> {
    let format = image::guess_format(bytes)?;
    let frames: Vec<AnimFrame> = if format == ImageFormat::Gif {
        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))?;
        decoder
            .into_frames()
            .map(|f| {
                let f = f?;
                let (num, den) = f.delay().numer_denom_ms();
                let delay_ms = (num / den.max(1)).max(20);
                let img = DynamicImage::ImageRgba8(f.into_buffer());
                Ok(AnimFrame {
                    frame: convert(&img, opts),
                    delay_ms,
                })
            })
            .collect::<Result<_, image::ImageError>>()?
    } else {
        let img = image::load_from_memory_with_format(bytes, format)?;
        vec![AnimFrame {
            frame: convert(&img, opts),
            delay_ms: 0,
        }]
    };
    if frames.is_empty() {
        return Err(ImageError::Empty);
    }
    Ok(frames)
}

fn convert(img: &DynamicImage, opts: ImageOptions) -> Frame {
    // Flatten transparency onto black before converting to luma.
    let mut rgba = img.to_rgba8();
    for p in rgba.pixels_mut() {
        let a = p[3] as u32;
        for c in 0..3 {
            p[c] = (p[c] as u32 * a / 255) as u8;
        }
        p[3] = 255;
    }
    let img = DynamicImage::ImageRgba8(rgba);

    let canvas: GrayImage = match opts.fit {
        Fit::Stretch => img
            .resize_exact(WIDTH, HEIGHT, FilterType::Triangle)
            .to_luma8(),
        Fit::Cover => img
            .resize_to_fill(WIDTH, HEIGHT, FilterType::Triangle)
            .to_luma8(),
        Fit::Contain => {
            let scaled = img.resize(WIDTH, HEIGHT, FilterType::Triangle).to_luma8();
            let mut canvas = GrayImage::new(WIDTH, HEIGHT);
            let ox = (WIDTH - scaled.width()) / 2;
            let oy = (HEIGHT - scaled.height()) / 2;
            image::imageops::overlay(&mut canvas, &scaled, ox as i64, oy as i64);
            canvas
        }
    };

    let mut frame = Frame::new();
    if opts.dither {
        let mut err: Vec<f32> = canvas.pixels().map(|p| p[0] as f32).collect();
        let w = WIDTH as usize;
        for y in 0..HEIGHT as usize {
            for x in 0..w {
                let i = y * w + x;
                let old = err[i];
                let on = old >= 128.0;
                frame.set(x as u32, y as u32, on);
                let e = old - if on { 255.0 } else { 0.0 };
                let mut spread = |dx: isize, dy: usize, k: f32| {
                    let nx = x as isize + dx;
                    if nx >= 0 && (nx as usize) < w && y + dy < HEIGHT as usize {
                        err[(y + dy) * w + nx as usize] += e * k;
                    }
                };
                spread(1, 0, 7.0 / 16.0);
                spread(-1, 1, 3.0 / 16.0);
                spread(0, 1, 5.0 / 16.0);
                spread(1, 1, 1.0 / 16.0);
            }
        }
    } else {
        for (x, y, p) in canvas.enumerate_pixels() {
            frame.set(x, y, p[0] >= opts.threshold);
        }
    }
    if opts.invert {
        frame.invert();
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_bits() {
        let mut f = Frame::new();
        f.set(9, 0, true);
        assert_eq!(f.as_bytes()[1], 0b0100_0000);
        assert!(f.get(9, 0));
        f.set(9, 0, false);
        assert!(!f.get(9, 0));
        f.set(500, 500, true); // out of bounds is ignored
    }

    #[test]
    fn wraps_words() {
        assert_eq!(
            wrap("hello brave new world", 11),
            vec!["hello brave", "new world"]
        );
        assert_eq!(wrap("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
    }

    #[test]
    fn text_draws_something() {
        let f = Frame::text("Hi", Align::Center);
        assert!(f.as_bytes().iter().any(|&b| b != 0));
        // Big font is chosen for short text: glyphs span most of the height.
        let rows = (0..HEIGHT)
            .filter(|&y| (0..WIDTH).any(|x| f.get(x, y)))
            .count();
        assert!(rows >= 12, "rows={rows}");
    }

    #[test]
    fn frame_serde_roundtrip() {
        let f = Frame::text("x", Align::Left);
        let j = serde_json::to_string(&f).unwrap();
        assert_eq!(serde_json::from_str::<Frame>(&j).unwrap(), f);
    }

    #[test]
    fn png_converts() {
        let mut img = image::RgbImage::new(256, 80);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < 128 {
                image::Rgb([255, 255, 255])
            } else {
                image::Rgb([0, 0, 0])
            };
        }
        let mut png = Vec::new();
        DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let frames = image_frames(
            &png,
            ImageOptions {
                dither: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(frames.len(), 1);
        assert!(frames[0].frame.get(10, 20));
        assert!(!frames[0].frame.get(100, 20));
    }
}
