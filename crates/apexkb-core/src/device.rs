use std::{io, path::PathBuf};

use serde::Serialize;

use crate::{
    color::Rgb,
    hidraw::{self, HidRaw},
    keys::{FormFactor, KeyCode},
    model::{self, Model, STEELSERIES_VID},
    oled::Frame,
    protocol,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no supported SteelSeries Apex keyboard found")]
    NotFound,
    #[error(
        "permission denied opening {0}; install packaging/70-apexkb.rules into /etc/udev/rules.d and replug the keyboard"
    )]
    Permission(PathBuf),
    #[error("keyboard I/O failed: {0}")]
    Io(#[from] io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Static facts about a connected keyboard.
#[derive(Clone, Debug, Serialize)]
pub struct DeviceInfo {
    pub model: &'static str,
    pub pid: String,
    pub form_factor: FormFactor,
    pub oled: bool,
    pub tested: bool,
    pub firmware: Option<String>,
    pub serial: Option<String>,
    pub hidraw: PathBuf,
}

/// A connected Apex keyboard, opened on its control interface.
#[derive(Debug)]
pub struct Keyboard {
    model: &'static Model,
    control: HidRaw,
    info: DeviceInfo,
}

impl Keyboard {
    /// Supported keyboards currently plugged in, as `(model, control node)`.
    pub fn list() -> io::Result<Vec<(&'static Model, PathBuf)>> {
        Ok(hidraw::enumerate()?
            .into_iter()
            .filter(|h| {
                h.vid == STEELSERIES_VID && h.interface == Some(protocol::CONTROL_INTERFACE)
            })
            .filter_map(|h| model::by_pid(h.pid).map(|m| (m, h.path)))
            .collect())
    }

    /// Open the first supported keyboard.
    pub fn open() -> Result<Self> {
        let (model, path) = Self::list()?.into_iter().next().ok_or(Error::NotFound)?;
        Self::open_path(model, path)
    }

    pub fn open_path(model: &'static Model, path: PathBuf) -> Result<Self> {
        let control = HidRaw::open(&path).map_err(|e| match e.kind() {
            io::ErrorKind::PermissionDenied => Error::Permission(path.clone()),
            _ => Error::Io(e),
        })?;
        let mut kb = Self {
            model,
            control,
            info: DeviceInfo {
                model: model.name,
                pid: format!("{:04x}", model.pid),
                form_factor: model.form,
                oled: model.oled,
                tested: model.tested,
                firmware: None,
                serial: None,
                hidraw: path,
            },
        };
        kb.info.firmware = kb
            .query(protocol::CMD_FIRMWARE)?
            .map(|r| protocol::ascii_reply(&r));
        kb.info.serial = kb
            .query(protocol::CMD_SERIAL)?
            .map(|r| protocol::ascii_reply(&r[..19]));
        Ok(kb)
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn model(&self) -> &'static Model {
        self.model
    }

    /// Send a query command and wait briefly for its reply.
    pub fn query(&mut self, cmd: u8) -> Result<Option<[u8; protocol::OUTPUT_LEN]>> {
        self.control.drain()?;
        self.control.write(&protocol::command(cmd))?;
        let mut reply = [0u8; protocol::OUTPUT_LEN];
        Ok(self.control.read_timeout(&mut reply, 200)?.map(|_| reply))
    }

    /// Set the given keys to the given colors. Keys not listed keep their
    /// current color. Switches the keyboard out of onboard lighting.
    pub fn set_colors(&mut self, colors: &[(KeyCode, Rgb)]) -> Result<()> {
        for packet in protocol::direct_colors(colors) {
            self.control.send_feature(&packet)?;
        }
        Ok(())
    }

    /// Return lighting control to the keyboard's onboard profile.
    pub fn onboard_lighting(&mut self) -> Result<()> {
        self.control
            .write(&protocol::command(protocol::CMD_ONBOARD))?;
        Ok(())
    }

    pub fn draw_oled(&mut self, frame: &Frame) -> Result<()> {
        self.control.send_feature(&protocol::oled(frame))?;
        Ok(())
    }
}
