mod client;
mod update;

use std::{net::SocketAddr, path::PathBuf};

use anyhow::Context;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

use crate::client::{Body, Client, DaemonDown};

const OVERVIEW: &str = "\
CONCEPTS
  The daemon (`apexkb daemon`) owns the keyboard. Every other command is a
  client that talks to it over $XDG_RUNTIME_DIR/apexkb.sock. Settings are
  saved on every change (~/.config/apexkb/) and restored when it restarts.

  Lighting is layered, bottom to top:
    1. effect      whole-board animation (`effect`), or `onboard` to let the
                   keyboard's own profile drive the LEDs
    2. overrides   fixed per-key colors (`set`, `clear`)
    3. reactive    keys that light up when pressed, or ripples (`reactive`)
    4. flashes     temporary highlights that expire on their own (`flash`)
    5. brightness  global 0-100 scale (`brightness`)
  On top of the onboard effect, overrides, reactive highlights and flashes
  light only their own keys and the rest of the board keeps its onboard
  lighting.

  The OLED (128x40, monochrome) shows one mode at a time: text, image, clock,
  sysinfo (default), blank or release. `oled notify` shows a message over it for a while.
  `profile save` stores lighting and OLED together under a name.

  The keyboard also stores five profiles of its own (`onboard`): key remaps,
  a META layer, macros and an OLED image that work with no software running.

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
                   `apexkb keys` lists them all.
    raw LED codes  0x87
    groups         all letters numbers fkeys wasd arrows nav modifiers numpad
  Quote a selector that contains spaces: apexkb set 'q w e' red

