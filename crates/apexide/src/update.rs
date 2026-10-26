//! Self-update from GitHub releases.
//!
//! Every merge to main publishes a release `vX.Y.N` with a static Linux binary
//! named `apexide-<arch>-linux` and its `.sha256`. Updating downloads that
//! binary next to the running one, checks the hash and renames it over the
//! old file, which is safe while the old binary (or the daemon) is running.

use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, anyhow, bail};
use apexide_daemon::VERSION;
use semver::Version;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const REPO: &str = "estridell/apexide";
const SERVICE: &str = "apexide.service";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub fn asset_name() -> String {
    format!("apexide-{}-linux", std::env::consts::ARCH)
}

/// Check the latest release and, unless `check_only`, install it over the
/// running binary and restart the user service if it is active.
pub fn run(check_only: bool, force: bool) -> anyhow::Result<Value> {
    let current = Version::parse(VERSION).context("parsing this build's version")?;
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = match get(&url) {
        Err(e) if matches!(e.downcast_ref(), Some(ureq::Error::StatusCode(404))) => {
            bail!("{REPO} has no published releases yet")
        }
        r => r?,
    }
    .read_to_string()
    .context("reading the release info")?;
    let release: Release = serde_json::from_str(&body).context("parsing the release info")?;
    let latest = Version::parse(release.tag_name.trim_start_matches('v'))
        .with_context(|| format!("release tag {:?} is not a version", release.tag_name))?;
    let available = latest > current;
    let mut out = json!({
        "current": VERSION,
        "latest": latest.to_string(),
        "update_available": available,
        "release_url": release.html_url,
        "updated": false,
    });
    if check_only || !(available || force) {
        return Ok(out);
    }

    let exe = std::env::current_exe().context("locating the running binary")?;
    let name = asset_name();
    let url_of = |n: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name == n)
            .map(|a| a.browser_download_url.as_str())
            .ok_or_else(|| anyhow!("release {} has no {n} asset", release.tag_name))
    };
    let (bin_url, sum_url) = (url_of(&name)?, url_of(&format!("{name}.sha256"))?);

    let expected = get(sum_url)?.read_to_string()?;
    let expected = expected
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow!("empty checksum file"))?
        .to_ascii_lowercase();
    let bytes = get(bin_url)?
        .with_config()
        .limit(200 * 1024 * 1024)
        .read_to_vec()
        .context("downloading the binary")?;
    let actual: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != expected {
        bail!("checksum mismatch for {name}: expected {expected}, got {actual}");
    }
    replace_binary(&exe, &bytes)?;
    out["updated"] = true.into();
    out["path"] = exe.display().to_string().into();
    out["service_restarted"] = restart_service().into();
    Ok(out)
}

fn get(url: &str) -> anyhow::Result<ureq::Body> {
    let resp = ureq::get(url)
        .header("User-Agent", &format!("apexide/{VERSION}"))
        .header("Accept", "application/vnd.github+json")
        .call()
        .with_context(|| format!("GET {url}"))?;
    Ok(resp.into_body())
}

/// Write `bytes` beside `exe` and atomically rename it into place.
fn replace_binary(exe: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", exe.display()))?;
    let tmp: PathBuf = dir.join(".apexide.update");
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o755))?;
        f.sync_all()?;
        std::fs::rename(&tmp, exe)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        let hint = if e.kind() == std::io::ErrorKind::PermissionDenied {
            " (install apexide somewhere you can write, like ~/.local/bin, or run with sudo)"
        } else {
            ""
        };
        anyhow!("replacing {}: {e}{hint}", exe.display())
    })
}

/// Restart the systemd user service if it is running. Returns whether it did.
fn restart_service() -> bool {
    let systemctl = |args: &[&str]| {
        Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    systemctl(&["is-active", "--quiet", SERVICE]) && systemctl(&["restart", SERVICE])
}

pub fn describe(v: &Value) -> String {
    let current = v["current"].as_str().unwrap_or("?");
    let latest = v["latest"].as_str().unwrap_or("?");
    if v["updated"] == true {
        let mut s = format!(
            "updated apexide {current} -> {latest} ({})",
            v["path"].as_str().unwrap_or("?")
        );
        if v["service_restarted"] == true {
            s += "\nrestarted apexide.service";
        } else {
            s += "\nrestart the daemon if it is running to use the new version";
        }
        s
    } else if v["update_available"] == true {
        format!("apexide {latest} is available (this is {current}); run `apexide update`")
    } else {
        format!("apexide {current} is up to date (latest release: {latest})")
    }
}
