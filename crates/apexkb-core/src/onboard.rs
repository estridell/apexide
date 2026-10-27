//! Onboard profiles: the five profile slots stored in the keyboard's flash.
//!
//! GG saves settings into these slots so they keep working with no software
//! running. A slot holds a name, key mappings, a second (META) layer of mappings,
//! macros and an OLED image. Per-key lighting is not in it; GG keeps that in
//! a second family of files ([`LIGHTING_FILE`]) that nobody has decoded yet.
//!
//! Prior art: the protocol and the payload layout come from
//! [steelseriesgg-rs](https://github.com/Ven0m0/steelseriesgg-rs) (MIT),
//! which reverse-engineered them from USB captures of GG and GG's own device
//! specifications, and confirmed read, write and switch on an Apex 7
//! (firmware 4.16.8, schema B). See its
//! `docs/development/apex-7-gg-capture.md`. On an Apex 7 TKL running firmware
//! 4.1.0, a read returns schema A and every pull is answered with `85 01`.
//!
//! Everything here is pure; [`crate::Keyboard`] sends the frames.
//!
//! | Step | Report | Frame |
//! | --- | --- | --- |
//! | copy a file slice into the 1024-byte buffer | output | `85 00, file u16, offset u32, size u16`, answered `85 01` |
//! | read the buffer | feature (set, then get) | `83 00, buffer offset u16, size u16` |
//! | open a file for writing | output | `88 00, file u16`, answered `88 01` |
//! | write the buffer | feature | `03 00, buffer offset u16, size u16, data` |
//! | copy the buffer into the file | output | `05 00, file u16, offset u32, size u16`, answered `05 01` |
//! | commit | output | `EB, slot + 1` |
//! | switch to a slot | output | `89, slot + 1` |
//!
//! All values are little-endian.

use serde::Serialize;

use crate::{
    color::Rgb,
    keys::KeyCode,
    oled::Frame,
    protocol::{FEATURE_LEN, OUTPUT_LEN},
};

/// Number of profile slots. GG calls them Config 1-5; on the wire and in
/// this module they are slots 0-4.
pub const SLOTS: u8 = 5;
/// File id of the profile in slot 0; slot `n` is `PROFILE_FILE + n`.
pub const PROFILE_FILE: u16 = 0x0201;
/// File id of what GG's specs call the lighting config of slot 0; slot `n`
/// is `LIGHTING_FILE + n`. Its contents are unknown.
pub const LIGHTING_FILE: u16 = 0x0301;

/// Size of the keyboard's transfer buffer, and of one pull or push.
pub const BUFFER_LEN: usize = 1024;
/// Bytes moved by one buffer read or write.
pub const CHUNK_LEN: usize = 512;
/// Bytes one write covers: GG's `profile_size`. The payload is padded with
/// zeros up to this length.
pub const REGION_LEN: usize = 40 * BUFFER_LEN;

pub const CMD_BUFFER_WRITE: u8 = 0x03;
pub const CMD_PUSH: u8 = 0x05;
pub const CMD_BUFFER_READ: u8 = 0x83;
pub const CMD_PULL: u8 = 0x85;
pub const CMD_OPEN: u8 = 0x88;
pub const CMD_SELECT: u8 = 0x89;
pub const CMD_COMMIT: u8 = 0xEB;
/// Status byte of an accepted pull, open or push.
pub const ACK_OK: u8 = 0x01;

/// File id of a slot's profile.
pub fn profile_file(slot: u8) -> u16 {
    PROFILE_FILE + u16::from(slot)
}

fn output(bytes: &[u8]) -> [u8; OUTPUT_LEN + 1] {
    let mut buf = [0u8; OUTPUT_LEN + 1];
    buf[1..1 + bytes.len()].copy_from_slice(bytes);
    buf
}

fn file_frame(cmd: u8, file: u16, offset: u32, len: u16) -> [u8; OUTPUT_LEN + 1] {
    let mut f = [0u8; 10];
    f[0] = cmd;
    f[2..4].copy_from_slice(&file.to_le_bytes());
    f[4..8].copy_from_slice(&offset.to_le_bytes());
    f[8..10].copy_from_slice(&len.to_le_bytes());
    output(&f)
}

