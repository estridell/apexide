mod client;

use std::{net::SocketAddr, path::PathBuf};

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

use crate::client::{Body, Client};

/// Control SteelSeries Apex keyboards: per-key RGB, effects, OLED and profiles.
///
/// Every command talks to the apexide daemon. Pass --json to get the raw API
/// response (stable, machine-readable), which is what scripts and agents
/// should use. `apexide schema` describes everything the API can do.
#[derive(Parser)]
#[command(version, propagate_version = true)]
struct Cli {
    /// Daemon socket [env: APEXIDE_SOCKET; default: $XDG_RUNTIME_DIR/apexide.sock]
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Print raw JSON responses.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the daemon that owns the keyboard (foreground).
    Daemon {
        /// Also serve the API over TCP, e.g. 127.0.0.1:7171.
        #[arg(long)]
        tcp: Option<SocketAddr>,
        /// Where state and profiles are stored [default: ~/.config/apexide].
        #[arg(long)]
        config_dir: Option<PathBuf>,
    },
    /// Connection, device and current settings.
    Status,
    /// List key names usable in selectors.
    Keys {
        #[arg(long, value_enum)]
        form: Option<Form>,
    },
    /// API overview: effects, OLED modes, endpoints, color and key syntax.
    Schema,
    /// Paint every key one color and drop per-key overrides.
    Fill { color: String },
    /// Set per-key colors: `apexide set wasd red esc '#00ff00'`.
    #[command(arg_required_else_help = true)]
    Set {
        /// Alternating SELECTOR COLOR pairs.
        #[arg(num_args = 2.., value_names = ["SELECTOR", "COLOR"])]
        pairs: Vec<String>,
    },
    /// Remove per-key overrides (all, or only the given selector).
    Clear { selector: Option<String> },
    /// Run a lighting effect.
    Effect {
        #[arg(value_enum)]
        name: EffectName,
        /// Color for static, breathe, twinkle.
        #[arg(long, short)]
        color: Option<String>,
        /// Gradient start color.
        #[arg(long)]
        from: Option<String>,
        /// Gradient end color.
        #[arg(long)]
        to: Option<String>,
        /// Twinkle background color.
        #[arg(long)]
        background: Option<String>,
        /// Effect cycle length in milliseconds.
        #[arg(long, short)]
        period_ms: Option<u32>,
        /// Wave width in key units.
        #[arg(long)]
        wavelength: Option<f32>,
        #[arg(long, short, value_enum)]
        direction: Option<Dir>,
    },
    /// Global lighting brightness, 0-100.
    Brightness { percent: u8 },
    /// Temporarily highlight keys.
    Flash {
        selector: String,
        color: String,
        #[arg(long, short, default_value_t = 1000)]
        duration_ms: u64,
        /// Blink at this interval instead of holding steady.
        #[arg(long, short)]
        blink_ms: Option<u32>,
    },
    /// Give lighting back to the keyboard's onboard profile.
    Onboard,
    /// Colors the daemon is currently sending, per key.
    Frame,
    /// Control the OLED screen.
    #[command(subcommand)]
    Oled(OledCmd),
    /// Save and restore complete setups.
    #[command(subcommand)]
    Profile(ProfileCmd),
}

