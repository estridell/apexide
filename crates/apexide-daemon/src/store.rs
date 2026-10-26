//! On-disk persistence: current settings and named profiles, as JSON under
//! `$XDG_CONFIG_HOME/apexide`.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::state::Settings;

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
            .join("apexide")
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
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

    fn write_atomic(path: &Path, settings: &Settings) -> Result<(), StoreError> {
        fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    fn read(path: &Path) -> Result<Settings, StoreError> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    /// The last saved settings, or defaults if there are none yet.
    pub fn load_state(&self) -> Result<Settings, StoreError> {
        match Self::read(&self.state_path()) {
            Err(StoreError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {
                Ok(Settings::default())
            }
            r => r,
        }
    }

    pub fn save_state(&self, settings: &Settings) -> Result<(), StoreError> {
        Self::write_atomic(&self.state_path(), settings)
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
        match Self::read(&self.profile_path(name)?) {
            Err(StoreError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {
                Err(StoreError::NotFound(name.into()))
            }
            r => r,
        }
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
        let dir = std::env::temp_dir().join(format!("apexide-test-{}", std::process::id()));
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
}