/// Copy `len` bytes of `file` from `offset` into the transfer buffer.
pub fn pull(file: u16, offset: u32, len: u16) -> [u8; OUTPUT_LEN + 1] {
    file_frame(CMD_PULL, file, offset, len)
}

/// Ask for `len` bytes of the transfer buffer from `offset`. The answer is
/// the next feature report read, with the data from byte 1.
pub fn read_request(offset: u16, len: u16) -> Vec<u8> {
    let mut buf = vec![0u8; FEATURE_LEN + 1];
    buf[1] = CMD_BUFFER_READ;
    buf[3..5].copy_from_slice(&offset.to_le_bytes());
    buf[5..7].copy_from_slice(&len.to_le_bytes());
    buf
}

/// Open `file` for writing.
pub fn open(file: u16) -> [u8; OUTPUT_LEN + 1] {
    let f = file.to_le_bytes();
    output(&[CMD_OPEN, 0, f[0], f[1]])
}

/// Write `data` (at most [`CHUNK_LEN`] bytes) into the transfer buffer at
/// `offset`.
pub fn buffer_write(offset: u16, data: &[u8]) -> Vec<u8> {
    assert!(data.len() <= CHUNK_LEN, "buffer write too long");
    let mut buf = vec![0u8; FEATURE_LEN + 1];
    buf[1] = CMD_BUFFER_WRITE;
    buf[3..5].copy_from_slice(&offset.to_le_bytes());
    buf[5..7].copy_from_slice(&(data.len() as u16).to_le_bytes());
    buf[7..7 + data.len()].copy_from_slice(data);
    buf
}

/// Copy the whole transfer buffer into `file` at `offset`.
pub fn push(file: u16, offset: u32) -> [u8; OUTPUT_LEN + 1] {
    file_frame(CMD_PUSH, file, offset, BUFFER_LEN as u16)
}

/// Commit the file written to `slot`.
pub fn commit(slot: u8) -> [u8; OUTPUT_LEN + 1] {
    output(&[CMD_COMMIT, slot + 1])
}

/// Make the keyboard run `slot`. Stored data is not touched.
pub fn select(slot: u8) -> [u8; OUTPUT_LEN + 1] {
    output(&[CMD_SELECT, slot + 1])
}

/// One frame of a write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteStep {
    /// An output report, then (if `ack` is set) an answer that starts with
    /// `ack` and carries [`ACK_OK`].
    Output {
        report: [u8; OUTPUT_LEN + 1],
        ack: Option<u8>,
    },
    Feature(Vec<u8>),
}

/// The frames that store `payload` in `slot`: open the file, then for each
/// 1024-byte window of [`REGION_LEN`] two buffer writes and a push, then
/// commit. This is the 122-frame sequence GG sends.
pub fn write_plan(slot: u8, payload: &[u8]) -> Vec<WriteStep> {
    assert!(payload.len() <= REGION_LEN, "payload larger than a slot");
    let file = profile_file(slot);
    let mut region = payload.to_vec();
    region.resize(REGION_LEN, 0);
    let mut steps = vec![WriteStep::Output {
        report: open(file),
        ack: Some(CMD_OPEN),
    }];
    for (i, window) in region.chunks(BUFFER_LEN).enumerate() {
        for (j, chunk) in window.chunks(CHUNK_LEN).enumerate() {
            steps.push(WriteStep::Feature(buffer_write(
                (j * CHUNK_LEN) as u16,
                chunk,
            )));
        }
        steps.push(WriteStep::Output {
            report: push(file, (i * BUFFER_LEN) as u32),
            ack: Some(CMD_PUSH),
        });
    }
    steps.push(WriteStep::Output {
        report: commit(slot),
        ack: None,
    });
    steps
}

/// The CRC-32 a profile ends with: STM32 hardware CRC (polynomial
/// `0x04C11DB7`, init `0xFFFFFFFF`, no reflection, no final XOR), fed with
/// the data as little-endian 32-bit words.
pub fn crc32(data: &[u8]) -> u32 {
    data.chunks(4).fold(0xFFFF_FFFF, |crc, word| {
        word.iter().rev().fold(crc, |mut crc, &byte| {
            crc ^= u32::from(byte) << 24;
            for _ in 0..8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04C1_1DB7
                } else {
                    crc << 1
                };
            }
            crc
        })
    })
}

