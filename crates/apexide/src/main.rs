mod client;

use std::{net::SocketAddr, path::PathBuf};

use anyhow::Context;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

use crate::client::{Body, Client, DaemonDown};

const OVERVIEW: &str = "\
CONCEPTS
  The daemon (`apexide daemon`) owns the keyboard. Every other command is a
  client that talks to it over $XDG_RUNTIME_DIR/apexide.sock. Settings are
  saved on every change (~/.config/apexide/) and restored when it restarts.

  Lighting is layered, bottom to top:
    1. effect      whole-board animation (`effect`), or `onboard` to let the
                   keyboard's own profile drive the LEDs
    2. overrides   fixed per-key colors (`set`, `clear`)
    3. flashes     temporary highlights that expire on their own (`flash`)
    4. brightness  global 0-100 scale (`brightness`)
  On top of the onboard effect, overrides and flashes light only their own
  keys and the rest of the board keeps its onboard lighting.

  The OLED (128x40, monochrome) shows one mode at a time: text, image, clock,
  sysinfo, blank or release. `oled notify` shows a message over it for a while.
  `profile save` stores lighting and OLED together under a name.

COLORS
  #rrggbb, #rgb, or a name: off black white red green blue yellow orange cyan
  teal magenta purple pink warm

KEY SELECTORS
  A comma- or space-separated list of any of:
    key names      esc f1..f12 prtsc scrlk pause grave 1..0 minus equal
                   backspace insert home pgup tab q..p lbracket rbracket
                   backslash delete end pgdn capslock a..l semicolon quote
                   enter lshift z..m comma period slash rshift up lctrl lwin
                   lalt space ralt fn menu rctrl left down right, numpad keys
                   kp0..kp9 etc. Common aliases work (escape, ctrl, win, del).
                   `apexide keys` lists them all.
    raw LED codes  0x87
    groups         all letters numbers fkeys wasd arrows nav modifiers numpad
  Quote a selector that contains spaces: apexide set 'q w e' red