#[derive(Subcommand)]
enum OledCmd {
    /// Show text (auto-sized and word-wrapped).
    Text {
        #[arg(required = true)]
        text: Vec<String>,
        #[arg(long, short, value_enum, default_value = "center")]
        align: AlignArg,
    },
    /// Show a PNG, JPEG, BMP or animated GIF.
    Image {
        path: PathBuf,
        #[arg(long, value_enum, default_value = "contain")]
        fit: FitArg,
        /// Threshold instead of dithering (better for line art).
        #[arg(long)]
        no_dither: bool,
        /// Luma cutoff 0-255 when not dithering.
        #[arg(long, default_value_t = 128)]
        threshold: u8,
        #[arg(long)]
        invert: bool,
    },
    /// Show a clock.
    Clock {
        /// strftime format.
        #[arg(long, short, default_value = "%H:%M")]
        format: String,
    },
    /// Show CPU and RAM usage.
    Sysinfo,
    /// Turn the screen off.
    Blank,
    /// Clear the screen and stop drawing on it.
    Release,
    /// Pop up a message for a while, then return to the current mode.
    Notify {
        #[arg(required = true)]
        text: Vec<String>,
        #[arg(long, short, default_value_t = 5000)]
        duration_ms: u64,
    },
    /// Print what the screen shows right now as ASCII art.
    Preview,
    /// Current OLED mode.
    Status,
}

#[derive(Subcommand)]
enum ProfileCmd {
    List,
    /// Save the current lighting and OLED setup.
    Save {
        name: String,
    },
    Apply {
        name: String,
    },
    Show {
        name: String,
    },
    Delete {
        name: String,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Form {
    Tkl,
    Full,
}

#[derive(Clone, Copy, ValueEnum)]
enum EffectName {
    Off,
    Static,
    Breathe,
    Spectrum,
    Wave,
    Gradient,
    Twinkle,
    Onboard,
}

#[derive(Clone, Copy, ValueEnum)]
enum Dir {
    Right,
    Left,
    Up,
    Down,
}

#[derive(Clone, Copy, ValueEnum)]
enum AlignArg {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, ValueEnum)]
enum FitArg {
    Contain,
    Cover,
    Stretch,
}

fn value_name<T: ValueEnum>(v: T) -> String {
    v.to_possible_value().unwrap().get_name().to_string()
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let socket = cli
        .socket
        .clone()
        .or_else(|| std::env::var_os("APEXIDE_SOCKET").map(PathBuf::from))
        .unwrap_or_else(apexide_daemon::default_socket);

    if let Cmd::Daemon { tcp, config_dir } = cli.cmd {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info".into()),
            )
            .init();
        let rt = tokio::runtime::Runtime::new()?;
        return rt.block_on(apexide_daemon::serve(apexide_daemon::ServeOptions {
            socket,
            tcp,
            config_dir: config_dir.unwrap_or_else(apexide_daemon::store::Store::default_dir),
        }));
    }

