//! HTTP/JSON API. Every mutation is persisted so the daemon comes back in the
//! same state after a restart.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use apexkb_core::{
    FormFactor, KeyCode, Rgb, keys,
    oled::{self, Align, ImageOptions},
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, FromRequest, Path, Query, Request, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    engine::Waker,
    state::{Effect, Flash, Lighting, MAX_FLASHES, Notification, OledMode, Shared, default_clock},
    store::{Store, StoreError},
};

#[derive(Clone)]
pub struct App {
    pub shared: Arc<Mutex<Shared>>,
    pub store: Store,
    wake: Waker,
    /// Held from changing the settings until they are on disk, so saves
    /// land in the same order as the changes.
    saving: Arc<tokio::sync::Mutex<()>>,
}

// ---------------------------------------------------------------------------
// Errors and extractors

pub struct ApiError(StatusCode, String);

impl ApiError {
    fn bad(msg: impl ToString) -> Self {
        Self(StatusCode::BAD_REQUEST, msg.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        let code = match e {
            StoreError::BadName => StatusCode::BAD_REQUEST,
            StoreError::NotFound(_) => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self(code, e.to_string())
    }
}

impl From<keys::UnknownKey> for ApiError {
    fn from(e: keys::UnknownKey) -> Self {
        Self::bad(e)
    }
}

/// `Json` with errors reported as `{"error": ...}`.
pub struct J<T>(T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for J<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(v)) => Ok(J(v)),
            Err(e @ JsonRejection::MissingJsonContentType(_)) => {
                Err(ApiError(e.status(), e.body_text()))
            }
            Err(e) => Err(ApiError::bad(e.body_text())),
        }
    }
}

type Res = Result<Json<Value>, ApiError>;

/// Run blocking store I/O off the async worker threads.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// Helpers

impl App {
    pub fn new(shared: Arc<Mutex<Shared>>, store: Store, wake: Waker) -> Self {
        Self {
            shared,
            store,
            wake,
            saving: Default::default(),
        }
    }

    fn form(&self) -> FormFactor {
        self.shared
            .lock()
            .unwrap()
            .device
            .as_ref()
            .map(|d| d.form_factor)
            .unwrap_or(FormFactor::Tkl)
    }

    /// Apply `f` to the lighting, persist it, and reply with the result.
    async fn lighting_reply(&self, f: impl FnOnce(&mut Lighting)) -> Res {
        let _saving = self.saving.lock().await;
        let lighting = {
            let mut s = self.shared.lock().unwrap();
            f(&mut s.settings.lighting);
            s.settings.lighting.clone()
        };
        self.wake.wake();
        let reply = json!(lighting);
        let store = self.store.clone();
        blocking(move || store.save_lighting(&lighting)).await?;
        Ok(Json(reply))
    }

    /// Switch the OLED to `mode`, persist it, and reply with its summary.
    async fn oled_reply(&self, mode: OledMode) -> Res {
        let _saving = self.saving.lock().await;
        let reply = json!({ "mode": mode.summary() });
        let saved = mode.clone();
        {
            let mut s = self.shared.lock().unwrap();
            s.settings.oled = mode;
            s.oled_since = Some(Instant::now());
        }
        self.wake.wake();
        let store = self.store.clone();
        blocking(move || store.save_oled(&saved)).await?;
        Ok(Json(reply))
    }
}

fn resolve(selector: &str, form: FormFactor) -> Result<Vec<KeyCode>, ApiError> {
    let codes = keys::resolve(selector, form)?;
    if codes.is_empty() {
        return Err(ApiError::bad(format!(
            "{selector:?} matches no keys on this keyboard"
        )));
    }
    Ok(codes)
}

// ---------------------------------------------------------------------------
// Routes