OUTPUT AND EXIT CODES
  Output is human-readable text by default. With --json the daemon's JSON
  response is printed unchanged, and errors print {\"error\": \"...\"} to stdout.
    0  success
    1  request rejected or failed (bad color, unknown key, ...)
    2  invalid command line
    3  daemon not reachable (start it with `apexide daemon`)

EXAMPLES
  apexide effect wave --period-ms 2500
  apexide set wasd red esc white
  apexide flash fkeys red -d 2000 -b 150
  apexide oled text \"build passed\"
  apexide profile save gaming

Run `apexide <command> --help` for one command, or `apexide reference`
(alias: --help-verbose) for every command's full help at once.";

/// Control SteelSeries Apex keyboards: per-key RGB, effects, OLED and profiles.
///
/// apexide is a Linux replacement for SteelSeries GG. A small daemon drives
/// the keyboard, and this CLI exposes everything it can do, both for people
/// and for scripts and agents (add --json).
#[derive(Parser)]
#[command(
    version,
    propagate_version = true,
    after_help = "Run `apexide --help` for concepts, color and key syntax, and exit codes,\nor `apexide reference` for the full help of every command.",
    after_long_help = OVERVIEW
)]
struct Cli {
    /// Daemon socket [env: APEXIDE_SOCKET] [default: $XDG_RUNTIME_DIR/apexide.sock]
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help_heading = "Global options"
    )]
    socket: Option<PathBuf>,
    /// Print the raw JSON response (or {"error": ...}) instead of text.
    #[arg(long, global = true, help_heading = "Global options")]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the daemon that owns the keyboard (foreground).
    ///
    /// Only one daemon can run at a time. It reconnects when the keyboard is
    /// replugged, and on SIGINT/SIGTERM it hands lighting back to the keyboard
    /// and clears the OLED. Set RUST_LOG=debug for verbose logs.
    #[command(
        after_long_help = "EXAMPLES\n  apexide daemon\n  apexide daemon --tcp 127.0.0.1:7171\n  systemctl --user enable --now apexide   # see packaging/apexide.service"
    )]
    Daemon {
        /// Also serve the HTTP API over TCP. Keep it on localhost; there is no auth.
        #[arg(long, value_name = "ADDR")]
        tcp: Option<SocketAddr>,
        /// Where state and profiles are stored [default: $XDG_CONFIG_HOME/apexide]
        #[arg(long, value_name = "DIR")]
        config_dir: Option<PathBuf>,
    },
    /// Show the connected keyboard and current lighting, OLED and profiles.
    Status,
    /// List key names, aliases and groups usable in key selectors.
    ///
    /// With --json, each key also has its LED code and physical position
    /// (x, y, w in key units), which is enough to draw the keyboard.
    Keys {
        /// Layout to list [default: the connected keyboard's, else tkl]
        #[arg(long, value_enum)]
        form: Option<Form>,
    },
    /// Print the HTTP API description as JSON (works without the daemon).
    Schema,
    /// Print the full help of every command (alias: --help-verbose).
    Reference,
    /// Paint every key one color and remove all per-key overrides.
    ///
    /// Same as `effect static --color COLOR` followed by `clear`.
    #[command(after_long_help = "EXAMPLES\n  apexide fill '#002244'\n  apexide fill warm")]
    Fill { color: String },
    /// Give keys fixed colors on top of the current effect.
    ///
    /// Takes alternating SELECTOR COLOR pairs. Overrides stay until cleared
    /// with `clear`, `fill`, or by applying a profile.
    #[command(
        arg_required_else_help = true,
        after_long_help = "EXAMPLES\n  apexide set wasd red\n  apexide set wasd red esc white 'f1 f2 f3' '#00ff88'\n  apexide set 0x87 blue"
    )]
    Set {
        #[arg(num_args = 2.., value_names = ["SELECTOR", "COLOR"])]
        pairs: Vec<String>,
    },
    /// Remove per-key overrides: all of them, or those matching SELECTOR.
    Clear { selector: Option<String> },
    /// Set the whole-board lighting effect.
    ///
    /// Options that do not apply to the chosen effect are ignored.
    ///   off        all LEDs off
    ///   static     --color (required)
    ///   breathe    --color (required), --period-ms [3000]
    ///   spectrum   whole board cycles hues; --period-ms [10000]
    ///   wave       rainbow sweep; --period-ms [3000], --wavelength [18],
    ///              --direction [right]
    ///   gradient   --from, --to (required), --direction [right]
    ///   twinkle    --color (required), --background [#000000], --period-ms [3000]
    ///   onboard    the keyboard's own lighting; overrides and flashes still
    ///              show on their keys
    ///
    /// Effects are rendered by the daemon at about 30 fps.
    #[command(
        verbatim_doc_comment,
        about = "Set the whole-board lighting effect",
        after_long_help = "EXAMPLES\n  apexide effect wave --period-ms 2500 --direction left\n  apexide effect breathe -c cyan -p 4000\n  apexide effect gradient --from purple --to orange -d down\n  apexide effect twinkle -c white --background '#000022'\n  apexide effect onboard"
    )]
    Effect {
        #[arg(value_enum)]
        name: EffectName,
        /// Main color (static, breathe, twinkle).
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
        /// Length of one effect cycle in milliseconds.
        #[arg(long, short, value_name = "MS")]
        period_ms: Option<u32>,
        /// Width of one full rainbow in key units (wave).
        #[arg(long, value_name = "KEYS")]
        wavelength: Option<f32>,
        /// Direction of travel (wave, gradient).
        #[arg(long, short, value_enum)]
        direction: Option<Dir>,
    },
    /// Set global lighting brightness, 0-100.
    ///
    /// Applied by the daemon to everything it draws. Onboard lighting itself
    /// follows the keyboard's own brightness keys instead.
    Brightness { percent: u8 },
    /// Highlight keys temporarily, steady or blinking.
    ///
    /// Flashes sit above overrides and expire on their own. Several can be
    /// active at once, and the newest wins where they overlap.
    #[command(
        after_long_help = "EXAMPLES\n  apexide flash esc red\n  apexide flash fkeys orange -d 5000 -b 250"
    )]
    Flash {
        selector: String,
        color: String,
        /// How long the flash lasts (max 600000).
        #[arg(long, short, default_value_t = 1000, value_name = "MS")]
        duration_ms: u64,
        /// Toggle on and off at this interval instead of holding steady.
        #[arg(long, short, value_name = "MS")]
        blink_ms: Option<u32>,
    },
    /// Hand lighting back to the keyboard's onboard profile.
    ///
    /// Same as `effect onboard`. Any overrides stay on their keys; run
    /// `clear` first to hand back the whole board.
    Onboard,
    /// Print the color each key is being sent right now.
    Frame,
    /// Control the 128x40 OLED screen.
    #[command(subcommand)]
    Oled(OledCmd),
    /// Save, apply and manage named setups (lighting and OLED together).
    #[command(subcommand)]
    Profile(ProfileCmd),
    /// Send a request straight to the HTTP API (see `apexide schema`).
    ///
    /// BODY is JSON, or @FILE to send a file's raw bytes (images).
    /// The response is always printed as JSON.
    #[command(
        after_long_help = "EXAMPLES\n  apexide raw GET /v1/lighting\n  apexide raw PUT /v1/lighting '{\"effect\":{\"type\":\"off\"},\"keys\":{\"esc\":\"red\"},\"brightness\":80}'\n  apexide raw POST '/v1/oled/image?fit=cover' @photo.png"
    )]
    Raw {
        /// GET, POST, PUT or DELETE.
        method: String,
        /// Path including any query string, e.g. /v1/status.
        path: String,
        body: Option<String>,
    },
}