OUTPUT AND EXIT CODES
  Output is human-readable text by default. With --json the daemon's JSON
  response is printed unchanged, and errors print {\"error\": \"...\"} to stdout.
    0  success
    1  request rejected or failed (bad color, unknown key, ...)
    2  invalid command line
    3  daemon not reachable (start it with `apexkb daemon`)

UPDATES
  `apexkb update` installs the latest GitHub release (`--check` only looks).
  `apexkb --version` prints this binary's version, `apexkb status` warns
  when the running daemon is a different version.

EXAMPLES
  apexkb effect wave --period-ms 2500
  apexkb set wasd red esc white
  apexkb reactive ripple -c cyan
  apexkb flash fkeys red -d 2000 -b 150
  apexkb oled text \"build passed\"
  apexkb profile save gaming

Run `apexkb <command> --help` for one command, or `apexkb reference`
(alias: --help-verbose) for every command's full help at once.";

/// Control SteelSeries Apex keyboards: per-key RGB, effects, OLED and profiles.
///
/// apexkb is a Linux replacement for SteelSeries GG. A small daemon drives
/// the keyboard, and this CLI exposes everything it can do, both for people
/// and for scripts and agents (add --json).
#[derive(Parser)]
#[command(
    name = "apexkb",
    version = apexkb_daemon::VERSION,
    propagate_version = true,
    after_help = "Run `apexkb --help` for concepts, color and key syntax, and exit codes,\nor `apexkb reference` for the full help of every command.",
    after_long_help = OVERVIEW
)]
struct Cli {
    /// Daemon socket [env: APEXKB_SOCKET] [default: $XDG_RUNTIME_DIR/apexkb.sock]
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
        after_long_help = "EXAMPLES\n  apexkb daemon\n  apexkb daemon --tcp 127.0.0.1:7171\n  systemctl --user enable --now apexkb   # see packaging/apexkb.service"
    )]
    Daemon {
        /// Also serve the HTTP API over TCP. Keep it on localhost; there is no auth.
        #[arg(long, value_name = "ADDR")]
        tcp: Option<SocketAddr>,
        /// Where state and profiles are stored [default: $XDG_CONFIG_HOME/apexkb]
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
    /// Update apexkb to the latest GitHub release (works without the daemon).
    ///
    /// Downloads the release binary, checks its SHA-256 and replaces the
    /// running binary in place, so install apexkb somewhere you can write
    /// (the README uses ~/.local/bin). If the apexkb.service user unit is
    /// active it is restarted; a daemon started by hand keeps running the old
    /// version until you restart it. Builds from source (version ending in
    /// -dev) count as older than the release of the same version.
    ///
    /// With --json: {current, latest, update_available, release_url, updated,
    /// and after an update path and service_restarted}.
    #[command(
        after_long_help = "EXAMPLES\n  apexkb update --check\n  apexkb update\n  apexkb --version"
    )]
    Update {
        /// Only report whether a newer release exists.
        #[arg(long)]
        check: bool,
        /// Reinstall the latest release even if it is not newer.
        #[arg(long, conflicts_with = "check")]
        force: bool,
    },
    /// Paint every key one color and remove all per-key overrides.
    ///
    /// Same as `effect static --color COLOR` followed by `clear`.
    #[command(after_long_help = "EXAMPLES\n  apexkb fill '#002244'\n  apexkb fill warm")]
    Fill { color: String },
    /// Give keys fixed colors on top of the current effect.
    ///
    /// Takes alternating SELECTOR COLOR pairs. Overrides stay until cleared
    /// with `clear`, `fill`, or by applying a profile.
    #[command(
        arg_required_else_help = true,
        after_long_help = "EXAMPLES\n  apexkb set wasd red\n  apexkb set wasd red esc white 'f1 f2 f3' '#00ff88'\n  apexkb set 0x87 blue"
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
    /// Animated effects are rendered by the daemon at about 30 fps.
    #[command(
        verbatim_doc_comment,
        about = "Set the whole-board lighting effect",
        after_long_help = "EXAMPLES\n  apexkb effect wave --period-ms 2500 --direction left\n  apexkb effect breathe -c cyan -p 4000\n  apexkb effect gradient --from purple --to orange -d down\n  apexkb effect twinkle -c white --background '#000022'\n  apexkb effect onboard"
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
    /// Light keys up when they are pressed.
    ///
    /// A layer above the effect and the overrides, so it combines with any
    /// effect. Options that do not apply to the chosen mode are ignored.
    ///   off      stop reading key presses (default)
    ///   keys     a pressed key shows --color while held, then fades out
    ///            over --fade-ms [500]
    ///   ripple   a ring of --color spreads from each pressed key at --speed
    ///            [20] key units per second, --width [1.5] keys thick, and
    ///            fades out over --fade-ms [1000]
    /// --color defaults to white.
    ///
    /// The daemon reads presses from the keyboard's own HID interfaces, only
    /// while a mode other than off is set. Typing works as usual. Presses are
    /// kept in memory only until their highlight fades, are never logged or
    /// saved, and `frame` leaves the reactive layer out.
    #[command(
        verbatim_doc_comment,
        about = "Light keys up when they are pressed",
        after_long_help = "EXAMPLES\n  apexkb reactive keys\n  apexkb reactive keys -c cyan --fade-ms 1000\n  apexkb reactive ripple -c orange --speed 30\n  apexkb effect off && apexkb reactive ripple   # ripples on a dark board\n  apexkb reactive off"
    )]
    Reactive {
        #[arg(value_enum)]
        mode: ReactiveName,
        /// Highlight color.
        #[arg(long, short)]
        color: Option<String>,
        /// How long a highlight takes to fade out.
        #[arg(long, short, value_name = "MS")]
        fade_ms: Option<u32>,
        /// How fast a ripple spreads, in key units per second.
        #[arg(long, value_name = "KEYS")]
        speed: Option<f32>,
        /// Thickness of a ripple ring, in key units.
        #[arg(long, value_name = "KEYS")]
        width: Option<f32>,
    },
    /// Set global lighting brightness, 0-100.
    ///
    /// Applied by the daemon to everything it draws. Onboard lighting itself
    /// follows the keyboard's own brightness keys instead.
    Brightness { percent: u8 },
    /// Highlight keys temporarily, steady or blinking.
    ///
    /// Flashes sit above overrides and expire on their own. Up to 64 can be
    /// active at once, and the newest wins where they overlap. Adding one
    /// beyond that ends the oldest early.
    #[command(
        after_long_help = "EXAMPLES\n  apexkb flash esc red\n  apexkb flash fkeys orange -d 5000 -b 250"
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
    /// Hand lighting back to the keyboard, or read and edit the profiles
    /// stored in it.
    ///
    /// Without a subcommand: same as `effect onboard`. Any overrides stay on
    /// their keys; run `clear` first to hand back the whole board.
    ///
    /// The keyboard stores five profiles (slots 1-5, GG's Config 1-5) that
    /// work with no software running: key remaps, a second layer of remaps
    /// used while the META key is held, macros and an OLED image. Per-key
    /// lighting is not part of them yet.
    ///
    /// Edits (remap, rename, oled, restore) read the slot, change only what
    /// they are about, and show the change without writing it. Add --write
    /// to store it: the slot's old bytes are saved under
    /// ~/.config/apexkb/onboard/, the slot is written, read back and
    /// compared, and the keyboard switches to it. `onboard restore` puts a
    /// saved slot back; `onboard read SLOT --bin FILE` saves one by hand.
    #[command(
        args_conflicts_with_subcommands = true,
        after_long_help = "EXAMPLES\n  apexkb onboard\n  apexkb onboard list\n  apexkb onboard read 1 --bin slot1.bin\n  apexkb onboard remap 1 capslock esc            # dry run\n  apexkb onboard remap 1 capslock esc --write\n  apexkb onboard remap 1 capslock default --write\n  apexkb onboard oled 2 cat.png --write\n  apexkb onboard select 2\n  apexkb onboard restore 1 slot1.bin --write"
    )]
    Onboard {
        #[command(subcommand)]
        cmd: Option<OnboardCmd>,
    },
    /// Print the color each key is being sent right now.
    Frame,
    /// Control the 128x40 OLED screen.
    #[command(subcommand)]
    Oled(OledCmd),
    /// Save, apply and manage named setups (lighting and OLED together).
    #[command(subcommand)]
    Profile(ProfileCmd),
    /// Send a request straight to the HTTP API (see `apexkb schema`).
    ///
    /// BODY is JSON, or @FILE to send a file's raw bytes (images).
    /// The response is always printed as JSON.
    #[command(
        after_long_help = "EXAMPLES\n  apexkb raw GET /v1/lighting\n  apexkb raw PUT /v1/lighting '{\"effect\":{\"type\":\"off\"},\"keys\":{\"esc\":\"red\"},\"brightness\":80}'\n  apexkb raw POST '/v1/oled/image?fit=cover' @photo.png"
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
        after_long_help = "EXAMPLES\n  apexkb oled text hello world\n  apexkb oled text --align left \"$(printf 'line one\\nline two')\""
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
        after_long_help = "EXAMPLES\n  apexkb oled image cat.gif\n  apexkb oled image logo.png --fit stretch --no-dither --threshold 100"
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
        after_long_help = "EXAMPLES\n  apexkb oled clock\n  apexkb oled clock -f '%H:%M:%S'\n  apexkb oled clock -f '%a %d %b'"
    )]
    Clock {
        /// strftime format.
        #[arg(long, short, default_value = "%H:%M")]
        format: String,
    },
    /// Show live CPU and RAM usage bars (the default mode).
    Sysinfo,
    /// Turn the screen off (stays off).
    Blank,
    /// Clear the screen and stop drawing on it.
    Release,
    /// Show a message over the current mode for a while.
    #[command(
        after_long_help = "EXAMPLES\n  apexkb oled notify \"Build passed\"\n  apexkb oled notify -d 10000 \"Meeting in 5 min\""
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
enum OnboardCmd {
    /// List the five slots by name.
    List,
    /// Decode a slot: name, key mappings, lighting options, macros, OLED.
    ///
    /// Changes nothing on the keyboard. Mappings lists every key that does
    /// something other than type itself; stock profiles already bind the
    /// media controls (0xf1-0xf8) and some META functions.
    Read {
        slot: u8,
        /// Also save the slot's raw bytes to FILE (a backup for `restore`).
        #[arg(long, value_name = "FILE")]
        bin: Option<PathBuf>,
        /// Print the stored OLED image as ASCII art.
        #[arg(long)]
        oled: bool,
    },
    /// Switch the keyboard to a slot. Stored data is not touched.
    #[command(visible_alias = "slot")]
    Select { slot: u8 },
    /// Make KEY send TARGET, or `default` to make it type itself again.
    ///
    /// TARGET is any key name, modifiers included. A dry run unless --write.
    Remap {
        slot: u8,
        key: String,
        target: String,
        /// Edit the layer used while the META key is held.
        #[arg(long)]
        meta: bool,
        /// Store the change in the keyboard.
        #[arg(long)]
        write: bool,
    },
    /// Rename a slot (up to 23 ASCII characters). A dry run unless --write.
    Rename {
        slot: u8,
        name: String,
        /// Store the name in the keyboard.
        #[arg(long)]
        write: bool,
    },
    /// Store an image as the slot's OLED picture, shown while apexkb is not
    /// drawing on the screen. A dry run unless --write.
    ///
    /// Takes the same images and options as `oled image`; an animated GIF
    /// keeps its first frame.
    Oled {
        slot: u8,
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
        /// Store the image in the keyboard.
        #[arg(long)]
        write: bool,
    },
    /// Put back a slot saved with `read --bin`. A dry run unless --write.
    Restore {
        slot: u8,
        file: PathBuf,
        /// Store the saved slot in the keyboard.
        #[arg(long)]
        write: bool,
    },
    /// Read the raw bytes of an onboard file: profiles 0x0201-0x0205, or the
    /// undecoded lighting files 0x0301-0x0305.
    Dump {
        file: String,
        /// Bytes to read [default: 12288].
        #[arg(long)]
        len: Option<usize>,
        /// Save to FILE instead of printing a hex dump.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
    },
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
enum ReactiveName {
    Off,
    Keys,
    Ripple,
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
    walk(&mut root, "apexkb", &mut out);
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
        .or_else(|| std::env::var_os("APEXKB_SOCKET").map(PathBuf::from))
        .unwrap_or_else(apexkb_daemon::default_socket);

