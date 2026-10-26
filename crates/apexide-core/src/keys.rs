//! Key names, LED codes and physical positions.
//!
//! The Apex lighting protocol addresses LEDs by (mostly) HID usage codes, so
//! a key's LED code is the same byte the key sends when pressed. Positions are
//! in key units (1u = one alphanumeric key) and drive spatial effects as well
//! as GUI rendering.

use std::fmt;

use serde::{Deserialize, Serialize};

/// LED address of a single key. Serialized as its name (`"esc"`), or as
/// `"0xNN"` for codes without a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyCode(pub u8);

impl Serialize for KeyCode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for KeyCode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        parse_one(&s).ok_or_else(|| serde::de::Error::custom(UnknownKey(s)))
    }
}

fn serialize_code<S: serde::Serializer>(code: &KeyCode, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u8(code.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    /// Function row, alphanumerics and modifiers.
    Main,
    /// Print screen cluster, navigation cluster and arrows.
    Nav,
    /// Keys only present on ISO layouts.
    Iso,
    /// Numeric keypad (full-size boards only).
    Numpad,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct KeyDef {
    pub name: &'static str,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub aliases: &'static [&'static str],
    #[serde(serialize_with = "serialize_code")]
    pub code: KeyCode,
    pub section: Section,
    /// Left edge, in key units.
    pub x: f32,
    /// Top edge, in key units.
    pub y: f32,
    /// Width, in key units.
    pub w: f32,
}

impl KeyDef {
    /// Center point, in key units.
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + 0.5)
    }
}

/// Physical form factor, which decides which keys exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FormFactor {
    Full,
    Tkl,
}

impl FormFactor {
    /// Keys present on this form factor. ISO-only keys are always included;
    /// lighting a key that does not exist is harmless.
    pub fn keys(self) -> impl Iterator<Item = &'static KeyDef> {
        KEYS.iter()
            .filter(move |k| self == FormFactor::Full || k.section != Section::Numpad)
    }

    /// Overall size in key units, `(width, height)`.
    pub fn size(self) -> (f32, f32) {
        match self {
            FormFactor::Full => (22.5, 6.5),
            FormFactor::Tkl => (18.25, 6.5),
        }
    }
}

const fn k(name: &'static str, code: u8, x: f32, y: f32) -> KeyDef {
    kw(name, code, x, y, 1.0)
}

const fn kw(name: &'static str, code: u8, x: f32, y: f32, w: f32) -> KeyDef {
    KeyDef {
        name,
        aliases: &[],
        code: KeyCode(code),
        section: Section::Main,
        x,
        y,
        w,
    }
}

const fn alias(mut def: KeyDef, aliases: &'static [&'static str]) -> KeyDef {
    def.aliases = aliases;
    def
}

const fn sec(mut def: KeyDef, section: Section) -> KeyDef {
    def.section = section;
    def
}

const R0: f32 = 0.0;
const R1: f32 = 1.5;
const R2: f32 = 2.5;
const R3: f32 = 3.5;
const R4: f32 = 4.5;
const R5: f32 = 5.5;
const NAV: f32 = 15.25;
const PAD: f32 = 18.5;

