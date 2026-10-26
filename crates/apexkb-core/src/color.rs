use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A 24-bit RGB color. Serialized as a `#rrggbb` string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Named colors accepted anywhere a color is parsed.
pub const NAMED_COLORS: &[(&str, Rgb)] = &[
    ("off", Rgb::BLACK),
    ("black", Rgb::BLACK),
    ("white", Rgb::new(255, 255, 255)),
    ("red", Rgb::new(255, 0, 0)),
    ("green", Rgb::new(0, 255, 0)),
    ("blue", Rgb::new(0, 0, 255)),
    ("yellow", Rgb::new(255, 200, 0)),
    ("orange", Rgb::new(255, 80, 0)),
    ("cyan", Rgb::new(0, 255, 255)),
    ("teal", Rgb::new(0, 160, 128)),
    ("magenta", Rgb::new(255, 0, 255)),
    ("purple", Rgb::new(128, 0, 255)),
    ("pink", Rgb::new(255, 40, 120)),
    ("warm", Rgb::new(255, 140, 60)),
];

impl Rgb {
    pub const BLACK: Rgb = Rgb::new(0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Hue in degrees, saturation and value in `0.0..=1.0`.
    pub fn from_hsv(h: f32, s: f32, v: f32) -> Self {
        let h = h.rem_euclid(360.0) / 60.0;
        let c = v * s;
        let x = c * (1.0 - (h % 2.0 - 1.0).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = v - c;
        let to_u8 = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        Self::new(to_u8(r), to_u8(g), to_u8(b))
    }

    /// Multiply every channel by `factor` (clamped to `0.0..=1.0`).
    pub fn scale(self, factor: f32) -> Self {
        let f = factor.clamp(0.0, 1.0);
        let s = |c: u8| (c as f32 * f).round() as u8;
        Self::new(s(self.r), s(self.g), s(self.b))
    }

    /// Linear blend from `self` (t = 0) to `other` (t = 1).
    pub fn lerp(self, other: Rgb, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let l = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Self::new(l(self.r, other.r), l(self.g, other.g), l(self.b, other.b))
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid color {0:?}: use #rrggbb, #rgb or a name like red/blue/off")]
pub struct ParseColorError(String);

impl FromStr for Rgb {
    type Err = ParseColorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let t = s.trim().to_ascii_lowercase();
        if let Some((_, c)) = NAMED_COLORS.iter().find(|(n, _)| *n == t) {
            return Ok(*c);
        }
        let hex = t.strip_prefix('#').unwrap_or(&t);
        let err = || ParseColorError(s.to_string());
        let nibble = |c: u8| (c as char).to_digit(16).map(|d| d as u8).ok_or_else(err);
        let b = hex.as_bytes();
        match b.len() {
            6 => Ok(Rgb::new(
                nibble(b[0])? << 4 | nibble(b[1])?,
                nibble(b[2])? << 4 | nibble(b[3])?,
                nibble(b[4])? << 4 | nibble(b[5])?,
            )),
            3 => Ok(Rgb::new(
                nibble(b[0])? * 17,
                nibble(b[1])? * 17,
                nibble(b[2])? * 17,
            )),
            _ => Err(err()),
        }
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_and_names() {
        assert_eq!("#ff8000".parse::<Rgb>().unwrap(), Rgb::new(255, 128, 0));
        assert_eq!("0f0".parse::<Rgb>().unwrap(), Rgb::new(0, 255, 0));
        assert_eq!("Red".parse::<Rgb>().unwrap(), Rgb::new(255, 0, 0));
        assert!("#12345".parse::<Rgb>().is_err());
        assert!("nope".parse::<Rgb>().is_err());
    }

    #[test]
    fn hsv_primaries() {
        assert_eq!(Rgb::from_hsv(0.0, 1.0, 1.0), Rgb::new(255, 0, 0));
        assert_eq!(Rgb::from_hsv(120.0, 1.0, 1.0), Rgb::new(0, 255, 0));
        assert_eq!(Rgb::from_hsv(240.0, 1.0, 1.0), Rgb::new(0, 0, 255));
    }

    #[test]
    fn serde_roundtrip() {
        let c = Rgb::new(1, 2, 3);
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(j, "\"#010203\"");
        assert_eq!(serde_json::from_str::<Rgb>(&j).unwrap(), c);
    }
}