#[derive(Subcommand)]
enum OledCmd {
    /// Show text, sized automatically and word-wrapped.
    ///
    /// The largest font that fits is used: about 12 characters x 2 lines at
    /// the biggest, down to 32 x 6 at the smallest. Newlines in the text
    /// start a new line.
    #[command(
        after_long_help = "EXAMPLES\n  apexide oled text hello world\n  apexide oled text --align left \"$(printf 'line one\\nline two')\""
    )]
    Text {
        /// Words are joined with spaces.
        #[arg(required = true)]
        text: Vec<String>,
        #[arg(long, short, value_enum, default_value = "center")]
        align: AlignArg,
    },
    /// Show a PNG, JPEG, BMP or animated GIF, converted to 1-bit.
    ///
    /// GIFs loop with their own frame timing. Photos look best dithered (the
    /// default); logos and line art look best with --no-dither.
    #[command(
        after_long_help = "EXAMPLES\n  apexide oled image cat.gif\n  apexide oled image logo.png --fit stretch --no-dither --threshold 100"
    )]
    Image {
        path: PathBuf,
        /// contain: fit inside, keep aspect; cover: fill and crop; stretch: exact 128x40.
        #[arg(long, value_enum, default_value = "contain")]
        fit: FitArg,
        /// Use a plain threshold instead of Floyd-Steinberg dithering.
        #[arg(long)]
        no_dither: bool,
        /// Brightness cutoff 0-255 used with --no-dither.
        #[arg(long, default_value_t = 128)]
        threshold: u8,
        /// Swap black and white.
        #[arg(long)]
        invert: bool,
    },
    /// Show a clock.
    #[command(
        after_long_help = "EXAMPLES\n  apexide oled clock\n  apexide oled clock -f '%H:%M:%S'\n  apexide oled clock -f '%a %d %b'"
    )]
    Clock {
        /// strftime format.
        #[arg(long, short, default_value = "%H:%M")]
        format: String,
    },
    /// Show live CPU and RAM usage bars.
    Sysinfo,
    /// Turn the screen off (stays off).
    Blank,
    /// Clear the screen and stop drawing on it.
    Release,
    /// Show a message over the current mode for a while.
    #[command(
        after_long_help = "EXAMPLES\n  apexide oled notify \"Build passed\"\n  apexide oled notify -d 10000 \"Meeting in 5 min\""
    )]
    Notify {
        /// Words are joined with spaces.
        #[arg(required = true)]
        text: Vec<String>,
        #[arg(long, short, default_value_t = 5000, value_name = "MS")]
        duration_ms: u64,
    },
    /// Print what the screen shows right now as ASCII art (# = lit).
    Preview,
    /// Show the current OLED mode.
    Status,
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// List saved profiles.
    List,
    /// Save the current lighting and OLED setup under NAME.
    ///
    /// Names may contain letters, digits, '-' and '_'. An existing profile
    /// with the same name is overwritten.
    Save { name: String },
    /// Replace the current setup with a saved profile.
    Apply { name: String },
    /// Print a profile's contents.
    Show { name: String },
    /// Delete a saved profile.
    Delete { name: String },
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

/// Full help of the top-level command and every subcommand.
fn reference() -> String {
    fn walk(cmd: &mut clap::Command, path: &str, out: &mut String) {
        for sub in cmd.get_subcommands_mut() {
            if sub.get_name() == "help" {
                continue;
            }
            let path = format!("{path} {}", sub.get_name());
            out.push_str(&format!(
                "\n\n{}\n{path}\n{}\n\n",
                "=".repeat(78),
                "=".repeat(78)
            ));
            out.push_str(&sub.render_long_help().to_string());
            walk(sub, &path, out);
        }
    }
    let mut root = Cli::command();
    root.build();
    let mut out = root.render_long_help().to_string();
    walk(&mut root, "apexide", &mut out);
    out
}

fn main() {
    if std::env::args().skip(1).any(|a| a == "--help-verbose") {
        print!("{}", reference());
        return;
    }
    let cli = Cli::parse();
    let json = cli.json;
    if let Err(e) = run(cli) {
        if json {
            println!("{}", json!({ "error": format!("{e:#}") }));
        } else {
            eprintln!("error: {e:#}");
        }
        let code = if e.downcast_ref::<DaemonDown>().is_some() {
            3
        } else {
            1
        };
        std::process::exit(code);
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
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
        Cmd::Schema => (apexide_daemon::api::schema(), |v| serde_json::to_string_pretty(v).unwrap()),
        Cmd::Reference => (Value::String(reference()), |v| v.as_str().unwrap_or_default().to_string()),
        Cmd::Raw { method, path, body } => {
            let body = match body {
                None => Body::None,
                Some(b) => match b.strip_prefix('@') {
                    Some(file) => Body::Bytes(std::fs::read(file).with_context(|| format!("reading {file}"))?),
                    None => Body::Json(serde_json::from_str(&b).context("BODY is not valid JSON (use @FILE for raw bytes)")?),
                },
            };
            (c.request(&method.to_ascii_uppercase(), &path, body)?, |v| serde_json::to_string_pretty(v).unwrap())
        }
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