pub fn router(app: App) -> Router {
    Router::new()
        .route("/v1/status", get(status))
        .route("/v1/keys", get(list_keys))
        .route("/v1/schema", get(schema_handler))
        .route("/v1/lighting", get(get_lighting).put(put_lighting))
        .route("/v1/lighting/effect", post(set_effect))
        .route("/v1/lighting/fill", post(fill))
        .route("/v1/lighting/keys", post(set_keys))
        .route("/v1/lighting/keys/clear", post(clear_keys))
        .route("/v1/lighting/brightness", put(set_brightness))
        .route("/v1/lighting/flash", post(flash))
        .route("/v1/lighting/frame", get(lighting_frame))
        .route("/v1/oled", get(get_oled).put(put_oled))
        .route("/v1/oled/preview", get(oled_preview))
        .route("/v1/oled/text", post(oled_text))
        .route(
            "/v1/oled/image",
            post(oled_image).layer(DefaultBodyLimit::max(32 << 20)),
        )
        .route("/v1/oled/clock", post(oled_clock))
        .route("/v1/oled/sysinfo", post(oled_sysinfo))
        .route("/v1/oled/blank", post(oled_blank))
        .route("/v1/oled/release", post(oled_release))
        .route("/v1/oled/notify", post(oled_notify))
        .route("/v1/profiles", get(list_profiles))
        .route(
            "/v1/profiles/{name}",
            get(get_profile).put(save_profile).delete(delete_profile),
        )
        .route("/v1/profiles/{name}/apply", post(apply_profile))
        .fallback(|| async {
            ApiError(
                StatusCode::NOT_FOUND,
                "no such endpoint; see GET /v1/schema".into(),
            )
        })
        .with_state(app)
}

type St = axum::extract::State<App>;

async fn status(st: St) -> Res {
    let store = st.store.clone();
    let profiles = blocking(move || store.list_profiles())
        .await
        .unwrap_or_default();
    let s = st.shared.lock().unwrap();
    Ok(Json(json!({
        "version": crate::VERSION,
        "connected": s.device.is_some(),
        "device": s.device,
        "error": s.last_error,
        "lighting": s.settings.lighting,
        "oled": s.settings.oled.summary(),
        "notification": s.notification.as_ref().map(|n| &n.text),
        "active_flashes": s.flashes.len(),
        "profiles": profiles,
    })))
}

#[derive(Deserialize)]
struct FormQuery {
    form: Option<FormFactor>,
}

async fn list_keys(st: St, Query(q): Query<FormQuery>) -> Res {
    let form = q.form.unwrap_or_else(|| st.form());
    let (w, h) = form.size();
    let keys: Vec<_> = form.keys().collect();
    let groups: Vec<_> = keys::GROUPS
        .iter()
        .map(|(n, d)| json!({ "name": n, "description": d }))
        .collect();
    Ok(Json(
        json!({ "form_factor": form, "size": [w, h], "keys": keys, "groups": groups }),
    ))
}

async fn get_lighting(st: St) -> Res {
    Ok(Json(json!(st.shared.lock().unwrap().settings.lighting)))
}

async fn put_lighting(st: St, J(l): J<crate::state::Lighting>) -> Res {
    st.lighting_reply(|lighting| *lighting = l).await
}

async fn set_effect(st: St, J(effect): J<Effect>) -> Res {
    st.lighting_reply(|l| l.effect = effect).await
}

#[derive(Deserialize)]
struct FillReq {
    color: Rgb,
}

async fn fill(st: St, J(r): J<FillReq>) -> Res {
    st.lighting_reply(|l| {
        l.effect = Effect::Static { color: r.color };
        l.keys.clear();
    })
    .await
}

#[derive(Deserialize)]
struct SetKeysReq {
    /// Selector (key names, groups) -> color.
    keys: BTreeMap<String, Rgb>,
}

async fn set_keys(st: St, J(r): J<SetKeysReq>) -> Res {
    let form = st.form();
    let mut resolved = Vec::new();
    for (sel, color) in &r.keys {
        for code in resolve(sel, form)? {
            resolved.push((code, *color));
        }
    }
    st.lighting_reply(|l| l.keys.extend(resolved)).await
}

#[derive(Deserialize)]
struct ClearKeysReq {
    /// Selector to clear; all overrides when omitted.
    keys: Option<String>,
}

async fn clear_keys(st: St, J(r): J<ClearKeysReq>) -> Res {
    let codes = r.keys.map(|k| resolve(&k, st.form())).transpose()?;
    st.lighting_reply(|l| match codes {
        Some(c) => l.keys.retain(|k, _| !c.contains(k)),
        None => l.keys.clear(),
    })
    .await
}

#[derive(Deserialize)]
struct BrightnessReq {
    brightness: u8,
}

async fn set_brightness(st: St, J(r): J<BrightnessReq>) -> Res {
    if r.brightness > 100 {
        return Err(ApiError::bad("brightness must be 0-100"));
    }
    st.lighting_reply(|l| l.brightness = r.brightness).await
}

#[derive(Deserialize)]
struct FlashReq {
    keys: String,
    color: Rgb,
    #[serde(default = "default_flash_ms")]
    duration_ms: u64,
    blink_ms: Option<u32>,
}

fn default_flash_ms() -> u64 {
    1000
}

