//! Composes OLED frames from the current [`OledMode`].

use std::{fs, time::Instant};

use apexkb_core::{
    Frame,
    oled::{self, Align, HEIGHT, WIDTH},
};
use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::state::{Notification, OledMode};

/// Samples CPU and memory usage from /proc.
#[derive(Debug, Default)]
pub struct SysSampler {
    prev_cpu: Option<(u64, u64)>,
    last: Option<Instant>,
    cpu: f32,
    mem: f32,
}

impl SysSampler {
    /// Sample /proc, at most once a second.
    pub fn refresh(&mut self) {
        if self.last.is_some_and(|t| t.elapsed().as_millis() < 1000) {
            return;
        }
        self.last = Some(Instant::now());
        if let Some((busy, total)) = read_cpu() {
            if let Some((pb, pt)) = self.prev_cpu
                && total > pt
            {
                self.cpu = (busy - pb) as f32 / (total - pt) as f32;
            }
            self.prev_cpu = Some((busy, total));
        }
        if let Some(m) = read_mem() {
            self.mem = m;
        }
    }
}

fn read_cpu() -> Option<(u64, u64)> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    let nums: Vec<u64> = stat
        .lines()
        .next()?
        .split_whitespace()
        .skip(1)
        .filter_map(|n| n.parse().ok())
        .collect();
    let total: u64 = nums.iter().sum();
    let idle = nums.get(3)? + nums.get(4).unwrap_or(&0);
    Some((total - idle, total))
}

fn read_mem() -> Option<f32> {
    let info = fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| -> Option<f32> {
        info.lines()
            .find(|l| l.starts_with(name))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    };
    let total = field("MemTotal:")?;
    Some(1.0 - field("MemAvailable:")? / total)
}

fn bar(frame: &mut Frame, label: &str, value: f32, y: i32) {
    oled::draw_text(
        frame,
        &format!("{label} {:>3.0}%", value * 100.0),
        Align::Left,
        (2, y, 60, 18),
    );
    let outline = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
    Rectangle::new(Point::new(64, y + 4), Size::new(62, 10))
        .into_styled(outline)
        .draw(frame)
        .ok();
    let w = (60.0 * value.clamp(0.0, 1.0)).round() as u32;
    Rectangle::new(Point::new(65, y + 5), Size::new(w, 8))
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(frame)
        .ok();
}

/// The frame to show at `t_ms`, or `None` to leave the OLED alone.
pub fn render(
    mode: &OledMode,
    notification: Option<&Notification>,
    sys: &SysSampler,
    t_ms: u64,
) -> Option<Frame> {
    if let Some(n) = notification {
        let mut f = Frame::new();
        let border = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
        Rectangle::new(Point::zero(), Size::new(WIDTH, HEIGHT))
            .into_styled(border)
            .draw(&mut f)
            .ok();
        oled::draw_text(
            &mut f,
            &n.text,
            Align::Center,
            (3, 2, WIDTH - 6, HEIGHT - 4),
        );
        return Some(f);
    }
    Some(match mode {
        OledMode::Release => return None,
        OledMode::Blank => Frame::new(),
        OledMode::Text { text, align } => Frame::text(text, *align),
        OledMode::Image { frames, .. } => {
            let total: u64 = frames.iter().map(|f| f.delay_ms.max(1) as u64).sum();
            let mut t = t_ms % total.max(1);
            frames
                .iter()
                .find(|f| {
                    let d = f.delay_ms.max(1) as u64;
                    if t < d {
                        true
                    } else {
                        t -= d;
                        false
                    }
                })
                .or(frames.first())?
                .frame
                .clone()
        }
        OledMode::Clock { format } => Frame::text(
            &chrono::Local::now().format(format).to_string(),
            Align::Center,
        ),
        OledMode::Sysinfo => {
            let mut f = Frame::new();
            bar(&mut f, "CPU", sys.cpu, 1);
            bar(&mut f, "RAM", sys.mem, 21);
            f
        }
    })
}
