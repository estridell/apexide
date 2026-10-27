//! API for the keyboard's onboard profile slots.
//!
//! Slots are numbered 1-5 here, like GG's Config 1-5. Reads change nothing
//! on the keyboard. Every edit reads the slot first, changes only the bytes
//! it is about, and is a dry run unless the request asks to write. Before a
//! write the slot's old bytes are saved (see [`crate::store::Store`]), and
//! after it the slot is read back and compared.

use apexkb_core::{
    Keyboard, keys,
    oled::{self, ImageOptions},
    onboard::{self, Layer, Mapping, Profile, ProfileError},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query},
    http::StatusCode,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{ApiError, App, J};

type Res = Result<Json<Value>, ApiError>;
type St = axum::extract::State<App>;

/// Bytes read from an onboard file when no length is given: 12 buffers,
/// enough for either profile schema.
const DEFAULT_FILE_LEN: usize = 12 * onboard::BUFFER_LEN;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/v1/onboard/slots", get(list))
        .route(
            "/v1/onboard/slots/{slot}",
            get(read)
                .put(restore)
                .layer(DefaultBodyLimit::max(onboard::REGION_LEN)),
        )
        .route("/v1/onboard/slots/{slot}/select", post(select))
        .route("/v1/onboard/slots/{slot}/remap", post(remap))
        .route("/v1/onboard/slots/{slot}/name", post(rename))
        .route(
            "/v1/onboard/slots/{slot}/oled",
            post(set_oled).layer(DefaultBodyLimit::max(32 << 20)),
        )
        .route("/v1/onboard/files/{file}", get(read_file))
}

impl From<apexkb_core::Error> for ApiError {
    fn from(e: apexkb_core::Error) -> Self {
        use apexkb_core::Error as E;
        let code = match &e {
            E::Profile(ProfileError::Empty) => StatusCode::NOT_FOUND,
            E::Profile(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ApiError::new(code, e)
    }
}

impl From<ProfileError> for ApiError {
    fn from(e: ProfileError) -> Self {
        apexkb_core::Error::from(e).into()
    }
}

/// Run `f` on the render thread with the keyboard.
async fn on_keyboard<T: Send + 'static>(
    app: &App,
    f: impl FnOnce(&mut Keyboard) -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.wake.run(Box::new(move |kb| {
        let result = match kb {
            Some(kb) => f(kb),
            None => Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "no keyboard connected",
            )),
        };
        tx.send(result).ok();
    }));
    rx.await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "the daemon is shutting down",
        )
    })?
}

/// Slot number from the path, 1-5, as the 0-based index the keyboard uses.
fn slot_index(slot: u8) -> Result<u8, ApiError> {
    if (1..=onboard::SLOTS).contains(&slot) {
        Ok(slot - 1)
    } else {
        Err(ApiError::bad(format!(
            "slot {slot} does not exist (1-{})",
            onboard::SLOTS
        )))
    }
}

/// Read `slot`, apply `edit`, and write the result if `write` and something
/// changed. `edit` returns a description of the change.
async fn edit_slot(
    app: &App,
    slot: u8,
    write: bool,
    edit: impl FnOnce(&mut Profile) -> Result<String, ApiError> + Send + 'static,
) -> Res {
    let index = slot_index(slot)?;
    let store = app.store.clone();
    on_keyboard(app, move |kb| {
        let before = kb.read_profile(index)?;
        if !before.checksum_ok() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                format!("slot {slot} does not match its own checksum, so apexkb will not edit it"),
            ));
        }
        let mut after = before.clone();
        let change = edit(&mut after)?;
        let changed = after != before;
        let written = write && changed;
        let mut backup = None;
        if written {
            let path = store.backup_onboard(slot, before.bytes())?;
            backup = Some(path.display().to_string());
            kb.write_profile(index, &after)?;
            let back = kb.read_profile(index)?;
            if back != after {
                return Err(ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("slot {slot} read back different from what was written"),
                ));
            }
            tracing::info!(slot, change, "wrote onboard profile");
        }
        Ok(Json(json!({
            "slot": slot,
            "change": change,
            "changed": changed,
            "written": written,
            "selected": written,
            "backup": backup,
            "before": before.summary(),
            "after": after.summary(),
        })))
    })
    .await
}

async fn list(st: St) -> Res {
    on_keyboard(&st, |kb| {
        let slots: Vec<Value> = (0..onboard::SLOTS)
            .map(|i| match kb.read_profile(i) {
                Ok(p) => Ok(json!({
                    "slot": i + 1,
                    "name": p.name(),
                    "schema": p.schema(),
                    "checksum_ok": p.checksum_ok(),
                })),
                Err(apexkb_core::Error::Profile(e)) => {
                    Ok(json!({ "slot": i + 1, "error": e.to_string() }))
                }
                Err(e) => Err(ApiError::from(e)),
            })
            .collect::<Result<_, _>>()?;
        Ok(Json(json!({ "slots": slots })))
    })
    .await
}

#[derive(Deserialize)]
struct ReadQuery {
    /// Include the raw payload, base64.
    #[serde(default)]
    bytes: bool,
}

async fn read(st: St, Path(slot): Path<u8>, Query(q): Query<ReadQuery>) -> Res {
    let index = slot_index(slot)?;
    on_keyboard(&st, move |kb| {
        let p = kb.read_profile(index)?;
        let mut reply = json!({ "slot": slot, "profile": p.summary() });
        if q.bytes {
            reply["bytes"] = json!(B64.encode(p.bytes()));
        }
        Ok(Json(reply))
    })
    .await
}

async fn select(st: St, Path(slot): Path<u8>) -> Res {
    let index = slot_index(slot)?;
    on_keyboard(&st, move |kb| {
        kb.select_profile(index)?;
        Ok(Json(json!({ "selected": slot })))
    })
    .await
}

