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

use apexide_core::{Frame, KeyCode, Keyboard, Rgb, keys::KeyDef};

use crate::{effects, screen, state::Shared};

const TICK: Duration = Duration::from_millis(33);
const RECONNECT_EVERY: Duration = Duration::from_secs(2);
/// Rewrite unchanged output this often, in case the keyboard's own UI
/// (brightness keys, profile switching) took over in the meantime.
const REFRESH_EVERY: Duration = Duration::from_secs(2);

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

#[derive(Default)]
struct Output {
    colors: Option<Vec<(KeyCode, Rgb)>>,
    oled: Option<Frame>,
    onboard_sent: bool,
    written: Option<Instant>,
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
            s.frame_colors = colors.clone().unwrap_or_default();
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
    colors: Option<Vec<(KeyCode, Rgb)>>,
    oled: Option<Frame>,
) -> apexide_core::device::Result<()> {
    let refresh = out.written.is_none_or(|t| t.elapsed() >= REFRESH_EVERY);
    match colors {
        Some(c) => {
            if refresh || out.colors.as_ref() != Some(&c) {
                kb.set_colors(&c)?;
                out.colors = Some(c);
                out.onboard_sent = false;
            }
        }
        None => {
            if !out.onboard_sent {
                kb.onboard_lighting()?;
                out.onboard_sent = true;
                out.colors = None;
            }
        }
    }
    match oled {
        Some(f) => {
            if refresh || out.oled.as_ref() != Some(&f) {
                kb.draw_oled(&f)?;
                out.oled = Some(f);
            }
        }
        None => out.oled = None,
    }
    if refresh {
        out.written = Some(Instant::now());
    }
    Ok(())
}
