//! Key presses read from the keyboard's own HID interfaces, for reactive
//! lighting.
//!
//! hidraw hands a copy of every input report to each open handle while the
//! kernel keeps handling typing, so reading here grabs nothing. Two
//! interfaces carry key state, both without report IDs:
//!
//! - interface 0, the boot keyboard: 8 bytes, a modifier bitmap (usages
//!   `0xE0`-`0xE7`), a reserved byte and 6 key slots;
//! - interface 2, N-key rollover: 32 bytes, one bit per usage `0x00`-`0xFF`,
//!   least significant bit first.
//!
//! The key state is the union of both, so a key reported on either one
//! counts once. LED codes are HID usages (see [`crate::keys`]), so a usage
//! maps to the LED code with the same value.

use std::{
    fs::File,
    io::{self, Read},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

use crate::{hidraw, keys::KeyCode};

/// USB interface of the 6-key-rollover boot keyboard.
pub const BOOT_INTERFACE: u8 = 0;
/// USB interface of the N-key-rollover bitmap keyboard.
pub const NKRO_INTERFACE: u8 = 2;

const BOOT_LEN: usize = 8;
const NKRO_LEN: usize = 32;
/// Usage a boot report repeats in every slot when too many keys are held.
const ERROR_ROLLOVER: u8 = 0x01;
/// Usages `0x00`-`0x03` are status codes, not keys.
const FIRST_KEY: u8 = 0x04;

/// A set of HID keyboard usages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeySet([u64; 4]);

impl KeySet {
    pub fn insert(&mut self, usage: u8) {
        if usage >= FIRST_KEY {
            self.0[usage as usize / 64] |= 1 << (usage % 64);
        }
    }

    pub fn contains(&self, usage: u8) -> bool {
        self.0[usage as usize / 64] & (1 << (usage % 64)) != 0
    }

    pub fn union(&self, other: &KeySet) -> KeySet {
        KeySet(std::array::from_fn(|i| self.0[i] | other.0[i]))
    }

    pub fn iter(&self) -> impl Iterator<Item = u8> + '_ {
        (0..=255u8).filter(|&u| self.contains(u))
    }
}

/// Keys held according to a boot keyboard report. `None` for a report that
/// is too short, or a rollover error that says nothing about which keys are
/// held.
pub fn parse_boot(report: &[u8]) -> Option<KeySet> {
    let report = report.get(..BOOT_LEN)?;
    let slots = &report[2..];
    if slots.contains(&ERROR_ROLLOVER) {
        return None;
    }
    let mut set = KeySet::default();
    for bit in 0..8 {
        if report[0] & (1 << bit) != 0 {
            set.insert(0xE0 + bit);
        }
    }
    for &usage in slots {
        set.insert(usage);
    }
    Some(set)
}