    let c = Client { socket };
    let json = cli.json;
    let (resp, human): (Value, fn(&Value) -> String) = match cli.cmd {
        Cmd::Daemon { .. } => unreachable!(),
        Cmd::Status => (c.get("/v1/status")?, fmt_status),
        Cmd::Keys { form } => {
            let q = form.map(|f| format!("?form={}", value_name(f))).unwrap_or_default();
            (c.get(&format!("/v1/keys{q}"))?, fmt_keys)
        }
        Cmd::Schema => (c.get("/v1/schema")?, |v| serde_json::to_string_pretty(v).unwrap()),
        Cmd::Fill { color } => (c.post("/v1/lighting/fill", json!({ "color": color }))?, fmt_lighting),
        Cmd::Set { pairs } => {
            if pairs.len() % 2 != 0 {
                anyhow::bail!("expected SELECTOR COLOR pairs, got an odd number of arguments");
            }
            let keys: serde_json::Map<_, _> =
                pairs.chunks(2).map(|p| (p[0].clone(), Value::String(p[1].clone()))).collect();
            (c.post("/v1/lighting/keys", json!({ "keys": keys }))?, fmt_lighting)
        }
        Cmd::Clear { selector } => (c.post("/v1/lighting/keys/clear", json!({ "keys": selector }))?, fmt_lighting),
        Cmd::Effect { name, color, from, to, background, period_ms, wavelength, direction } => {
            let mut e = json!({ "type": value_name(name) });
            let o = e.as_object_mut().unwrap();
            let mut put = |k: &str, v: Option<Value>| {
                if let Some(v) = v {
                    o.insert(k.into(), v);
                }
            };
            put("color", color.map(Value::from));
            put("from", from.map(Value::from));
            put("to", to.map(Value::from));
            put("background", background.map(Value::from));
            put("period_ms", period_ms.map(Value::from));
            put("wavelength", wavelength.map(Value::from));
            put("direction", direction.map(|d| Value::from(value_name(d))));
            (c.post("/v1/lighting/effect", e)?, fmt_lighting)
        }
        Cmd::Brightness { percent } => {
            (c.put("/v1/lighting/brightness", json!({ "brightness": percent }))?, fmt_lighting)
        }
        Cmd::Flash { selector, color, duration_ms, blink_ms } => (
            c.post(
                "/v1/lighting/flash",
                json!({ "keys": selector, "color": color, "duration_ms": duration_ms, "blink_ms": blink_ms }),
            )?,
            |_| "flashing".into(),
        ),
        Cmd::Onboard => (c.post("/v1/lighting/effect", json!({ "type": "onboard" }))?, fmt_lighting),
        Cmd::Frame => (c.get("/v1/lighting/frame")?, fmt_frame),
        Cmd::Oled(cmd) => match cmd {
            OledCmd::Text { text, align } => (
                c.post("/v1/oled/text", json!({ "text": text.join(" "), "align": value_name(align) }))?,
                fmt_oled,
            ),
            OledCmd::Image { path, fit, no_dither, threshold, invert } => {
                let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let q = format!(
                    "/v1/oled/image?name={}&fit={}&dither={}&threshold={threshold}&invert={invert}",
                    urlencode(&name),
                    value_name(fit),
                    !no_dither,
                );
                (c.request("POST", &q, Body::Bytes(bytes))?, fmt_oled)
            }
            OledCmd::Clock { format } => (c.post("/v1/oled/clock", json!({ "format": format }))?, fmt_oled),
            OledCmd::Sysinfo => (c.post("/v1/oled/sysinfo", json!({}))?, fmt_oled),
            OledCmd::Blank => (c.post("/v1/oled/blank", json!({}))?, fmt_oled),
            OledCmd::Release => (c.post("/v1/oled/release", json!({}))?, fmt_oled),
            OledCmd::Notify { text, duration_ms } => (
                c.post("/v1/oled/notify", json!({ "text": text.join(" "), "duration_ms": duration_ms }))?,
                |_| "notification shown".into(),
            ),
            OledCmd::Preview => (c.get("/v1/oled/preview")?, |v| match v["ascii"].as_str() {
                Some(a) => a.trim_end().to_string(),
                None => "(OLED not driven by apexide)".into(),
            }),
            OledCmd::Status => (c.get("/v1/oled")?, fmt_oled),
        },
        Cmd::Profile(cmd) => match cmd {
            ProfileCmd::List => (c.get("/v1/profiles")?, |v| {
                let names: Vec<_> = v["profiles"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                if names.is_empty() { "no profiles saved".into() } else { names.join("\n") }
            }),
            ProfileCmd::Save { name } => (c.put(&format!("/v1/profiles/{}", urlencode(&name)), json!({}))?, |v| {
                format!("saved {}", v["saved"].as_str().unwrap_or_default())
            }),
            ProfileCmd::Apply { name } => (c.post(&format!("/v1/profiles/{}/apply", urlencode(&name)), json!({}))?, |v| {
                format!("applied {}", v["applied"].as_str().unwrap_or_default())
            }),
            ProfileCmd::Show { name } => (c.get(&format!("/v1/profiles/{}", urlencode(&name)))?, |v| {
                serde_json::to_string_pretty(v).unwrap()
            }),
            ProfileCmd::Delete { name } => (c.request("DELETE", &format!("/v1/profiles/{}", urlencode(&name)), Body::None)?, |v| {
                format!("deleted {}", v["deleted"].as_str().unwrap_or_default())
            }),
        },
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&resp)?);
    } else {
        println!("{}", human(&resp));
    }
    Ok(())
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn describe_effect(e: &Value) -> String {
    let kind = e["type"].as_str().unwrap_or("?");
    let params: Vec<String> = e
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(k, _)| *k != "type")
        .map(|(k, v)| {
            format!(
                "{k}={}",
                v.as_str()
                    .map(String::from)
                    .unwrap_or_else(|| v.to_string())
            )
        })
        .collect();
    if params.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} ({})", params.join(", "))
    }
}

