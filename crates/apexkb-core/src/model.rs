use serde::Serialize;

use crate::keys::FormFactor;

pub const STEELSERIES_VID: u16 = 0x1038;

/// A keyboard model apexkb knows how to drive.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Model {
    pub pid: u16,
    pub name: &'static str,
    pub form: FormFactor,
    pub oled: bool,
    /// Confirmed working on real hardware. Untested models share the same
    /// (gen 1) protocol and are expected to work.
    pub tested: bool,
}

pub static MODELS: &[Model] = &[
    Model {
        pid: 0x1618,
        name: "Apex 7 TKL",
        form: FormFactor::Tkl,
        oled: true,
        tested: true,
    },
    Model {
        pid: 0x1612,
        name: "Apex 7",
        form: FormFactor::Full,
        oled: true,
        tested: false,
    },
    Model {
        pid: 0x1610,
        name: "Apex Pro",
        form: FormFactor::Full,
        oled: true,
        tested: false,
    },
    Model {
        pid: 0x1614,
        name: "Apex Pro TKL",
        form: FormFactor::Tkl,
        oled: true,
        tested: false,
    },
    Model {
        pid: 0x161C,
        name: "Apex 5",
        form: FormFactor::Full,
        oled: true,
        tested: false,
    },
];

pub fn by_pid(pid: u16) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.pid == pid)
}