async fn flash(st: St, J(r): J<FlashReq>) -> Res {
    let keys = resolve(&r.keys, st.form())?;
    let now = Instant::now();
    let until = now + Duration::from_millis(r.duration_ms.min(600_000));
    {
        let mut s = st.shared.lock().unwrap();
        if s.flashes.len() >= MAX_FLASHES {
            let excess = s.flashes.len() + 1 - MAX_FLASHES;
            s.flashes.drain(..excess);
        }
        s.flashes.push(Flash {
            keys,
            color: r.color,
            started: now,
            until,
            blink_ms: r.blink_ms,
        });
    }
    st.wake.wake();
    Ok(Json(json!({ "ok": true, "duration_ms": r.duration_ms })))
}

async fn lighting_frame(st: St) -> Res {
    use crate::effects::LightingFrame;
    let s = st.shared.lock().unwrap();
    let mode = match s.frame_lighting {
        LightingFrame::Onboard => "onboard",
        LightingFrame::Full(_) => "software",
        LightingFrame::Overlay(_) => "overlay",
    };
    let keys: BTreeMap<String, Rgb> = s
        .frame_lighting
        .colors()
        .iter()
        .map(|(k, c)| (k.to_string(), *c))
        .collect();
    Ok(Json(json!({ "mode": mode, "keys": keys })))
}

async fn get_oled(st: St) -> Res {
    let s = st.shared.lock().unwrap();
    Ok(Json(json!({
        "mode": s.settings.oled.summary(),
        "notification": s.notification.as_ref().map(|n| &n.text),
    })))
}

async fn put_oled(st: St, J(mode): J<OledMode>) -> Res {
    validate_oled(&mode)?;
    st.oled_reply(mode).await
}

fn validate_oled(mode: &OledMode) -> Result<(), ApiError> {
    match mode {
        OledMode::Clock { format } => validate_clock(format),
        OledMode::Image { frames, .. } if frames.is_empty() => {
            Err(ApiError::bad("image needs at least one frame"))
        }
        _ => Ok(()),
    }
}

fn validate_clock(format: &str) -> Result<(), ApiError> {
    use chrono::format::{Item, StrftimeItems};
    if StrftimeItems::new(format).any(|i| matches!(i, Item::Error)) {
        return Err(ApiError::bad(format!("invalid strftime format {format:?}")));
    }
    Ok(())
}

/// What the OLED is showing right now, as ASCII art.
async fn oled_preview(st: St) -> Res {
    let s = st.shared.lock().unwrap();
    Ok(Json(
        json!({ "ascii": s.frame_oled.as_ref().map(|f| f.to_ascii()) }),
    ))
}

#[derive(Deserialize)]
struct TextReq {
    text: String,
    #[serde(default)]
    align: Align,
}

async fn oled_text(st: St, J(r): J<TextReq>) -> Res {
    st.oled_reply(OledMode::Text {
        text: r.text,
        align: r.align,
    })
    .await
}

/// Query parameters for image uploads. Spelled out instead of flattening
/// `ImageOptions`, because flattened query strings cannot parse non-strings.
#[derive(Deserialize)]
struct ImageQuery {
    name: Option<String>,
    fit: Option<oled::Fit>,
    dither: Option<bool>,
    threshold: Option<u8>,
    invert: Option<bool>,
}

async fn oled_image(st: St, Query(q): Query<ImageQuery>, body: Bytes) -> Res {
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
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(ApiError::bad)?;
    st.oled_reply(OledMode::Image {
        name: q.name.unwrap_or_else(|| "image".into()),
        frames,
    })
    .await
}

#[derive(Deserialize, Default)]
struct ClockReq {
    format: Option<String>,
}

async fn oled_clock(st: St, body: Bytes) -> Res {
    let req: ClockReq = if body.is_empty() {
        ClockReq::default()
    } else {
        serde_json::from_slice(&body).map_err(ApiError::bad)?
    };
    let format = req.format.unwrap_or_else(default_clock);
    validate_clock(&format)?;
    st.oled_reply(OledMode::Clock { format }).await
}

async fn oled_sysinfo(st: St) -> Res {
    st.oled_reply(OledMode::Sysinfo).await
}

async fn oled_blank(st: St) -> Res {
    st.oled_reply(OledMode::Blank).await
}

async fn oled_release(st: St) -> Res {
    st.oled_reply(OledMode::Release).await
}

#[derive(Deserialize)]
struct NotifyReq {
    text: String,
    #[serde(default = "default_notify_ms")]
    duration_ms: u64,
}

fn default_notify_ms() -> u64 {
    5000
}

