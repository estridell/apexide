use std::{
    io,
    path::PathBuf,
    thread::sleep,
    time::{Duration, Instant},
};

use serde::Serialize;

use crate::{
    color::Rgb,
    hidraw::{self, HidRaw},
    keys::{FormFactor, KeyCode},
    model::{self, Model, STEELSERIES_VID},
    oled::Frame,
    onboard::{self, Profile, ProfileError, WriteStep},
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
    #[error("onboard profile: {0}")]
    Onboard(String),
    #[error(transparent)]
    Profile(#[from] ProfileError),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Pause after each onboard frame, as GG paces them.
const FRAME_GAP: Duration = Duration::from_millis(5);
/// Wait between asking for a buffer read and reading the answer; reading
/// right away can return the buffer before the keyboard has filled it.
const READ_SETTLE: Duration = Duration::from_millis(2);
/// Longest wait for the answer to an open or push. GG's capture shows the
/// open answered after about 300 ms.
const ACK_TIMEOUT: Duration = Duration::from_secs(2);
/// Longest wait for the answer to a pull. Some boards answer `85 01`, others
/// (the Apex 7 in steelseriesgg-rs' capture) only send an input report.
const PULL_TIMEOUT: Duration = Duration::from_millis(200);
/// Wait after a commit before switching slots. GG waits 250 ms, and a switch
/// sent right after a commit can be ignored.
const COMMIT_SETTLE: Duration = Duration::from_millis(250);

fn check_slot(slot: u8) -> Result<()> {
    if slot >= onboard::SLOTS {
        return Err(Error::Onboard(format!(
            "slot {} does not exist (1-{})",
            slot + 1,
            onboard::SLOTS
        )));
    }
    Ok(())
}

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

    /// Read the first `len` bytes of an onboard file. Changes nothing on
    /// the keyboard.
    pub fn read_onboard_file(&mut self, file: u16, len: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(len.next_multiple_of(onboard::BUFFER_LEN));
        for offset in (0..len).step_by(onboard::BUFFER_LEN) {
            let pull = onboard::pull(file, offset as u32, onboard::BUFFER_LEN as u16);
            self.onboard_output(&pull, onboard::CMD_PULL, PULL_TIMEOUT, false)?;
            for chunk in (0..onboard::BUFFER_LEN).step_by(onboard::CHUNK_LEN) {
                let request = onboard::read_request(chunk as u16, onboard::CHUNK_LEN as u16);
                self.control.send_feature(&request)?;
                sleep(READ_SETTLE);
                let mut reply = [0u8; protocol::FEATURE_LEN + 1];
                let n = self.control.get_feature(&mut reply)?;
                if n < 1 + onboard::CHUNK_LEN {
                    return Err(Error::Onboard(format!("buffer read returned {n} bytes")));
                }
                out.extend_from_slice(&reply[1..1 + onboard::CHUNK_LEN]);
                sleep(FRAME_GAP);
            }
        }
        out.truncate(len);
        Ok(out)
    }

    /// Read and parse the profile in `slot` (0-4). Changes nothing on the
    /// keyboard.
    pub fn read_profile(&mut self, slot: u8) -> Result<Profile> {
        check_slot(slot)?;
        let bytes = self.read_onboard_file(
            onboard::profile_file(slot),
            onboard::Schema::B.payload_len(),
        )?;
        Ok(Profile::parse(&bytes)?)
    }

    /// Store `profile` in `slot` (0-4), then switch the keyboard to that
    /// slot like GG does. Nothing reads the write back; read the slot again
    /// to verify it.
    pub fn write_profile(&mut self, slot: u8, profile: &Profile) -> Result<()> {
        check_slot(slot)?;
        if !profile.checksum_ok() {
            return Err(Error::Onboard(
                "refusing to write a profile whose checksum does not match".into(),
            ));
        }
        for step in onboard::write_plan(slot, profile.bytes()) {
            match step {
                WriteStep::Output { report, ack } => match ack {
                    Some(cmd) => self.onboard_output(&report, cmd, ACK_TIMEOUT, true)?,
                    None => {
                        self.control.write(&report)?;
                        sleep(FRAME_GAP);
                    }
                },
                WriteStep::Feature(report) => {
                    self.control.send_feature(&report)?;
                    sleep(FRAME_GAP);
                }
            }
        }
        sleep(COMMIT_SETTLE);
        self.select_profile(slot)
    }

    /// Make the keyboard run the profile in `slot` (0-4). Stored data is not
    /// touched.
    pub fn select_profile(&mut self, slot: u8) -> Result<()> {
        check_slot(slot)?;
        self.control.write(&onboard::select(slot))?;
        Ok(())
    }

    /// Send an onboard output frame and wait for the answer that starts
    /// with `cmd`. A refusal is always an error; no answer at all is one
    /// only if `required`, and otherwise any input report ends the wait.
    fn onboard_output(
        &mut self,
        report: &[u8],
        cmd: u8,
        timeout: Duration,
        required: bool,
    ) -> Result<()> {
        self.control.drain()?;
        self.control.write(report)?;
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; protocol::OUTPUT_LEN];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                if required {
                    return Err(Error::Onboard(format!(
                        "the keyboard did not answer frame 0x{cmd:02x}"
                    )));
                }
                break;
            }
            match self
                .control
                .read_timeout(&mut buf, left.as_millis() as i32 + 1)?
            {
                Some(n) if n >= 2 && buf[0] == cmd => {
                    if buf[1] != onboard::ACK_OK {
                        return Err(Error::Onboard(format!(
                            "the keyboard refused frame 0x{cmd:02x} (status 0x{:02x})",
                            buf[1]
                        )));
                    }
                    break;
                }
                Some(_) if !required => break,
                _ => {}
            }
        }
        sleep(FRAME_GAP);
        Ok(())
    }
}