/// Profile layout version, the first word of a profile. Firmware before
/// 4.16.3 uses A; B adds Rapid Tap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Schema {
    A,
    B,
}

impl Schema {
    pub fn from_word(word: u16) -> Option<Self> {
        match word {
            0x000A => Some(Self::A),
            0x000B => Some(Self::B),
            _ => None,
        }
    }

    /// Payload length, checksum included.
    pub fn payload_len(self) -> usize {
        match self {
            Self::A => 11_648,
            Self::B => 11_696,
        }
    }

    fn macros(self) -> usize {
        match self {
            Self::A => 2025,
            Self::B => 2076,
        }
    }

    fn guid(self) -> usize {
        self.macros() + MACROS_LEN
    }
}

/// Which layer of key mappings: the normal one, or the one active while the
/// META key is held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    #[default]
    Base,
    Meta,
}

const NAME: usize = 2;
/// Name field length, NUL terminator included.
const NAME_LEN: usize = 24;
const LIGHTING: usize = 27;
const META_KEY: usize = 84;
const MAPPINGS: usize = 85;
const META_MAPPINGS: usize = 735;
const MAPPING_LEN: usize = 5;
const OLED: usize = 1385;
const MACROS_LEN: usize = 9600;
const GUID_LEN: usize = 16;

/// Function byte of a mapping that sends a key or combo.
const FN_KEY: u8 = 0x51;
const FN_MEDIA: u8 = 0x61;
const FN_META: u8 = 0x62;
const FN_MACRO: u8 = 0x71;

/// GG's `device-offset-lookup-table` (via steelseriesgg-rs): the key code
/// each mapping slot belongs to, `0xFF` for none. Codes are HID usages, the
/// same as [`KeyCode`]s, except `0xF0` (the SteelSeries/fn key) and
/// `0xF1`-`0xF8`, which carry the media and wheel bindings.
const SLOT_KEYS: [u8; 128] = [
    41, 53, 43, 57, 225, 224, 255, 255, 58, 30, 20, 4, 100, 227, 72, 97, 59, 31, 26, 22, 29, 226,
    74, 87, 60, 32, 8, 7, 27, 255, 75, 92, 61, 33, 21, 9, 6, 139, 83, 93, 62, 34, 23, 10, 25, 44,
    84, 94, 63, 35, 28, 11, 5, 138, 85, 133, 64, 36, 24, 13, 17, 136, 86, 89, 65, 37, 12, 14, 16,
    255, 77, 90, 66, 38, 18, 15, 54, 230, 78, 91, 67, 39, 19, 51, 55, 231, 95, 99, 68, 45, 47, 52,
    56, 240, 96, 88, 69, 46, 48, 50, 135, 228, 255, 255, 70, 137, 49, 40, 229, 80, 255, 255, 71,
    42, 73, 76, 82, 81, 79, 98, 241, 242, 243, 244, 245, 246, 247, 248,
];

fn mapping_slot(key: KeyCode) -> Option<usize> {
    SLOT_KEYS.iter().position(|&k| k == key.0 && k != 0xFF)
}

/// One key mapping: a function byte and four codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mapping {
    pub function: u8,
    pub codes: [u8; 4],
}

impl Mapping {
    /// What GG ships for a key in `layer`: the base layer binds every key to
    /// itself, the META layer leaves keys unbound. Both type the key.
    pub const fn stock(layer: Layer, key: KeyCode) -> Self {
        match layer {
            Layer::Base => Self::key(key),
            Layer::Meta => Self {
                function: 0,
                codes: [0; 4],
            },
        }
    }

    /// Send `key` instead.
    pub const fn key(key: KeyCode) -> Self {
        Self {
            function: FN_KEY,
            codes: [key.0, 0, 0, 0],
        }
    }