fn fmt_lighting(l: &Value) -> String {
    let overrides = l["keys"].as_object().map(|m| m.len()).unwrap_or(0);
    format!(
        "effect: {}\nkey overrides: {overrides}\nbrightness: {}%",
        describe_effect(&l["effect"]),
        l["brightness"]
    )
}

fn describe_oled(m: &Value) -> String {
    match m["type"].as_str() {
        Some("text") => format!("text {:?}", m["text"].as_str().unwrap_or_default()),
        Some("image") => format!(
            "image {} ({} frames)",
            m["name"].as_str().unwrap_or_default(),
            m["frame_count"]
        ),
        Some("clock") => format!("clock {}", m["format"].as_str().unwrap_or_default()),
        Some("release") => "released (keyboard's own screen)".into(),
        Some(other) => other.into(),
        None => "?".into(),
    }
}

fn fmt_oled(v: &Value) -> String {
    let mut s = format!("oled: {}", describe_oled(&v["mode"]));
    if let Some(n) = v["notification"].as_str() {
        s += &format!("\nnotification: {n:?}");
    }
    s
}

fn fmt_status(v: &Value) -> String {
    let mut out = String::new();
    if v["connected"].as_bool() == Some(true) {
        let d = &v["device"];
        out += &format!(
            "keyboard: {} (fw {}, {})\n",
            d["model"].as_str().unwrap_or("?"),
            d["firmware"].as_str().unwrap_or("?"),
            d["hidraw"].as_str().unwrap_or("?"),
        );
    } else {
        out += &format!(
            "keyboard: not connected ({})\n",
            v["error"].as_str().unwrap_or("waiting")
        );
    }
    out += &fmt_lighting(&v["lighting"]);
    out += &format!("\noled: {}", describe_oled(&v["oled"]));
    if let Some(n) = v["notification"].as_str() {
        out += &format!("\nnotification: {n:?}");
    }
    let profiles: Vec<_> = v["profiles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !profiles.is_empty() {
        out += &format!("\nprofiles: {}", profiles.join(", "));
    }
    out
}

fn fmt_keys(v: &Value) -> String {
    let mut out = format!(
        "{} layout\n\nkeys:\n",
        v["form_factor"].as_str().unwrap_or("?")
    );
    let mut line = String::new();
    for k in v["keys"].as_array().into_iter().flatten() {
        let name = k["name"].as_str().unwrap_or("?");
        if line.len() + name.len() > 78 {
            out += line.trim_end();
            out += "\n";
            line.clear();
        }
        line += name;
        line += " ";
    }
    out += line.trim_end();
    out += "\n\ngroups:\n";
    for g in v["groups"].as_array().into_iter().flatten() {
        out += &format!(
            "  {:<10} {}\n",
            g["name"].as_str().unwrap_or(""),
            g["description"].as_str().unwrap_or("")
        );
    }
    out.trim_end().to_string()
}

fn fmt_frame(v: &Value) -> String {
    if v["mode"] == "onboard" {
        return "onboard lighting (keyboard controls the LEDs)".into();
    }
    let header = if v["mode"] == "overlay" {
        "onboard lighting, with these keys set by apexide:\n"
    } else {
        ""
    };
    let keys = v["keys"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, c)| format!("{k:<14} {}", c.as_str().unwrap_or("")))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{header}{keys}")
}
