//! The daemon's model of what the keyboard should be showing.

use std::{collections::BTreeMap, time::Instant};

use apexide_core::{
    DeviceInfo, KeyCode, Rgb,
    oled::{Align, AnimFrame},
};
use serde::{Deserialize, Serialize};

/// Software lighting effect for the whole board. Per-key overrides in
/// [`Lighting::keys`] are painted on top.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    /// All LEDs off.
    Off,
    /// One solid color.
    Static { color: Rgb },
    /// Fade a color in and out.
    Breathe {
        color: Rgb,
        #[serde(default = "default_period")]
        period_ms: u32,
    },
    /// Whole board cycles through the hue wheel.
    Spectrum {
        #[serde(default = "default_slow_period")]
        period_ms: u32,
    },
    /// Rainbow sweeping across the board.
    Wave {
        #[serde(default = "default_period")]
        period_ms: u32,
        /// Width of one full rainbow, in key units.
        #[serde(default = "default_wavelength")]
        wavelength: f32,
        #[serde(default)]
        direction: Direction,
    },
    /// Static two-color gradient across the board.
    Gradient {
        from: Rgb,
        to: Rgb,
        #[serde(default)]
        direction: Direction,
    },
    /// Random keys glint on a background.
    Twinkle {
        color: Rgb,
        #[serde(default)]
        background: Rgb,
        #[serde(default = "default_period")]
        period_ms: u32,
    },
    /// Hand lighting back to the keyboard's own onboard profile.
    Onboard,
}

fn default_period() -> u32 {
    3000
}
fn default_slow_period() -> u32 {
    10000
}
fn default_wavelength() -> f32 {
    18.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[default]
    Right,
    Left,
    Up,
    Down,
}

/// Persistent lighting configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lighting {
    pub effect: Effect,
    /// Per-key colors painted over the effect.
    pub keys: BTreeMap<KeyCode, Rgb>,
    /// Global brightness, 0-100. Applied in software.
    pub brightness: u8,
}

impl Default for Lighting {
    fn default() -> Self {
        Self {
            effect: Effect::Onboard,
            keys: BTreeMap::new(),
            brightness: 100,
        }
    }
}

/// What the OLED shows when no notification is active.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OledMode {
    /// apexide clears the OLED once and then stops drawing on it.
    Release,
    /// Screen off.
    Blank,
    Text {
        text: String,
        #[serde(default)]
        align: Align,
    },
    /// A still image or animation.
    Image {
        name: String,
        frames: Vec<AnimFrame>,
    },
    /// Current time, `format` is a strftime string.
    Clock {
        #[serde(default = "default_clock")]
        format: String,
    },
    /// CPU and memory usage bars. The default, because a mode that keeps
    /// redrawing also clears the frame the firmware menu leaves behind.
    #[default]
    Sysinfo,
}

pub fn default_clock() -> String {
    "%H:%M".into()
}

impl OledMode {
    /// A copy without bulky image data, for status output.
    pub fn summary(&self) -> serde_json::Value {
        match self {
            OledMode::Image { name, frames } => serde_json::json!({
                "type": "image",
                "name": name,
                "frame_count": frames.len(),
            }),
            other => serde_json::to_value(other).unwrap_or_default(),
        }
    }
}

/// Everything that is saved to disk and in profiles.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub lighting: Lighting,
    pub oled: OledMode,
}

/// A temporary highlight that expires on its own.
#[derive(Clone, Debug)]
pub struct Flash {
    pub keys: Vec<KeyCode>,
    pub color: Rgb,
    pub started: Instant,
    pub until: Instant,
    /// Toggle on/off at this interval instead of holding steady.
    pub blink_ms: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Notification {
    pub text: String,
    pub until: Instant,
}

/// Shared between the API and the render thread.
#[derive(Debug, Default)]
pub struct Shared {
    pub settings: Settings,
    pub flashes: Vec<Flash>,
    pub notification: Option<Notification>,
    /// Set by the render thread.
    pub device: Option<DeviceInfo>,
    pub last_error: Option<String>,
    /// Last lighting and OLED frame computed by the render thread.
    pub frame_lighting: crate::effects::LightingFrame,
    pub frame_oled: Option<apexide_core::Frame>,
    /// When the OLED mode last changed; animations play from here.
    pub oled_since: Option<Instant>,
}
