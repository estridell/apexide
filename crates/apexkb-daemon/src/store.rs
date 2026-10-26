//! On-disk persistence: current settings and named profiles, as JSON under
//! `$XDG_CONFIG_HOME/apexkb`.
//!
//! The current lighting lives in `state.json` and the OLED mode in
//! `oled.json`, so lighting changes do not rewrite image frames.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Serialize, de::DeserializeOwned};

use crate::state::{Lighting, OledMode, Settings};

/// Move settings saved before the rename from apexide (the sibling
/// `apexide` dir) to `dir`, unless `dir` already exists.
pub fn migrate_legacy_dir(dir: &Path) {
    let legacy = dir.with_file_name("apexide");
    if !dir.exists() && legacy.is_dir() {
        fs::rename(&legacy, dir).ok();
    }
}

#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("profile names may only contain letters, digits, '-' and '_'")]
    BadName,
    #[error("no profile named {0:?}")]
    NotFound(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("corrupt settings file: {0}")]
    Json(#[from] serde_json::Error),
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn default_dir() -> PathBuf {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("apexkb")
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    fn oled_path(&self) -> PathBuf {
        self.dir.join("oled.json")
    }

    fn profiles_dir(&self) -> PathBuf {
        self.dir.join("profiles")
    }

    fn profile_path(&self, name: &str) -> Result<PathBuf, StoreError> {
        let ok = !name.is_empty()
            && name.len() <= 64
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !ok {
            return Err(StoreError::BadName);
        }
        Ok(self.profiles_dir().join(format!("{name}.json")))
    }

    /// Write through a temporary file that no other writer uses, so
    /// concurrent writes never install a half-written file.
    fn write_atomic(path: &Path, value: &impl Serialize) -> Result<(), StoreError> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension(format!(
            "json.{}-{}.tmp",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let written = fs::write(&tmp, serde_json::to_vec_pretty(value)?)
            .and_then(|()| fs::rename(&tmp, path));
        if written.is_err() {
            fs::remove_file(&tmp).ok();
        }
        Ok(written?)
    }

    fn read<T: DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    /// Like [`Self::read`], but `None` if the file does not exist.
    fn read_opt<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, StoreError> {
        match Self::read(path) {
            Err(StoreError::Io(e)) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            r => r.map(Some),
        }
    }

    /// The last saved settings, or defaults if there are none yet.
    pub fn load_state(&self) -> Result<Settings, StoreError> {
        let state = Self::read_opt::<Settings>(&self.state_path())?;
        let oled = Self::read_opt(&self.oled_path())?;
        Ok(match (state, oled) {
            (Some(mut settings), Some(oled)) => {
                settings.oled = oled;
                settings
            }
            // Older versions kept the OLED mode in state.json. Move it out
            // before the next lighting save drops it from there.
            (Some(settings), None) => {
                self.save_oled(&settings.oled)?;
                settings
            }
            (None, Some(oled)) => Settings {
                oled,
                ..Default::default()
            },
            (None, None) => Settings::default(),
        })
    }

    pub fn save_lighting(&self, lighting: &Lighting) -> Result<(), StoreError> {
        #[derive(Serialize)]
        struct State<'a> {
            lighting: &'a Lighting,
        }
        Self::write_atomic(&self.state_path(), &State { lighting })
    }

    pub fn save_oled(&self, oled: &OledMode) -> Result<(), StoreError> {
        Self::write_atomic(&self.oled_path(), oled)
    }

    pub fn save_state(&self, settings: &Settings) -> Result<(), StoreError> {
        self.save_lighting(&settings.lighting)?;
        self.save_oled(&settings.oled)
    }

    pub fn list_profiles(&self) -> Result<Vec<String>, StoreError> {
        let mut names = Vec::new();
        let entries = match fs::read_dir(self.profiles_dir()) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(names),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json")
                && let Some(stem) = path.file_stem()
            {
                names.push(stem.to_string_lossy().into_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn load_profile(&self, name: &str) -> Result<Settings, StoreError> {
        Self::read_opt(&self.profile_path(name)?)?.ok_or_else(|| StoreError::NotFound(name.into()))
    }

    pub fn save_profile(&self, name: &str, settings: &Settings) -> Result<(), StoreError> {
        Self::write_atomic(&self.profile_path(name)?, settings)
    }

    pub fn delete_profile(&self, name: &str) -> Result<(), StoreError> {
        match fs::remove_file(self.profile_path(name)?) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(StoreError::NotFound(name.into())),
            r => Ok(r?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Effect, OledMode};

    #[test]
    fn profile_roundtrip() {
        let dir = std::env::temp_dir().join(format!("apexkb-test-{}", std::process::id()));
        let store = Store::new(dir.clone());
        let mut s = Settings::default();
        s.lighting.effect = Effect::Spectrum { period_ms: 1234 };
        s.oled = OledMode::Clock {
            format: "%H".into(),
        };
        store.save_profile("gaming", &s).unwrap();
        assert_eq!(store.list_profiles().unwrap(), vec!["gaming"]);
        let back = store.load_profile("gaming").unwrap();
        assert_eq!(back.lighting, s.lighting);
        assert!(matches!(
            store.save_profile("../evil", &s),
            Err(StoreError::BadName)
        ));
        store.delete_profile("gaming").unwrap();
        assert!(matches!(
            store.load_profile("gaming"),
            Err(StoreError::NotFound(_))
        ));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn legacy_dir_migration() {
        let base = std::env::temp_dir().join(format!("apexkb-legacy-{}", std::process::id()));
        let legacy = base.join("apexide");
        let dir = base.join("apexkb");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("state.json"), "{}").unwrap();

        migrate_legacy_dir(&dir);
        assert!(dir.join("state.json").exists());
        assert!(!legacy.exists());

        // An existing new dir is never overwritten.
        fs::create_dir_all(&legacy).unwrap();
        migrate_legacy_dir(&dir);
        assert!(legacy.exists());
        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn state_split_and_migration() {
        let dir = std::env::temp_dir().join(format!("apexkb-state-{}", std::process::id()));
        let store = Store::new(dir.clone());
        assert!(matches!(
            store.load_state().unwrap().oled,
            OledMode::Sysinfo
        ));

        // An OLED change alone, with no lighting saved yet, survives.
        store.save_oled(&OledMode::Blank).unwrap();
        assert!(matches!(store.load_state().unwrap().oled, OledMode::Blank));
        fs::remove_file(store.oled_path()).unwrap();

        // A state.json from before the split still has the OLED mode in it.
        let mut old = Settings::default();
        old.lighting.brightness = 40;
        old.oled = OledMode::Clock {
            format: "%H".into(),
        };
        fs::create_dir_all(&dir).unwrap();
        fs::write(store.state_path(), serde_json::to_vec(&old).unwrap()).unwrap();
        let loaded = store.load_state().unwrap();
        assert_eq!(loaded.lighting.brightness, 40);
        assert!(matches!(loaded.oled, OledMode::Clock { .. }));

        // Saving lighting alone keeps the migrated OLED mode.
        let mut l = loaded.lighting.clone();
        l.effect = Effect::Off;
        store.save_lighting(&l).unwrap();
        let back = store.load_state().unwrap();
        assert_eq!(back.lighting, l);
        assert!(matches!(back.oled, OledMode::Clock { .. }));
        let leftovers = fs::read_dir(&dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0);
        fs::remove_dir_all(dir).ok();
    }
}