    /// What this mapping does on `own`, the key it belongs to.
    pub fn action(&self, own: KeyCode) -> Action {
        let keys = || {
            self.codes
                .iter()
                .filter(|&&c| c != 0)
                .map(|&c| KeyCode(c))
                .collect::<Vec<_>>()
        };
        match self.function {
            // GG binds every key to itself by default and leaves unused
            // slots all zero; both type the key as usual.
            0 if self.codes == [0; 4] => Action::Default,
            FN_KEY if self.codes == [own.0, 0, 0, 0] => Action::Default,
            FN_KEY => match keys().as_slice() {
                [k] => Action::Key { key: *k },
                ks => Action::Combo { keys: ks.to_vec() },
            },
            b @ 0x01..=0x08 => Action::Mouse { button: b },
            FN_MEDIA => Action::Media {
                codes: hex(&self.codes),
            },
            FN_META => Action::Meta {
                codes: hex(&self.codes),
            },
            FN_MACRO => Action::Macro {
                index: self.codes[2],
            },
            function => Action::Other {
                function,
                codes: hex(&self.codes),
            },
        }
    }
}

/// A decoded [`Mapping`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// The key types itself.
    Default,
    Key {
        key: KeyCode,
    },
    Combo {
        keys: Vec<KeyCode>,
    },
    Mouse {
        button: u8,
    },
    Media {
        codes: String,
    },
    /// A META function (layer switch or a keyboard feature).
    Meta {
        codes: String,
    },
    /// Plays a macro; `index` is GG's raw binding byte.
    Macro {
        index: u8,
    },
    Other {
        function: u8,
        codes: String,
    },
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    #[error("the slot is empty (no saved profile)")]
    Empty,
    #[error("unknown profile schema 0x{0:04x}")]
    UnknownSchema(u16),
    #[error("profile is {got} bytes, its schema needs {want}")]
    Short { got: usize, want: usize },
    #[error("{0}")]
    Invalid(String),
}

/// One slot's payload. Edits change only the bytes they are about and
/// recompute the checksum, so everything apexkb does not understand (macros,
/// GUID, Rapid Tap, unknown bytes) is kept as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    bytes: Vec<u8>,
    schema: Schema,
}