/// Every addressable key across Apex 7 / Apex 7 TKL layouts.
pub static KEYS: &[KeyDef] = &[
    // Function row
    alias(k("esc", 0x29, 0.0, R0), &["escape"]),
    k("f1", 0x3A, 2.0, R0),
    k("f2", 0x3B, 3.0, R0),
    k("f3", 0x3C, 4.0, R0),
    k("f4", 0x3D, 5.0, R0),
    k("f5", 0x3E, 6.5, R0),
    k("f6", 0x3F, 7.5, R0),
    k("f7", 0x40, 8.5, R0),
    k("f8", 0x41, 9.5, R0),
    k("f9", 0x42, 11.0, R0),
    k("f10", 0x43, 12.0, R0),
    k("f11", 0x44, 13.0, R0),
    k("f12", 0x45, 14.0, R0),
    sec(
        alias(k("prtsc", 0x46, NAV, R0), &["printscreen", "print"]),
        Section::Nav,
    ),
    sec(
        alias(k("scrlk", 0x47, NAV + 1.0, R0), &["scrolllock"]),
        Section::Nav,
    ),
    sec(
        alias(k("pause", 0x48, NAV + 2.0, R0), &["break"]),
        Section::Nav,
    ),
    // Number row
    alias(k("grave", 0x35, 0.0, R1), &["`", "tilde", "backtick"]),
    k("1", 0x1E, 1.0, R1),
    k("2", 0x1F, 2.0, R1),
    k("3", 0x20, 3.0, R1),
    k("4", 0x21, 4.0, R1),
    k("5", 0x22, 5.0, R1),
    k("6", 0x23, 6.0, R1),
    k("7", 0x24, 7.0, R1),
    k("8", 0x25, 8.0, R1),
    k("9", 0x26, 9.0, R1),
    k("0", 0x27, 10.0, R1),
    alias(k("minus", 0x2D, 11.0, R1), &["-"]),
    alias(k("equal", 0x2E, 12.0, R1), &["=", "equals"]),
    alias(kw("backspace", 0x2A, 13.0, R1, 2.0), &["bksp"]),
    sec(alias(k("insert", 0x49, NAV, R1), &["ins"]), Section::Nav),
    sec(k("home", 0x4A, NAV + 1.0, R1), Section::Nav),
    sec(
        alias(k("pgup", 0x4B, NAV + 2.0, R1), &["pageup"]),
        Section::Nav,
    ),
    // Top letter row
    kw("tab", 0x2B, 0.0, R2, 1.5),
    k("q", 0x14, 1.5, R2),
    k("w", 0x1A, 2.5, R2),
    k("e", 0x08, 3.5, R2),
    k("r", 0x15, 4.5, R2),
    k("t", 0x17, 5.5, R2),
    k("y", 0x1C, 6.5, R2),
    k("u", 0x18, 7.5, R2),
    k("i", 0x0C, 8.5, R2),
    k("o", 0x12, 9.5, R2),
    k("p", 0x13, 10.5, R2),
    alias(k("lbracket", 0x2F, 11.5, R2), &["["]),
    alias(k("rbracket", 0x30, 12.5, R2), &["]"]),
    alias(kw("backslash", 0x31, 13.5, R2, 1.5), &["\\"]),
    sec(alias(k("delete", 0x4C, NAV, R2), &["del"]), Section::Nav),
    sec(k("end", 0x4D, NAV + 1.0, R2), Section::Nav),
    sec(
        alias(k("pgdn", 0x4E, NAV + 2.0, R2), &["pagedown"]),
        Section::Nav,
    ),
    // Home row
    alias(kw("capslock", 0x39, 0.0, R3, 1.75), &["caps"]),
    k("a", 0x04, 1.75, R3),
    k("s", 0x16, 2.75, R3),
    k("d", 0x07, 3.75, R3),
    k("f", 0x09, 4.75, R3),
    k("g", 0x0A, 5.75, R3),
    k("h", 0x0B, 6.75, R3),
    k("j", 0x0D, 7.75, R3),
    k("k", 0x0E, 8.75, R3),
    k("l", 0x0F, 9.75, R3),
    alias(k("semicolon", 0x33, 10.75, R3), &[";"]),
    alias(k("quote", 0x34, 11.75, R3), &["'", "apostrophe"]),
    sec(
        alias(k("iso_hash", 0x32, 12.75, R3), &["nonus_hash"]),
        Section::Iso,
    ),
    alias(kw("enter", 0x28, 12.75, R3, 2.25), &["return"]),
    // Bottom letter row
    alias(kw("lshift", 0xE1, 0.0, R4, 2.25), &["shift"]),
    sec(
        alias(k("iso_backslash", 0x64, 1.25, R4), &["nonus_backslash"]),
        Section::Iso,
    ),
    k("z", 0x1D, 2.25, R4),
    k("x", 0x1B, 3.25, R4),
    k("c", 0x06, 4.25, R4),
    k("v", 0x19, 5.25, R4),
    k("b", 0x05, 6.25, R4),
    k("n", 0x11, 7.25, R4),
    k("m", 0x10, 8.25, R4),
    alias(k("comma", 0x36, 9.25, R4), &[","]),
    alias(k("period", 0x37, 10.25, R4), &[".", "dot"]),
    alias(k("slash", 0x38, 11.25, R4), &["/"]),
    kw("rshift", 0xE5, 12.25, R4, 2.75),
    sec(k("up", 0x52, NAV + 1.0, R4), Section::Nav),
    // Modifier row
    alias(kw("lctrl", 0xE0, 0.0, R5, 1.25), &["ctrl", "control"]),
    alias(
        kw("lwin", 0xE3, 1.25, R5, 1.25),
        &["win", "super", "lgui", "meta"],
    ),
    alias(kw("lalt", 0xE2, 2.5, R5, 1.25), &["alt"]),
    kw("space", 0x2C, 3.75, R5, 6.25),
    alias(kw("ralt", 0xE6, 10.0, R5, 1.25), &["altgr"]),
    alias(kw("fn", 0xF0, 11.25, R5, 1.25), &["steelseries", "ss"]),
    alias(kw("menu", 0xE7, 12.5, R5, 1.25), &["rwin", "rgui", "app"]),
    kw("rctrl", 0xE4, 13.75, R5, 1.25),
    sec(k("left", 0x50, NAV, R5), Section::Nav),
    sec(k("down", 0x51, NAV + 1.0, R5), Section::Nav),
    sec(k("right", 0x4F, NAV + 2.0, R5), Section::Nav),
    // Numpad
    sec(k("numlock", 0x53, PAD, R1), Section::Numpad),
    sec(
        alias(k("kp_slash", 0x54, PAD + 1.0, R1), &["kp_divide"]),
        Section::Numpad,
    ),
    sec(
        alias(k("kp_star", 0x55, PAD + 2.0, R1), &["kp_multiply"]),
        Section::Numpad,
    ),
    sec(k("kp_minus", 0x56, PAD + 3.0, R1), Section::Numpad),
    sec(k("kp7", 0x5F, PAD, R2), Section::Numpad),
    sec(k("kp8", 0x60, PAD + 1.0, R2), Section::Numpad),
    sec(k("kp9", 0x61, PAD + 2.0, R2), Section::Numpad),
    sec(k("kp_plus", 0x57, PAD + 3.0, R2), Section::Numpad),
    sec(k("kp4", 0x5C, PAD, R3), Section::Numpad),
    sec(k("kp5", 0x5D, PAD + 1.0, R3), Section::Numpad),
    sec(k("kp6", 0x5E, PAD + 2.0, R3), Section::Numpad),
    sec(k("kp1", 0x59, PAD, R4), Section::Numpad),
    sec(k("kp2", 0x5A, PAD + 1.0, R4), Section::Numpad),
    sec(k("kp3", 0x5B, PAD + 2.0, R4), Section::Numpad),
    sec(k("kp_enter", 0x58, PAD + 3.0, R4), Section::Numpad),
    sec(kw("kp0", 0x62, PAD, R5, 2.0), Section::Numpad),
    sec(
        alias(k("kp_dot", 0x63, PAD + 2.0, R5), &["kp_period"]),
        Section::Numpad,
    ),
];