/// Keys held according to an N-key rollover bitmap report.
pub fn parse_nkro(report: &[u8]) -> Option<KeySet> {
    let report = report.get(..NKRO_LEN)?;
    let mut set = KeySet::default();
    for (i, byte) in report.iter().enumerate() {
        for bit in 0..8 {
            if byte & (1 << bit) != 0 {
                set.insert((i * 8 + bit) as u8);
            }
        }
    }
    Some(set)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEvent {
    Press(KeyCode),
    Release(KeyCode),
}

/// Append the events that turn `old` into `new`.
pub fn diff(old: &KeySet, new: &KeySet, events: &mut Vec<KeyEvent>) {
    events.extend(
        old.iter()
            .filter(|&u| !new.contains(u))
            .map(|u| KeyEvent::Release(KeyCode(u))),
    );
    events.extend(
        new.iter()
            .filter(|&u| !old.contains(u))
            .map(|u| KeyEvent::Press(KeyCode(u))),
    );
}

struct Node {
    file: File,
    parse: fn(&[u8]) -> Option<KeySet>,
    held: KeySet,
}

/// The key input interfaces of one keyboard, opened read-only.
pub struct KeyInput {
    control: PathBuf,
    nodes: Vec<Node>,
    held: KeySet,
}

impl KeyInput {
    /// Open the input interfaces of the keyboard whose control interface is
    /// `control` (a `/dev/hidrawN` path, see [`crate::DeviceInfo::hidraw`]).
    pub fn open(control: &Path) -> io::Result<Self> {
        let all = hidraw::enumerate()?;
        let usb = all
            .iter()
            .find(|h| h.path == control)
            .and_then(|h| h.usb_device.clone())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{} is not a USB keyboard", control.display()),
                )
            })?;
        let mut nodes = Vec::new();
        for h in all.iter().filter(|h| h.usb_device.as_ref() == Some(&usb)) {
            let parse = match h.interface {
                Some(BOOT_INTERFACE) => parse_boot,
                Some(NKRO_INTERFACE) => parse_nkro,
                _ => continue,
            };
            nodes.push(Node {
                file: File::open(&h.path)?,
                parse,
                held: KeySet::default(),
            });
        }
        if nodes.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the keyboard has no key input interface",
            ));
        }
        Ok(Self {
            control: control.to_path_buf(),
            nodes,
            held: KeySet::default(),
        })
    }

    /// The control interface this input belongs to.
    pub fn control(&self) -> &Path {
        &self.control
    }

    /// Wait up to `timeout_ms` for input reports and append the presses and
    /// releases they contain. An error means the keyboard is gone.
    pub fn read(&mut self, timeout_ms: i32, events: &mut Vec<KeyEvent>) -> io::Result<()> {
        let mut fds: Vec<libc::pollfd> = self
            .nodes
            .iter()
            .map(|n| libc::pollfd {
                fd: n.file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        // SAFETY: `fds` is a valid array of `fds.len()` pollfds.
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout_ms) };
        if rc < 0 {
            let e = io::Error::last_os_error();
            return match e.kind() {
                io::ErrorKind::Interrupted => Ok(()),
                _ => Err(e),
            };
        }
        let mut buf = [0u8; 64];
        for (node, pfd) in self.nodes.iter_mut().zip(&fds) {
            if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "keyboard input closed",
                ));
            }
            if pfd.revents & libc::POLLIN != 0 {
                let n = node.file.read(&mut buf)?;
                if let Some(set) = (node.parse)(&buf[..n]) {
                    node.held = set;
                }
            }
        }
        let held = self
            .nodes
            .iter()
            .fold(KeySet::default(), |acc, n| acc.union(&n.held));
        diff(&self.held, &held, events);
        self.held = held;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_report() {
        // Left shift + right alt, then A and Backspace.
        let set = parse_boot(&[0b0100_0010, 0, 0x04, 0x2A, 0, 0, 0, 0]).unwrap();
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![0x04, 0x2A, 0xE1, 0xE6]);
        assert!(parse_boot(&[0, 0, 1, 1, 1, 1, 1, 1]).is_none());
        assert!(parse_boot(&[0, 0, 4]).is_none());
    }

    #[test]
    fn nkro_report() {
        let mut r = [0u8; 32];
        r[0] |= 1 << 4; // usage 0x04
        r[0xE5 / 8] |= 1 << (0xE5 % 8);
        r[0] |= 0b11; // status usages are not keys
        let set = parse_nkro(&r).unwrap();
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![0x04, 0xE5]);
    }

    #[test]
    fn diff_reports_changes() {
        let a = parse_boot(&[0, 0, 0x04, 0x05, 0, 0, 0, 0]).unwrap();
        let b = parse_boot(&[0, 0, 0x05, 0x06, 0, 0, 0, 0]).unwrap();
        let mut ev = Vec::new();
        diff(&a, &b, &mut ev);
        assert_eq!(
            ev,
            vec![
                KeyEvent::Release(KeyCode(0x04)),
                KeyEvent::Press(KeyCode(0x06))
            ]
        );
        ev.clear();
        diff(&a, &a.union(&b), &mut ev);
        assert_eq!(ev, vec![KeyEvent::Press(KeyCode(0x06))]);
    }
}
