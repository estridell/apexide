//! Minimal Linux hidraw access: sysfs enumeration plus feature/output report
//! I/O. Avoids a dependency on libudev or hidapi.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct HidInfo {
    /// `/dev/hidrawN`
    pub path: PathBuf,
    pub vid: u16,
    pub pid: u16,
    /// USB interface number, if the device sits on USB.
    pub interface: Option<u8>,
    /// sysfs path of the USB device, shared by all interfaces of one
    /// physical keyboard.
    pub usb_device: Option<PathBuf>,
    pub name: String,
}

/// List every hidraw node on the system.
pub fn enumerate() -> io::Result<Vec<HidInfo>> {
    let mut out = Vec::new();
    for entry in fs::read_dir("/sys/class/hidraw")? {
        let entry = entry?;
        if let Some(info) = describe(&entry.path()) {
            out.push(info);
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn describe(sys: &Path) -> Option<HidInfo> {
    let dev = sys.join("device");
    let uevent = fs::read_to_string(dev.join("uevent")).ok()?;
    let mut ids = None;
    let mut name = String::new();
    for line in uevent.lines() {
        if let Some(v) = line.strip_prefix("HID_ID=") {
            // bus:vendor:product, each hex
            let parts: Vec<_> = v.split(':').collect();
            if parts.len() == 3 {
                let vid = u32::from_str_radix(parts[1], 16).ok()? as u16;
                let pid = u32::from_str_radix(parts[2], 16).ok()? as u16;
                ids = Some((vid, pid));
            }
        } else if let Some(v) = line.strip_prefix("HID_NAME=") {
            name = v.to_string();
        }
    }
    let (vid, pid) = ids?;
    // .../<usb device>/<usb interface>/<hid device>/hidraw/hidrawN; the
    // interface number lives on the USB interface directory above the HID
    // device.
    let usb_interface = fs::canonicalize(&dev)
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let interface = usb_interface
        .as_ref()
        .and_then(|p| fs::read_to_string(p.join("bInterfaceNumber")).ok())
        .and_then(|s| u8::from_str_radix(s.trim(), 16).ok());
    let usb_device = interface
        .and(usb_interface)
        .and_then(|p| p.parent().map(Path::to_path_buf));
    Some(HidInfo {
        path: Path::new("/dev").join(sys.file_name()?),
        vid,
        pid,
        interface,
        usb_device,
        name,
    })
}

/// An open hidraw node.
#[derive(Debug)]
pub struct HidRaw {
    file: File,
}

const IOC_READ_WRITE: libc::c_ulong = 3;

const fn hid_ioc(nr: libc::c_ulong, len: usize) -> libc::c_ulong {
    (IOC_READ_WRITE << 30) | ((len as libc::c_ulong) << 16) | ((b'H' as libc::c_ulong) << 8) | nr
}

impl HidRaw {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Self { file })
    }

    /// Send an output report. `buf[0]` is the report number (0 if unused).
    pub fn write(&mut self, buf: &[u8]) -> io::Result<()> {
        let n = self.file.write(buf)?;
        if n != buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short hidraw write",
            ));
        }
        Ok(())
    }

    /// Send a feature report. `buf[0]` is the report number (0 if unused).
    pub fn send_feature(&mut self, buf: &[u8]) -> io::Result<()> {
        // SAFETY: HIDIOCSFEATURE reads `buf.len()` bytes from the pointer,
        // which the length encoded in the request matches.
        let rc = unsafe {
            libc::ioctl(
                self.file.as_raw_fd(),
                hid_ioc(0x06, buf.len()) as _,
                buf.as_ptr(),
            )
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Read one input report, waiting at most `timeout_ms`.
    pub fn read_timeout(&mut self, buf: &mut [u8], timeout_ms: i32) -> io::Result<Option<usize>> {
        let mut pfd = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        match rc {
            0 => Ok(None),
            n if n < 0 => Err(io::Error::last_os_error()),
            _ => self.file.read(buf).map(Some),
        }
    }

    /// Discard any queued input reports.
    pub fn drain(&mut self) -> io::Result<()> {
        let mut buf = [0u8; 64];
        while self.read_timeout(&mut buf, 0)?.is_some() {}
        Ok(())
    }
}