impl Profile {
    /// Parse a payload read from a slot. Bytes past the schema's length are
    /// dropped. The checksum is not checked here; see [`Self::checksum_ok`].
    pub fn parse(bytes: &[u8]) -> Result<Self, ProfileError> {
        let word = u16::from_le_bytes([*bytes.first().unwrap_or(&0), *bytes.get(1).unwrap_or(&0)]);
        let schema = match Schema::from_word(word) {
            Some(s) => s,
            None if word == 0 || word == 0xFFFF => return Err(ProfileError::Empty),
            None => return Err(ProfileError::UnknownSchema(word)),
        };
        if bytes.len() < schema.payload_len() {
            return Err(ProfileError::Short {
                got: bytes.len(),
                want: schema.payload_len(),
            });
        }
        Ok(Self {
            bytes: bytes[..schema.payload_len()].to_vec(),
            schema,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn schema(&self) -> Schema {
        self.schema
    }

    fn checksum_at(&self) -> usize {
        self.bytes.len() - 4
    }

    pub fn stored_checksum(&self) -> u32 {
        let at = self.checksum_at();
        u32::from_le_bytes(self.bytes[at..at + 4].try_into().unwrap())
    }

    pub fn checksum(&self) -> u32 {
        crc32(&self.bytes[..self.checksum_at()])
    }

    pub fn checksum_ok(&self) -> bool {
        self.stored_checksum() == self.checksum()
    }

    fn seal(&mut self) {
        let at = self.checksum_at();
        let crc = self.checksum();
        self.bytes[at..at + 4].copy_from_slice(&crc.to_le_bytes());
    }

    pub fn name(&self) -> String {
        let field = &self.bytes[NAME..NAME + NAME_LEN];
        let end = field.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        String::from_utf8_lossy(&field[..end]).into_owned()
    }

    /// Rename the profile: printable ASCII, at most 23 characters.
    pub fn set_name(&mut self, name: &str) -> Result<(), ProfileError> {
        if name.is_empty()
            || name.len() >= NAME_LEN
            || !name.bytes().all(|b| (0x20..0x7F).contains(&b))
        {
            return Err(ProfileError::Invalid(format!(
                "a profile name is 1-{} printable ASCII characters",
                NAME_LEN - 1
            )));
        }
        let field = &mut self.bytes[NAME..NAME + NAME_LEN];
        field.fill(0);
        field[..name.len()].copy_from_slice(name.as_bytes());
        self.seal();
        Ok(())
    }

    fn table(layer: Layer) -> usize {
        match layer {
            Layer::Base => MAPPINGS,
            Layer::Meta => META_MAPPINGS,
        }
    }

    fn mapping_at(&self, layer: Layer, slot: usize) -> Mapping {
        let at = Self::table(layer) + slot * MAPPING_LEN;
        let b = &self.bytes[at..at + MAPPING_LEN];
        Mapping {
            function: b[0],
            codes: [b[1], b[2], b[3], b[4]],
        }
    }

    /// The mapping of `key`, or `None` if the profile has no slot for it.
    pub fn mapping(&self, layer: Layer, key: KeyCode) -> Option<Mapping> {
        mapping_slot(key).map(|s| self.mapping_at(layer, s))
    }

    pub fn set_mapping(
        &mut self,
        layer: Layer,
        key: KeyCode,
        mapping: Mapping,
    ) -> Result<(), ProfileError> {
        let slot = mapping_slot(key).ok_or_else(|| {
            ProfileError::Invalid(format!("{key} cannot be remapped in an onboard profile"))
        })?;
        let at = Self::table(layer) + slot * MAPPING_LEN;
        self.bytes[at] = mapping.function;
        self.bytes[at + 1..at + MAPPING_LEN].copy_from_slice(&mapping.codes);
        self.seal();
        Ok(())
    }

    /// Every key of `layer` that does something other than type itself.
    /// Stock profiles already bind the media controls and some META
    /// functions this way.
    pub fn mappings(&self, layer: Layer) -> Vec<KeyMapping> {
        SLOT_KEYS
            .iter()
            .enumerate()
            .filter(|&(_, &k)| k != 0xFF)
            .filter_map(|(slot, &k)| {
                let key = KeyCode(k);
                let action = self.mapping_at(layer, slot).action(key);
                (action != Action::Default).then_some(KeyMapping { key, layer, action })
            })
            .collect()
    }

    /// The key that switches to the META layer.
    pub fn meta_key(&self) -> Option<KeyCode> {
        Some(self.bytes[META_KEY]).filter(|&k| k != 0).map(KeyCode)
    }

    pub fn oled(&self) -> Frame {
        Frame::from_bytes(&self.bytes[OLED..OLED + Frame::BYTES]).expect("OLED block size")
    }

    pub fn set_oled(&mut self, frame: &Frame) {
        self.bytes[OLED..OLED + Frame::BYTES].copy_from_slice(frame.as_bytes());
        self.seal();
    }

    fn rgb(&self, at: usize) -> Rgb {
        Rgb::new(self.bytes[at], self.bytes[at + 1], self.bytes[at + 2])
    }

    pub fn lighting(&self) -> LightingOptions {
        let b = &self.bytes;
        LightingOptions {
            lock_key_color: self.rgb(LIGHTING + 6),
            meta_key_color: self.rgb(LIGHTING + 9),
            disabled_win_key_color: self.rgb(LIGHTING + 12),
            meta_key_dimmer: b[LIGHTING + 51],
            idle_dimmer: b[LIGHTING + 52],
            idle_timeout_ms: u32::from_le_bytes(
                b[LIGHTING + 53..LIGHTING + 57].try_into().unwrap(),
            ),
        }
    }

    /// Bytes of the macro block in use (not zero).
    pub fn macro_bytes(&self) -> usize {
        let at = self.schema.macros();
        self.bytes[at..at + MACROS_LEN]
            .iter()
            .filter(|&&b| b != 0)
            .count()
    }

    pub fn guid(&self) -> String {
        let at = self.schema.guid();
        hex(&self.bytes[at..at + GUID_LEN])
    }

    pub fn summary(&self) -> ProfileSummary {
        let mut mappings = self.mappings(Layer::Base);
        mappings.extend(self.mappings(Layer::Meta));
        ProfileSummary {
            name: self.name(),
            schema: self.schema,
            checksum_ok: self.checksum_ok(),
            meta_key: self.meta_key(),
            mappings,
            lighting: self.lighting(),
            oled: self.oled(),
            macro_bytes: self.macro_bytes(),
            guid: self.guid(),
        }
    }
}

/// A key that does something other than type itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KeyMapping {
    pub key: KeyCode,
    pub layer: Layer,
    pub action: Action,
}

/// The lighting options a profile stores. Per-key lighting is not among
/// them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LightingOptions {
    pub lock_key_color: Rgb,
    pub meta_key_color: Rgb,
    pub disabled_win_key_color: Rgb,
    pub meta_key_dimmer: u8,
    pub idle_dimmer: u8,
    pub idle_timeout_ms: u32,
}

/// What a slot holds, for the API.
#[derive(Clone, Debug, Serialize)]
pub struct ProfileSummary {
    pub name: String,
    pub schema: Schema,
    pub checksum_ok: bool,
    pub meta_key: Option<KeyCode>,
    pub mappings: Vec<KeyMapping>,
    pub lighting: LightingOptions,
    pub oled: Frame,
    pub macro_bytes: usize,
    pub guid: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A schema A profile with every base key bound to itself and the META
    /// layer unbound, as GG ships it.
    fn stock() -> Profile {
        let mut b = vec![0u8; Schema::A.payload_len()];
        b[0] = 0x0A;
        b[NAME..NAME + 8].copy_from_slice(b"Config 1");
        b[META_KEY] = 0xF0;
        for (slot, &k) in SLOT_KEYS.iter().enumerate() {
            if k != 0xFF {
                let at = MAPPINGS + slot * MAPPING_LEN;
                b[at] = FN_KEY;
                b[at + 1] = k;
            }
        }
        let mut p = Profile::parse(&b).unwrap();
        p.seal();
        p
    }

