//! Per-frame color computation for software lighting effects.

use std::f32::consts::TAU;

use apexide_core::{KeyCode, Rgb, keys::KeyDef};

use crate::state::{Direction, Effect, Flash, Lighting};

/// Normalised position of a key along `dir`, in key units from the start edge.
fn along(key: &KeyDef, dir: Direction, size: (f32, f32)) -> f32 {
    let (cx, cy) = key.center();
    match dir {
        Direction::Right => cx,
        Direction::Left => size.0 - cx,
        Direction::Down => cy,
        Direction::Up => size.1 - cy,
    }
}

fn phase(t_ms: u64, period_ms: u32) -> f32 {
    let p = period_ms.max(1) as u64;
    (t_ms % p) as f32 / p as f32
}

/// Cheap deterministic per-key hash in `0.0..1.0`.
fn hash01(code: u8, salt: u32) -> f32 {
    let mut x = (code as u32).wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65536.0
}

/// Color of one key under `effect` at time `t_ms`. `None` means the effect
/// does not drive lighting (onboard mode).
pub fn effect_color(effect: &Effect, key: &KeyDef, size: (f32, f32), t_ms: u64) -> Option<Rgb> {
    Some(match effect {
        Effect::Onboard => return None,
        Effect::Off => Rgb::BLACK,
        Effect::Static { color } => *color,
        Effect::Breathe { color, period_ms } => {
            let p = phase(t_ms, *period_ms);
            color.scale(0.5 - 0.5 * (p * TAU).cos())
        }
        Effect::Spectrum { period_ms } => Rgb::from_hsv(phase(t_ms, *period_ms) * 360.0, 1.0, 1.0),
        Effect::Wave {
            period_ms,
            wavelength,
            direction,
        } => {
            let pos = along(key, *direction, size) / wavelength.max(0.1);
            Rgb::from_hsv((pos - phase(t_ms, *period_ms)) * 360.0, 1.0, 1.0)
        }
        Effect::Gradient {
            from,
            to,
            direction,
        } => {
            let extent = match direction {
                Direction::Left | Direction::Right => size.0,
                Direction::Up | Direction::Down => size.1,
            };
            from.lerp(*to, along(key, *direction, size) / extent)
        }
        Effect::Twinkle {
            color,
            background,
            period_ms,
        } => {
            // Each key gets its own offset and a slightly different period so
            // the glints never line up.
            let period = (*period_ms as f32 * (0.6 + 0.8 * hash01(key.code.0, 1))) as u32;
            let offset = (hash01(key.code.0, 2) * period as f32) as u64;
            let p = phase(t_ms + offset, period);
            let glint = (1.0 - (p * 4.0)).max(0.0); // short spike, then rest
            background.lerp(*color, glint * glint)
        }
    })
}

/// Final colors for every key, with overrides, flashes and brightness
/// applied. `None` when lighting is left to the keyboard.
pub fn render(
    lighting: &Lighting,
    flashes: &[Flash],
    keys: &[&KeyDef],
    size: (f32, f32),
    t_ms: u64,
    now: std::time::Instant,
) -> Option<Vec<(KeyCode, Rgb)>> {
    if lighting.effect == Effect::Onboard && flashes.is_empty() && lighting.keys.is_empty() {
        return None;
    }
    let brightness = lighting.brightness.min(100) as f32 / 100.0;
    let colors =
        keys.iter()
            .map(|key| {
                let mut c = effect_color(&lighting.effect, key, size, t_ms).unwrap_or(Rgb::BLACK);
                if let Some(o) = lighting.keys.get(&key.code) {
                    c = *o;
                }
                for f in flashes.iter().filter(|f| f.keys.contains(&key.code)) {
                    let on = match f.blink_ms {
                        Some(b) => ((now - f.started).as_millis() as u64 / b.max(1) as u64)
                            .is_multiple_of(2),
                        None => true,
                    };
                    if on {
                        c = f.color;
                    }
                }
                (key.code, c.scale(brightness))
            })
            .collect();
    Some(colors)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use apexide_core::{FormFactor, keys};

    use super::*;

    fn tkl() -> Vec<&'static KeyDef> {
        FormFactor::Tkl.keys().collect()
    }

    #[test]
    fn onboard_renders_nothing() {
        let l = Lighting::default();
        assert!(render(&l, &[], &tkl(), FormFactor::Tkl.size(), 0, Instant::now()).is_none());
    }

    #[test]
    fn overrides_and_brightness() {
        let mut l = Lighting {
            effect: Effect::Static {
                color: Rgb::new(0, 0, 200),
            },
            ..Default::default()
        };
        let esc = keys::find("esc").unwrap().code;
        l.keys.insert(esc, Rgb::new(200, 0, 0));
        l.brightness = 50;
        let out = render(&l, &[], &tkl(), FormFactor::Tkl.size(), 0, Instant::now()).unwrap();
        let get = |k| out.iter().find(|(c, _)| *c == k).unwrap().1;
        assert_eq!(get(esc), Rgb::new(100, 0, 0));
        assert_eq!(get(keys::find("a").unwrap().code), Rgb::new(0, 0, 100));
    }

    #[test]
    fn blinking_flash_alternates() {
        let l = Lighting {
            effect: Effect::Off,
            ..Default::default()
        };
        let a = keys::find("a").unwrap().code;
        let start = Instant::now();
        let flash = Flash {
            keys: vec![a],
            color: Rgb::new(255, 0, 0),
            started: start,
            until: start + Duration::from_secs(5),
            blink_ms: Some(100),
        };
        let at = |ms| {
            let out = render(
                &l,
                std::slice::from_ref(&flash),
                &tkl(),
                (18.0, 6.0),
                0,
                start + Duration::from_millis(ms),
            )
            .unwrap();
            out.iter().find(|(c, _)| *c == a).unwrap().1
        };
        assert_eq!(at(50), Rgb::new(255, 0, 0));
        assert_eq!(at(150), Rgb::BLACK);
    }

    #[test]
    fn gradient_endpoints() {
        let e = Effect::Gradient {
            from: Rgb::new(255, 0, 0),
            to: Rgb::new(0, 0, 255),
            direction: Direction::Right,
        };
        let size = FormFactor::Tkl.size();
        let esc = effect_color(&e, keys::find("esc").unwrap(), size, 0).unwrap();
        let right = effect_color(&e, keys::find("right").unwrap(), size, 0).unwrap();
        assert!(esc.r > 200 && esc.b < 50);
        assert!(right.b > 200 && right.r < 50);
    }
}