#[derive(Deserialize)]
struct WriteQuery {
    #[serde(default)]
    write: bool,
}

/// Replace a slot with a payload saved earlier (`GET ...?bytes=true`).
async fn restore(st: St, Path(slot): Path<u8>, Query(q): Query<WriteQuery>, body: Bytes) -> Res {
    let new = Profile::parse(&body)?;
    if !new.checksum_ok() {
        return Err(ApiError::bad("the profile does not match its own checksum"));
    }
    edit_slot(&st, slot, q.write, move |p| {
        if new.schema() != p.schema() {
            return Err(ApiError::bad(format!(
                "the profile is schema {:?}, but the slot holds schema {:?}",
                new.schema(),
                p.schema()
            )));
        }
        let change = format!("restore {:?}", new.name());
        *p = new;
        Ok(change)
    })
    .await
}

#[derive(Deserialize)]
struct RemapReq {
    key: String,
    /// A key name, or `default` for the key itself.
    to: String,
    #[serde(default)]
    layer: Layer,
    #[serde(default)]
    write: bool,
}

/// Keys a mapping can send: HID keyboard usages, modifiers included.
const TARGETS: std::ops::RangeInclusive<u8> = 0x04..=0xE7;

async fn remap(st: St, Path(slot): Path<u8>, J(r): J<RemapReq>) -> Res {
    let key = keys::parse_one(&r.key).ok_or_else(|| keys::UnknownKey(r.key.clone()))?;
    let to = if r.to.eq_ignore_ascii_case("default") {
        key
    } else {
        let to = keys::parse_one(&r.to).ok_or_else(|| keys::UnknownKey(r.to.clone()))?;
        if !TARGETS.contains(&to.0) {
            return Err(ApiError::bad(format!("{to} cannot be sent by a mapping")));
        }
        to
    };
    let layer = r.layer;
    edit_slot(&st, slot, r.write, move |p| {
        if p.meta_key() == Some(key) && to != key {
            return Err(ApiError::bad(format!(
                "{key} is this profile's META key; remapping it would lock out the META layer"
            )));
        }
        let mapping = if to == key {
            Mapping::stock(layer, key)
        } else {
            Mapping::key(to)
        };
        p.set_mapping(layer, key, mapping)?;
        let layer = match layer {
            Layer::Base => "base",
            Layer::Meta => "meta",
        };
        Ok(if to == key {
            format!("{key} types itself ({layer} layer)")
        } else {
            format!("{key} -> {to} ({layer} layer)")
        })
    })
    .await
}

#[derive(Deserialize)]
struct NameReq {
    name: String,
    #[serde(default)]
    write: bool,
}

async fn rename(st: St, Path(slot): Path<u8>, J(r): J<NameReq>) -> Res {
    edit_slot(&st, slot, r.write, move |p| {
        p.set_name(&r.name)?;
        Ok(format!("name {:?}", r.name))
    })
    .await
}

#[derive(Deserialize)]
struct OledQuery {
    fit: Option<oled::Fit>,
    dither: Option<bool>,
    threshold: Option<u8>,
    invert: Option<bool>,
    #[serde(default)]
    write: bool,
}

/// Store an image as the slot's OLED picture. An animated image keeps its
/// first frame.
async fn set_oled(st: St, Path(slot): Path<u8>, Query(q): Query<OledQuery>, body: Bytes) -> Res {
    if body.is_empty() {
        return Err(ApiError::bad("send the image file as the request body"));
    }
    let d = ImageOptions::default();
    let opts = ImageOptions {
        fit: q.fit.unwrap_or(d.fit),
        dither: q.dither.unwrap_or(d.dither),
        threshold: q.threshold.unwrap_or(d.threshold),
        invert: q.invert.unwrap_or(d.invert),
    };
    let frames = tokio::task::spawn_blocking(move || oled::image_frames(&body, opts))
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map_err(ApiError::bad)?;
    let count = frames.len();
    let frame = frames
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::bad("the image has no frames"))?
        .frame;
    edit_slot(&st, slot, q.write, move |p| {
        p.set_oled(&frame);
        Ok(if count > 1 {
            format!("OLED image (first of {count} frames)")
        } else {
            "OLED image".into()
        })
    })
    .await
}

#[derive(Deserialize)]
struct FileQuery {
    len: Option<usize>,
}

/// Raw bytes of an onboard file, for working out what the files hold. Only
/// the profile (`0x0201`-`0x0205`) and lighting (`0x0301`-`0x0305`) files
/// can be read.
async fn read_file(st: St, Path(file): Path<String>, Query(q): Query<FileQuery>) -> Res {
    let id = u16::from_str_radix(file.trim_start_matches("0x"), 16)
        .map_err(|_| ApiError::bad(format!("{file:?} is not a hex file id")))?;
    let known = [onboard::PROFILE_FILE, onboard::LIGHTING_FILE]
        .iter()
        .any(|&base| (base..base + u16::from(onboard::SLOTS)).contains(&id));
    if !known {
        return Err(ApiError::bad(format!(
            "file 0x{id:04x} is not a profile (0x0201-0x0205) or lighting (0x0301-0x0305) file"
        )));
    }
    let len = q.len.unwrap_or(DEFAULT_FILE_LEN);
    if len == 0 || len > onboard::REGION_LEN {
        return Err(ApiError::bad(format!(
            "len must be 1-{}",
            onboard::REGION_LEN
        )));
    }
    on_keyboard(&st, move |kb| {
        let bytes = kb.read_onboard_file(id, len)?;
        Ok(Json(json!({
            "file": format!("0x{id:04x}"),
            "len": bytes.len(),
            "bytes": B64.encode(&bytes),
        })))
    })
    .await
}