    #[test]
    fn crc_matches_stm32() {
        // STM32 CRC unit, one word 0x00000000 and the classic check value
        // of CRC-32/MPEG-2 over "123456789" padded to whole words.
        assert_eq!(crc32(&[0, 0, 0, 0]), 0xC704_DD7B);
        assert_eq!(crc32(&[]), 0xFFFF_FFFF);
    }

    #[test]
    fn frames() {
        assert_eq!(
            &pull(0x0201, 1024, 1024)[..12],
            &[0, 0x85, 0, 0x01, 0x02, 0, 4, 0, 0, 0, 4, 0]
        );
        let r = read_request(512, 512);
        assert_eq!(r.len(), 643);
        assert_eq!(&r[..7], &[0, 0x83, 0, 0, 2, 0, 2]);
        assert_eq!(&open(0x0203)[..5], &[0, 0x88, 0, 0x03, 0x02]);
        let w = buffer_write(512, &[9; 512]);
        assert_eq!(w.len(), 643);
        assert_eq!(&w[..8], &[0, 0x03, 0, 0, 2, 0, 2, 9]);
        assert_eq!(
            &push(0x0201, 39 * 1024)[..11],
            &[0, 0x05, 0, 0x01, 0x02, 0, 0x9C, 0, 0, 0, 4]
        );
        assert_eq!(&commit(0)[..3], &[0, 0xEB, 1]);
        assert_eq!(&select(4)[..3], &[0, 0x89, 5]);
    }

    #[test]
    fn write_plan_is_ggs_sequence() {
        let p = stock();
        let plan = write_plan(2, p.bytes());
        // open, 40 x (2 buffer writes + push), commit
        assert_eq!(plan.len(), 122);
        assert_eq!(
            plan[0],
            WriteStep::Output {
                report: open(0x0203),
                ack: Some(CMD_OPEN)
            }
        );
        assert_eq!(
            plan[1],
            WriteStep::Feature(buffer_write(0, &p.bytes()[..512]))
        );
        assert_eq!(
            plan[2],
            WriteStep::Feature(buffer_write(512, &p.bytes()[512..1024]))
        );
        assert_eq!(
            plan[3],
            WriteStep::Output {
                report: push(0x0203, 0),
                ack: Some(CMD_PUSH)
            }
        );
        // Past the payload, the region is padded with zeros.
        assert_eq!(plan[118], WriteStep::Feature(buffer_write(0, &[0; 512])));
        assert_eq!(
            plan[121],
            WriteStep::Output {
                report: commit(2),
                ack: None
            }
        );
    }

