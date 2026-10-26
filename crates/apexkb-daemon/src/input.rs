//! The input thread: reads key presses for the reactive lighting layer.
//!
//! It opens the keyboard's key input interfaces only while a reactive layer
//! is on and a keyboard is connected, and closes them as soon as either
//! stops. Presses go straight into [`Shared::presses`] and are never logged.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

use apexkb_core::input::{KeyEvent, KeyInput};

use crate::state::Shared;

/// Longest wait for input before checking whether it is still wanted.
const POLL_MS: i32 = 500;
const RETRY_EVERY: Duration = Duration::from_secs(2);
/// Longest sleep while no input is wanted, unless a [`crate::engine::Waker`]
/// reports a change first.
const IDLE_WAIT: Duration = Duration::from_secs(60);

pub fn run(
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    wake: mpsc::Receiver<()>,
    render: mpsc::Sender<()>,
) {
    let mut input: Option<KeyInput> = None;
    let mut last_attempt: Option<Instant> = None;
    let mut last_error: Option<String> = None;
    let mut events = Vec::new();

    // Fade out keys that were held when input stopped, since their releases
    // will never arrive.
    let close = |input: &mut Option<KeyInput>| {
        if input.take().is_some() {
            shared.lock().unwrap().release_all(Instant::now());
            render.send(()).ok();
            tracing::info!("stopped reading key presses");
        }
    };

    while !stop.load(Ordering::SeqCst) {
        wake.try_iter().for_each(drop);
        let want: Option<PathBuf> = {
            let s = shared.lock().unwrap();
            s.settings
                .lighting
                .reactive
                .is_on()
                .then(|| s.device.as_ref().map(|d| d.hidraw.clone()))
                .flatten()
        };

        if input.as_ref().map(|i| i.control()) != want.as_deref() {
            close(&mut input);
            if let Some(path) = &want
                && last_attempt.is_none_or(|t| t.elapsed() >= RETRY_EVERY)
            {
                last_attempt = Some(Instant::now());
                match KeyInput::open(path) {
                    Ok(i) => {
                        tracing::info!("reading key presses for reactive lighting");
                        input = Some(i);
                        last_error = None;
                    }
                    Err(e) => {
                        let msg = format!("cannot read key presses: {e}");
                        if last_error.as_ref() != Some(&msg) {
                            tracing::warn!("{msg}");
                        }
                        last_error = Some(msg);
                    }
                }
            }
        }

        let Some(i) = input.as_mut() else {
            let wait = if want.is_some() {
                RETRY_EVERY
            } else {
                IDLE_WAIT
            };
            if let Err(RecvTimeoutError::Disconnected) = wake.recv_timeout(wait) {
                thread::sleep(wait);
            }
            continue;
        };

        events.clear();
        if let Err(e) = i.read(POLL_MS, &mut events) {
            tracing::debug!("key input closed: {e}");
            close(&mut input);
            continue;
        }
        if events.is_empty() {
            continue;
        }
        let now = Instant::now();
        {
            let mut s = shared.lock().unwrap();
            for ev in &events {
                match *ev {
                    KeyEvent::Press(k) => s.press(k, now),
                    KeyEvent::Release(k) => s.release(k, now),
                }
            }
        }
        render.send(()).ok();
    }
    close(&mut input);
}
