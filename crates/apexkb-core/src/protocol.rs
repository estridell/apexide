//! Packet encoders for the gen 1 Apex protocol (Apex 7, Apex 7 TKL, Apex Pro,
//! Apex 5). Everything here is pure so it can be tested without hardware.
//!
//! The control interface (USB interface 1) has no numbered reports and
//! exposes a 64-byte output report and a 642-byte feature report. Buffers
//! returned here are ready for hidraw: they start with the report number
//! byte (0) where the kernel expects one.
//!
//! Sources: OpenRGB's `SteelSeriesApexController` and apex-tux, confirmed
//! against an Apex 7 TKL running firmware 4.1.0.

use crate::{color::Rgb, keys::KeyCode, oled::Frame};

/// USB interface that accepts lighting, OLED and query commands.
pub const CONTROL_INTERFACE: u8 = 1;

/// Output report payload length (without the report number byte).
pub const OUTPUT_LEN: usize = 64;
/// Feature report payload length (without the report number byte).
pub const FEATURE_LEN: usize = 642;

/// Set per-key colors directly, bypassing the onboard lighting.
pub const CMD_DIRECT: u8 = 0x3A;
/// Hand lighting back to the keyboard's onboard profile.
pub const CMD_ONBOARD: u8 = 0x3B;
/// Draw a 128x40 bitmap on the OLED.
pub const CMD_OLED: u8 = 0x61;
/// Query firmware version (reply: ASCII, NUL padded).
pub const CMD_FIRMWARE: u8 = 0x90;
/// Query serial number (reply: 19 ASCII bytes).
pub const CMD_SERIAL: u8 = 0xFF;

/// Maximum key entries per direct lighting packet.
pub const KEYS_PER_PACKET: usize = (FEATURE_LEN - 2) / 4;

/// A 64-byte output report carrying a bare command, prefixed with report 0.
pub fn command(cmd: u8) -> [u8; OUTPUT_LEN + 1] {
    let mut buf = [0u8; OUTPUT_LEN + 1];
    buf[1] = cmd;
    buf
}

/// Direct lighting feature reports. Keys beyond one packet's capacity are
/// split across several packets.
pub fn direct_colors(colors: &[(KeyCode, Rgb)]) -> Vec<Vec<u8>> {
    colors
        .chunks(KEYS_PER_PACKET)
        .map(|chunk| {
            let mut buf = vec![0u8; FEATURE_LEN + 1];
            buf[1] = CMD_DIRECT;
            buf[2] = chunk.len() as u8;
            for (i, (key, c)) in chunk.iter().enumerate() {
                let o = 3 + i * 4;
                buf[o..o + 4].copy_from_slice(&[key.0, c.r, c.g, c.b]);
            }
            buf
        })
        .collect()
}

/// OLED feature report. The command byte doubles as the report number, so
/// the buffer is sent as-is without a leading 0.
pub fn oled(frame: &Frame) -> [u8; FEATURE_LEN] {
    let mut buf = [0u8; FEATURE_LEN];
    buf[0] = CMD_OLED;
    buf[1..1 + Frame::BYTES].copy_from_slice(frame.as_bytes());
    buf
}

/// Decode a NUL-padded ASCII reply.
pub fn ascii_reply(reply: &[u8]) -> String {
    let end = reply.iter().position(|&b| b == 0).unwrap_or(reply.len());
    String::from_utf8_lossy(&reply[..end]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_packet_layout() {
        let pkts = direct_colors(&[
            (KeyCode(0x04), Rgb::new(1, 2, 3)),
            (KeyCode(0x29), Rgb::new(4, 5, 6)),
        ]);
        assert_eq!(pkts.len(), 1);
        let p = &pkts[0];
        assert_eq!(p.len(), 643);
        assert_eq!(&p[..11], &[0, 0x3A, 2, 0x04, 1, 2, 3, 0x29, 4, 5, 6]);
        assert!(p[11..].iter().all(|&b| b == 0));
    }

    #[test]
    fn direct_packets_split() {
        let colors: Vec<_> = (0..200u8).map(|i| (KeyCode(i), Rgb::BLACK)).collect();
        let pkts = direct_colors(&colors);
        assert_eq!(pkts.len(), 2);
        assert_eq!(pkts[0][2] as usize, KEYS_PER_PACKET);
        assert_eq!(pkts[1][2] as usize, 200 - KEYS_PER_PACKET);
    }

    #[test]
    fn oled_packet_layout() {
        let mut f = Frame::new();
        f.set(0, 0, true);
        f.set(127, 39, true);
        let p = oled(&f);
        assert_eq!(p[0], 0x61);
        assert_eq!(p[1], 0x80);
        assert_eq!(p[640], 0x01);
        assert_eq!(p[641], 0);
    }

    #[test]
    fn ascii() {
        assert_eq!(ascii_reply(b"4.1.0\0\0\0"), "4.1.0");
    }
}