    #[test]
    fn parse_rejects_empty_and_unknown() {
        assert_eq!(Profile::parse(&[0; 12288]), Err(ProfileError::Empty));
        assert_eq!(Profile::parse(&[0xFF; 12288]), Err(ProfileError::Empty));
        assert_eq!(
            Profile::parse(&[0x0C, 0, 0, 0]),
            Err(ProfileError::UnknownSchema(0x0C))
        );
        assert!(matches!(
            Profile::parse(&[0x0B, 0, 0, 0]),
            Err(ProfileError::Short { want: 11_696, .. })
        ));
    }

    #[test]
    fn stock_profile_decodes() {
        let p = stock();
        assert!(p.checksum_ok());
        assert_eq!(p.name(), "Config 1");
        assert_eq!(p.meta_key(), Some(KeyCode(0xF0)));
        assert!(p.mappings(Layer::Base).is_empty());
        assert!(p.mappings(Layer::Meta).is_empty());
    }

    #[test]
    fn edits_touch_only_their_bytes() {
        let before = stock();
        let mut p = before.clone();
        let caps = KeyCode(0x39);
        let esc = KeyCode(0x29);
        p.set_mapping(Layer::Meta, caps, Mapping::key(esc)).unwrap();
        assert!(p.checksum_ok());
        assert_eq!(p.mapping(Layer::Meta, caps), Some(Mapping::key(esc)));
        assert_eq!(p.mapping(Layer::Base, caps), Some(Mapping::key(caps)));
        assert_eq!(
            p.mappings(Layer::Meta),
            vec![KeyMapping {
                key: caps,
                layer: Layer::Meta,
                action: Action::Key { key: esc }
            }]
        );
        let changed: Vec<usize> = (0..p.bytes().len() - 4)
            .filter(|&i| p.bytes()[i] != before.bytes()[i])
            .collect();
        // Caps lock is slot 3 of the META table; its function byte and
        // first code change.
        let at = META_MAPPINGS + 3 * MAPPING_LEN;
        assert_eq!(changed, vec![at, at + 1]);

        // Back to stock restores the original bytes.
        p.set_mapping(Layer::Meta, caps, Mapping::stock(Layer::Meta, caps))
            .unwrap();
        assert_eq!(p, before);
    }

    #[test]
    fn name_and_oled() {
        let mut p = stock();
        p.set_name("Gaming").unwrap();
        assert_eq!(p.name(), "Gaming");
        assert!(p.set_name(&"x".repeat(24)).is_err());
        assert!(p.set_name("").is_err());
        assert!(p.set_name("caf\u{e9}").is_err());

        let mut f = Frame::new();
        f.set(0, 0, true);
        p.set_oled(&f);
        assert_eq!(p.oled(), f);
        assert_eq!(p.bytes()[OLED], 0x80);
        assert!(p.checksum_ok());
    }

    #[test]
    fn keys_without_a_slot_are_refused() {
        let mut p = stock();
        // 0x65 (application) has no mapping slot.
        assert!(
            p.set_mapping(Layer::Base, KeyCode(0x65), Mapping::key(KeyCode(4)))
                .is_err()
        );
        assert_eq!(p.mapping(Layer::Base, KeyCode(0x65)), None);
    }

    #[test]
    fn actions() {
        let a = KeyCode(4);
        let m = |function, codes| Mapping { function, codes };
        assert_eq!(m(0, [0; 4]).action(a), Action::Default);
        assert_eq!(m(0x51, [4, 0, 0, 0]).action(a), Action::Default);
        assert_eq!(
            m(0x51, [5, 0, 0, 0]).action(a),
            Action::Key { key: KeyCode(5) }
        );
        assert_eq!(
            m(0x51, [0xE0, 6, 0, 0]).action(a),
            Action::Combo {
                keys: vec![KeyCode(0xE0), KeyCode(6)]
            }
        );
        assert_eq!(m(0x02, [0; 4]).action(a), Action::Mouse { button: 2 });
        assert_eq!(
            m(0x62, [1, 0, 0, 0]).action(a),
            Action::Meta {
                codes: "01000000".into()
            }
        );
        assert_eq!(
            m(0x71, [0, 0, 14, 0]).action(a),
            Action::Macro { index: 14 }
        );
    }
}