/// Named groups usable wherever a key selector is accepted.
pub const GROUPS: &[(&str, &str)] = &[
    ("all", "every key on the board"),
    ("letters", "a-z"),
    ("numbers", "number row 0-9"),
    ("fkeys", "f1-f12"),
    ("wasd", "w a s d"),
    ("arrows", "up down left right"),
    ("nav", "print screen, navigation cluster and arrows"),
    ("modifiers", "shift, ctrl, alt, win, fn, menu"),
    ("numpad", "numeric keypad"),
];

/// Look up a key by name, alias, single character or raw code (`0x2c`).
pub fn find(name: &str) -> Option<&'static KeyDef> {
    let n = name.trim().to_ascii_lowercase();
    KEYS.iter()
        .find(|k| k.name == n || k.aliases.contains(&n.as_str()))
}

/// Parse a single key name, alias or raw code (`0x87`); no groups.
pub fn parse_one(name: &str) -> Option<KeyCode> {
    if let Some(def) = find(name) {
        return Some(def.code);
    }
    let t = name.trim().to_ascii_lowercase();
    u8::from_str_radix(t.strip_prefix("0x")?, 16)
        .ok()
        .map(KeyCode)
}

pub fn by_code(code: KeyCode) -> Option<&'static KeyDef> {
    KEYS.iter().find(|k| k.code == code)
}

