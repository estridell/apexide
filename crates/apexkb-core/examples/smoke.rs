//! Hardware smoke test: prints device info, lights WASD, writes the OLED,
//! then hands lighting back to the keyboard.
use std::{thread::sleep, time::Duration};

use apexkb_core::{Frame, Keyboard, Rgb, keys, oled::Align};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut kb = Keyboard::open()?;
    println!("{}", serde_json::to_string_pretty(kb.info())?);
    let form = kb.model().form;
    let mut colors: Vec<_> = keys::resolve("all", form)?
        .into_iter()
        .map(|k| (k, Rgb::new(0, 0, 40)))
        .collect();
    colors.extend(
        keys::resolve("wasd", form)?
            .into_iter()
            .map(|k| (k, Rgb::new(255, 0, 0))),
    );
    kb.set_colors(&colors)?;
    kb.draw_oled(&Frame::text("apexkb says hi", Align::Center))?;
    sleep(Duration::from_secs(4));
    kb.onboard_lighting()?;
    Ok(())
}