    if let Cmd::Daemon { tcp, config_dir } = cli.cmd {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info".into()),
            )
            .init();
        // The API is light, so a couple of workers is plenty; the keyboard
        // has its own render thread.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        return rt.block_on(apexkb_daemon::serve(apexkb_daemon::ServeOptions {
            socket,
            tcp,
            config_dir: config_dir.unwrap_or_else(apexkb_daemon::store::Store::default_dir),
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
        Cmd::Schema => (apexkb_daemon::api::schema(), |v| serde_json::to_string_pretty(v).unwrap()),
        Cmd::Reference => (Value::String(reference()), |v| v.as_str().unwrap_or_default().to_string()),
        Cmd::Update { check, force } => (update::run(check, force)?, update::describe),
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
        Cmd::Reactive { mode, color, fade_ms, speed, width } => {
            let mut r = json!({ "type": value_name(mode) });
            let o = r.as_object_mut().unwrap();
            let mut put = |k: &str, v: Option<Value>| {
                if let Some(v) = v {
                    o.insert(k.into(), v);
                }
            };
            put("color", color.map(Value::from));
            put("fade_ms", fade_ms.map(Value::from));
            put("speed", speed.map(Value::from));
            put("width", width.map(Value::from));
            (c.post("/v1/lighting/reactive", r)?, fmt_lighting)
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
        Cmd::Onboard { cmd: None } => (c.post("/v1/lighting/effect", json!({ "type": "onboard" }))?, fmt_lighting),
        Cmd::Onboard { cmd: Some(cmd) } => onboard(&c, cmd)?,
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
                None => "(OLED not driven by apexkb)".into(),
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

/// A response and how to print it for people.
type Reply = (Value, fn(&Value) -> String);

fn onboard(c: &Client, cmd: OnboardCmd) -> anyhow::Result<Reply> {
    let slots = "/v1/onboard/slots";
    Ok(match cmd {
        OnboardCmd::List => (c.get(slots)?, fmt_onboard_list),
        OnboardCmd::Read { slot, bin, oled } => {
            let q = if bin.is_some() { "?bytes=true" } else { "" };
            let resp = c.get(&format!("{slots}/{slot}{q}"))?;
            if let Some(path) = bin {
                let bytes = B64.decode(resp["bytes"].as_str().unwrap_or_default())?;
                std::fs::write(&path, bytes)
                    .with_context(|| format!("writing {}", path.display()))?;
            }
            (
                resp,
                if oled {
                    fmt_onboard_read_oled
                } else {
                    fmt_onboard_read
                },
            )
        }
        OnboardCmd::Select { slot } => {
            (c.post(&format!("{slots}/{slot}/select"), json!({}))?, |v| {
                format!("keyboard switched to slot {}", v["selected"])
            })
        }
        OnboardCmd::Remap {
            slot,
            key,
            target,
            meta,
            write,
        } => {
            let layer = if meta { "meta" } else { "base" };
            let body = json!({ "key": key, "to": target, "layer": layer, "write": write });
            (
                c.post(&format!("{slots}/{slot}/remap"), body)?,
                fmt_onboard_edit,
            )
        }
        OnboardCmd::Rename { slot, name, write } => (
            c.post(
                &format!("{slots}/{slot}/name"),
                json!({ "name": name, "write": write }),
            )?,
            fmt_onboard_edit,
        ),
        OnboardCmd::Oled {
            slot,
            path,
            fit,
            no_dither,
            threshold,
            invert,
            write,
        } => {
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let q = format!(
                "{slots}/{slot}/oled?fit={}&dither={}&threshold={threshold}&invert={invert}&write={write}",
                value_name(fit),
                !no_dither,
            );
            (c.request("POST", &q, Body::Bytes(bytes))?, fmt_onboard_edit)
        }
        OnboardCmd::Restore { slot, file, write } => {
            let bytes =
                std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            (
                c.request(
                    "PUT",
                    &format!("{slots}/{slot}?write={write}"),
                    Body::Bytes(bytes),
                )?,
                fmt_onboard_edit,
            )
        }
        OnboardCmd::Dump { file, len, out } => {
            let q = len.map(|l| format!("?len={l}")).unwrap_or_default();
            let resp = c.get(&format!("/v1/onboard/files/{}{q}", urlencode(&file)))?;
            let bytes = B64.decode(resp["bytes"].as_str().unwrap_or_default())?;
            match out {
                Some(path) => {
                    std::fs::write(&path, &bytes)
                        .with_context(|| format!("writing {}", path.display()))?;
                    (resp, |v| {
                        format!(
                            "saved {} bytes of file {}",
                            v["len"],
                            v["file"].as_str().unwrap_or("?")
                        )
                    })
                }
                None => (resp, fmt_hex_dump),
            }
        }
    })
}

fn fmt_onboard_list(v: &Value) -> String {
    v["slots"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| match s["error"].as_str() {
            Some(e) => format!("{}  ({e})", s["slot"]),
            None => {
                let bad = if s["checksum_ok"] == true {
                    ""
                } else {
                    "  (checksum mismatch)"
                };
                format!("{}  {}{bad}", s["slot"], s["name"].as_str().unwrap_or("?"))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn describe_action(a: &Value) -> String {
    let field = |k: &str| a[k].as_str().unwrap_or("?").to_string();
    match a["type"].as_str() {
        Some("default") => "itself".into(),
        Some("key") => field("key"),
        Some("combo") => a["keys"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("+"),
        Some("mouse") => format!("mouse button {}", a["button"]),
        Some("media") => format!("media {}", field("codes")),
        Some("meta") => format!("META function {}", field("codes")),
        Some("macro") => format!("macro (binding {})", a["index"]),
        _ => format!(
            "function 0x{:02x} {}",
            a["function"].as_u64().unwrap_or(0),
            field("codes")
        ),
    }
}

fn describe_onboard(p: &Value) -> String {
    let l = &p["lighting"];
    let color = |k: &str| l[k].as_str().unwrap_or("?").to_string();
    let idle = match l["idle_timeout_ms"].as_u64() {
        Some(0) | None => "off".to_string(),
        Some(ms) => format!("{} s", ms / 1000),
    };
    let checksum = if p["checksum_ok"] == true {
        "checksum ok"
    } else {
        "CHECKSUM MISMATCH"
    };
    let mut out = format!(
        "name: {}\nschema: {} ({checksum})\nmeta key: {}\nlighting: lock keys {}, META keys {}, disabled win key {}, idle timeout {idle}\nmacros: {}\n",
        p["name"].as_str().unwrap_or("?"),
        p["schema"].as_str().unwrap_or("?"),
        p["meta_key"].as_str().unwrap_or("none"),
        color("lock_key_color"),
        color("meta_key_color"),
        color("disabled_win_key_color"),
        match p["macro_bytes"].as_u64() {
            Some(0) | None => "none".to_string(),
            Some(n) => format!("{n} bytes in use"),
        },
    );
    let mappings: Vec<_> = p["mappings"].as_array().into_iter().flatten().collect();
    if mappings.is_empty() {
        out += "mappings: every key types itself";
    } else {
        out += "mappings:";
        for m in mappings {
            out += &format!(
                "\n  {:<5} {:<12} {}",
                m["layer"].as_str().unwrap_or("?"),
                m["key"].as_str().unwrap_or("?"),
                describe_action(&m["action"])
            );
        }
    }
    out
}

fn oled_ascii(p: &Value) -> Option<String> {
    let bytes = B64.decode(p["oled"].as_str()?).ok()?;
    Some(
        apexkb_core::Frame::from_bytes(&bytes)?
            .to_ascii()
            .trim_end()
            .to_string(),
    )
}

fn fmt_onboard_read(v: &Value) -> String {
    let p = &v["profile"];
    let lit = B64
        .decode(p["oled"].as_str().unwrap_or_default())
        .map(|b| b.iter().map(|x| x.count_ones()).sum::<u32>())
        .unwrap_or(0);
    format!(
        "slot {}\n{}\noled: {}",
        v["slot"],
        describe_onboard(p),
        if lit == 0 {
            "blank".to_string()
        } else {
            format!("{lit} pixels lit (--oled to show)")
        }
    )
}

fn fmt_onboard_read_oled(v: &Value) -> String {
    let p = &v["profile"];
    format!(
        "slot {}\n{}\noled:\n{}",
        v["slot"],
        describe_onboard(p),
        oled_ascii(p).unwrap_or_else(|| "?".into())
    )
}

fn fmt_onboard_edit(v: &Value) -> String {
    let head = format!(
        "slot {} {:?}: {}",
        v["slot"],
        v["after"]["name"].as_str().unwrap_or("?"),
        v["change"].as_str().unwrap_or("?")
    );
    if v["changed"] != true {
        format!("{head}\nalready set; nothing to write")
    } else if v["written"] == true {
        format!(
            "{head}\nwritten and verified; the keyboard now runs slot {}\nprevious contents saved to {}",
            v["slot"],
            v["backup"].as_str().unwrap_or("?")
        )
    } else {
        format!("{head}\ndry run: nothing was written. Add --write to store it in the keyboard.")
    }
}

fn fmt_hex_dump(v: &Value) -> String {
    let bytes = B64
        .decode(v["bytes"].as_str().unwrap_or_default())
        .unwrap_or_default();
    bytes
        .chunks(16)
        .enumerate()
        .map(|(i, row)| {
            let hex: Vec<_> = row.iter().map(|b| format!("{b:02x}")).collect();
            format!("{:06x}  {}", i * 16, hex.join(" "))
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        "effect: {}\nkey overrides: {overrides}\nreactive: {}\nbrightness: {}%",
        describe_effect(&l["effect"]),
        describe_effect(&l["reactive"]),
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
    if let Some(daemon) = v["version"].as_str()
        && daemon != apexkb_daemon::VERSION
    {
        out += &format!(
            "daemon: {daemon} (this CLI is {}; restart the daemon)\n",
            apexkb_daemon::VERSION
        );
    }
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
        "onboard lighting, with these keys set by apexkb:\n"
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
