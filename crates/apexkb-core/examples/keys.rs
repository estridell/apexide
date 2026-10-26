//! Hardware probe for key input: prints the presses and releases the
//! keyboard reports for 10 seconds, as HID usages and key names.
use std::time::{Duration, Instant};

use apexkb_core::{
    Keyboard,
    input::{KeyEvent, KeyInput},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (_, control) = Keyboard::list()?
        .into_iter()
        .next()
        .ok_or("no keyboard found")?;
    let mut input = KeyInput::open(&control)?;
    println!("reading key presses for 10 s, type something");
    let end = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    while Instant::now() < end {
        events.clear();
        input.read(100, &mut events)?;
        for ev in &events {
            let (what, k) = match ev {
                KeyEvent::Press(k) => ("press  ", k),
                KeyEvent::Release(k) => ("release", k),
            };
            println!("{what} 0x{:02x} {k}", k.0);
        }
    }
    Ok(())
}