impl fmt::Display for KeyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match by_code(*self) {
            Some(def) => f.write_str(def.name),
            None => write!(f, "0x{:02x}", self.0),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown key or group {0:?} (see `apexide keys`)")]
pub struct UnknownKey(pub String);

/// Resolve a selector into LED codes. A selector is a comma- or
/// whitespace-separated list of key names, aliases, raw codes (`0x87`) and
/// group names (`wasd`, `fkeys`, `all`, ...).
pub fn resolve(selector: &str, form: FormFactor) -> Result<Vec<KeyCode>, UnknownKey> {
    let mut out: Vec<KeyCode> = Vec::new();
    for token in selector.split([',', ' ']).filter(|t| !t.is_empty()) {
        for code in resolve_token(token, form)? {
            if !out.contains(&code) {
                out.push(code);
            }
        }
    }
    Ok(out)
}

fn resolve_token(token: &str, form: FormFactor) -> Result<Vec<KeyCode>, UnknownKey> {
    let t = token.trim().to_ascii_lowercase();
    let codes = |pred: &dyn Fn(&KeyDef) -> bool| -> Vec<KeyCode> {
        form.keys().filter(|k| pred(k)).map(|k| k.code).collect()
    };
    let group = match t.as_str() {
        "all" => Some(codes(&|_| true)),
        "letters" => Some(codes(&|k| (0x04..=0x1D).contains(&k.code.0))),
        "numbers" => Some(codes(&|k| (0x1E..=0x27).contains(&k.code.0))),
        "fkeys" => Some(codes(&|k| (0x3A..=0x45).contains(&k.code.0))),
        "wasd" => Some(vec![
            KeyCode(0x1A),
            KeyCode(0x04),
            KeyCode(0x16),
            KeyCode(0x07),
        ]),
        "arrows" => Some(codes(&|k| (0x4F..=0x52).contains(&k.code.0))),
        "nav" => Some(codes(&|k| k.section == Section::Nav)),
        "modifiers" => Some(codes(&|k| k.code.0 >= 0xE0)),
        "numpad" => Some(codes(&|k| k.section == Section::Numpad)),
        _ => None,
    };
    if let Some(g) = group {
        return Ok(g);
    }
    parse_one(&t)
        .map(|c| vec![c])
        .ok_or_else(|| UnknownKey(token.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_codes_are_unique() {
        let mut names = std::collections::HashSet::new();
        let mut codes = std::collections::HashSet::new();
        for k in KEYS {
            assert!(names.insert(k.name), "duplicate name {}", k.name);
            for a in k.aliases {
                assert!(names.insert(a), "duplicate alias {a}");
            }
            assert!(codes.insert(k.code), "duplicate code {:?}", k.code);
        }
    }

    #[test]
    fn resolves_selectors() {
        let r = resolve("wasd, esc 0x87", FormFactor::Tkl).unwrap();
        assert_eq!(r.len(), 6);
        assert_eq!(r[4], KeyCode(0x29));
        assert_eq!(r[5], KeyCode(0x87));
        assert_eq!(
            resolve("W,w", FormFactor::Tkl).unwrap(),
            vec![KeyCode(0x1A)]
        );
        assert!(resolve("bogus", FormFactor::Tkl).is_err());
    }

    #[test]
    fn keycode_serde_uses_names() {
        assert_eq!(serde_json::to_string(&KeyCode(0x29)).unwrap(), "\"esc\"");
        assert_eq!(serde_json::to_string(&KeyCode(0x87)).unwrap(), "\"0x87\"");
        assert_eq!(
            serde_json::from_str::<KeyCode>("\"Escape\"").unwrap(),
            KeyCode(0x29)
        );
    }

    #[test]
    fn tkl_has_no_numpad() {
        assert!(resolve("numpad", FormFactor::Tkl).unwrap().is_empty());
        assert_eq!(resolve("numpad", FormFactor::Full).unwrap().len(), 17);
        assert_eq!(resolve("letters", FormFactor::Tkl).unwrap().len(), 26);
    }
}
