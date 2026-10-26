//! The render thread: the only code that talks to the keyboard.
//!
//! Every tick it computes the desired key colors and OLED frame from
//! [`Shared`], writes whatever changed, and reconnects if the keyboard was
//! unplugged.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use apexide_core::{Frame, Keyboard, keys::KeyDef};

use crate::{
    effects::{self, LightingFrame},
    screen,
    state::Shared,
};

const TICK: Duration = Duration::from_millis(33);
const RECONNECT_EVERY: Duration = Duration::from_secs(2);
/// Rewrite unchanged lighting this often, in case the keyboard's own UI
/// (brightness keys, profile switching) took over in the meantime.
const REFRESH_EVERY: Duration = Duration::from_secs(2);
/// Rewrite an unchanged OLED frame this often. The firmware menu (hold the OLED
/// menu button) ignores our frames while it is open and leaves its last
/// frame behind when it closes, without reporting either event, so the next
/// rewrite is what clears it.
const OLED_REFRESH_EVERY: Duration = Duration::from_millis(250);

pub struct Engine {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Engine {
    pub fn spawn(shared: Arc<Mutex<Shared>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = thread::Builder::new()
            .name("apexide-render".into())
            .spawn(move || run(shared, flag))
            .expect("spawn render thread");
        Self {
            stop,
            handle: Some(handle),
        }
    }

    /// Stop rendering and hand the keyboard back to its onboard lighting.
    pub fn shutdown(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            h.join().ok();
        }
    }
}

/// What was last written to the keyboard. `None` means unknown, which
/// forces a write.
#[derive(Default)]
struct Output {
    lighting: Option<LightingFrame>,
    oled: Option<Frame>,
    written: Option<Instant>,
    oled_written: Option<Instant>,
}

fn run(shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>) {
    let epoch = Instant::now();
    let mut kb: Option<Keyboard> = None;
    let mut last_attempt: Option<Instant> = None;
    let mut out = Output::default();
    let mut sys = screen::SysSampler::default();

    while !stop.load(Ordering::SeqCst) {
        let tick_start = Instant::now();

        if kb.is_none() && last_attempt.is_none_or(|t| t.elapsed() >= RECONNECT_EVERY) {
            last_attempt = Some(Instant::now());
            match Keyboard::open() {
                Ok(k) => {
                    tracing::info!(model = k.info().model, path = %k.info().hidraw.display(), "keyboard connected");
                    let mut s = shared.lock().unwrap();
                    s.device = Some(k.info().clone());
                    s.last_error = None;
                    out = Output::default();
                    kb = Some(k);
                }
                Err(e) => {
                    let msg = e.to_string();
                    let mut s = shared.lock().unwrap();
                    if s.last_error.as_deref() != Some(&msg) {
                        tracing::warn!("{msg}");
                    }
                    s.device = None;
                    s.last_error = Some(msg);
                }
            }
        }

        // Compute the desired output under the lock, write without it.
        let now = Instant::now();
        let t_ms = (now - epoch).as_millis() as u64;
        let (colors, oled) = {
            let mut s = shared.lock().unwrap();
            s.flashes.retain(|f| f.until > now);
            if s.notification.as_ref().is_some_and(|n| n.until <= now) {
                s.notification = None;
            }
            let form = kb
                .as_ref()
                .map(|k| k.model().form)
                .unwrap_or(apexide_core::FormFactor::Tkl);
            let keys: Vec<&KeyDef> = form.keys().collect();
            let colors = effects::render(
                &s.settings.lighting,
                &s.flashes,
                &keys,
                form.size(),
                t_ms,
                now,
            );
            let oled_ms = (now - s.oled_since.unwrap_or(epoch)).as_millis() as u64;
            let oled = screen::render(&s.settings.oled, s.notification.as_ref(), &mut sys, oled_ms);
            s.frame_lighting = colors.clone();
            s.frame_oled = oled.clone();
            (colors, oled)
        };

        if let Some(k) = kb.as_mut()
            && let Err(e) = write(k, &mut out, colors, oled.filter(|_| k.model().oled))
        {
            tracing::warn!("lost keyboard: {e}");
            let mut s = shared.lock().unwrap();
            s.device = None;
            s.last_error = Some(e.to_string());
            kb = None;
        }

        if let Some(rest) = TICK.checked_sub(tick_start.elapsed()) {
            thread::sleep(rest);
        }
    }

    if let Some(mut k) = kb {
        k.onboard_lighting().ok();
        if out.oled.is_some() {
            k.draw_oled(&Frame::new()).ok();
        }
        tracing::info!("keyboard returned to onboard lighting");
    }
}

fn write(
    kb: &mut Keyboard,
    out: &mut Output,
    lighting: LightingFrame,
    oled: Option<Frame>,
) -> apexide_core::device::Result<()> {
    let refresh = out.written.is_none_or(|t| t.elapsed() >= REFRESH_EVERY);
    let prev = out.lighting.take();
    let changed = prev.as_ref() != Some(&lighting);
    match &lighting {
        LightingFrame::Onboard => {
            if changed {
                kb.onboard_lighting()?;
            }
        }
        LightingFrame::Full(c) => {
            if changed || refresh {
                kb.set_colors(c)?;
            }
        }
        LightingFrame::Overlay(c) => {
            if changed || refresh {
                // A key can only be handed back to onboard lighting by
                // restoring the whole board, so do that whenever a key we set
                // earlier is no longer part of the overlay.
                let restore = match &prev {
                    None | Some(LightingFrame::Full(_)) => true,
                    Some(LightingFrame::Onboard) => false,
                    Some(LightingFrame::Overlay(p)) => {
                        p.iter().any(|(k, _)| !c.iter().any(|(n, _)| n == k))
                    }
                };
                if restore {
                    kb.onboard_lighting()?;
                }
                kb.set_colors(c)?;
            }
        }
    }
    out.lighting = Some(lighting);

    match oled {
        Some(f) => {
            let oled_refresh = out
                .oled_written
                .is_none_or(|t| t.elapsed() >= OLED_REFRESH_EVERY);
            if oled_refresh || out.oled.as_ref() != Some(&f) {
                kb.draw_oled(&f)?;
                out.oled = Some(f);
                out.oled_written = Some(Instant::now());
            }
        }
        // Clear whatever we drew last so it does not linger after a
        // notification expires or the screen is released.
        None => {
            if out.oled.take().is_some() {
                kb.draw_oled(&Frame::new())?;
            }
        }
    }
    if refresh {
        out.written = Some(Instant::now());
    }
    Ok(())
}