async fn oled_notify(st: St, J(r): J<NotifyReq>) -> Res {
    let until = Instant::now() + Duration::from_millis(r.duration_ms.min(600_000));
    st.shared.lock().unwrap().notification = Some(Notification {
        text: r.text,
        until,
    });
    st.wake.wake();
    Ok(Json(json!({ "ok": true, "duration_ms": r.duration_ms })))
}

async fn list_profiles(st: St) -> Res {
    let store = st.store.clone();
    let profiles = blocking(move || store.list_profiles()).await?;
    Ok(Json(json!({ "profiles": profiles })))
}

async fn get_profile(st: St, Path(name): Path<String>) -> Res {
    let (store, n) = (st.store.clone(), name.clone());
    let p = blocking(move || store.load_profile(&n)).await?;
    Ok(Json(
        json!({ "name": name, "lighting": p.lighting, "oled": p.oled.summary() }),
    ))
}

async fn save_profile(st: St, Path(name): Path<String>) -> Res {
    let settings = st.shared.lock().unwrap().settings.clone();
    let (store, n) = (st.store.clone(), name.clone());
    blocking(move || store.save_profile(&n, &settings)).await?;
    Ok(Json(json!({ "saved": name })))
}

async fn apply_profile(st: St, Path(name): Path<String>) -> Res {
    let (store, n) = (st.store.clone(), name.clone());
    let p = blocking(move || store.load_profile(&n)).await?;
    let _saving = st.saving.lock().await;
    {
        let mut s = st.shared.lock().unwrap();
        s.settings = p.clone();
        s.oled_since = Some(Instant::now());
    }
    st.wake.wake();
    let store = st.store.clone();
    blocking(move || store.save_state(&p)).await?;
    Ok(Json(json!({ "applied": name })))
}

async fn delete_profile(st: St, Path(name): Path<String>) -> Res {
    let (store, n) = (st.store.clone(), name.clone());
    blocking(move || store.delete_profile(&n)).await?;
    Ok(Json(json!({ "deleted": name })))
}

async fn schema_handler() -> Json<Value> {
    Json(schema())
}

/// Machine-readable overview of the API, aimed at agents and GUI builders.
pub fn schema() -> Value {
    json!({
        "colors": "#rrggbb, #rgb, or one of: off black white red green blue yellow orange cyan teal magenta purple pink warm",
        "key_selectors": "comma/space separated key names (GET /v1/keys), raw codes like 0x87, or groups: all letters numbers fkeys wasd arrows nav modifiers numpad",
        "effects": {
            "off": {},
            "static": { "color": "color" },
            "breathe": { "color": "color", "period_ms": 3000 },
            "spectrum": { "period_ms": 10000 },
            "wave": { "period_ms": 3000, "wavelength": 18.0, "direction": "right|left|up|down" },
            "gradient": { "from": "color", "to": "color", "direction": "right|left|up|down" },
            "twinkle": { "color": "color", "background": "#000000", "period_ms": 3000 },
            "onboard": {}
        },
        "oled_modes": {
            "release": "apexkb clears the OLED once and stops drawing on it",
            "blank": "screen off",
            "text": { "text": "string", "align": "left|center|right" },
            "image": "POST raw PNG/JPEG/BMP/GIF bytes to /v1/oled/image?fit=contain|cover|stretch&dither=true&invert=false&threshold=128",
            "clock": { "format": "%H:%M (strftime)" },
            "sysinfo": "CPU and RAM bars (default)"
        },
        "endpoints": [
            "GET  /v1/status",
            "GET  /v1/keys[?form=tkl|full]",
            "GET  /v1/lighting | PUT /v1/lighting {effect, keys, brightness}",
            "POST /v1/lighting/effect {type, ...params}",
            "POST /v1/lighting/fill {color}",
            "POST /v1/lighting/keys {keys: {selector: color}}",
            "POST /v1/lighting/keys/clear {keys?: selector}",
            "PUT  /v1/lighting/brightness {brightness: 0-100}",
            "POST /v1/lighting/flash {keys, color, duration_ms?, blink_ms?}",
            "GET  /v1/lighting/frame",
            "GET  /v1/oled | PUT /v1/oled {type, ...}",
            "GET  /v1/oled/preview",
            "POST /v1/oled/text {text, align?}",
            "POST /v1/oled/image?... (raw image body)",
            "POST /v1/oled/clock {format?}",
            "POST /v1/oled/sysinfo | /v1/oled/blank | /v1/oled/release",
            "POST /v1/oled/notify {text, duration_ms?}",
            "GET  /v1/profiles",
            "GET|PUT|DELETE /v1/profiles/{name}",
            "POST /v1/profiles/{name}/apply"
        ]
    })
}
