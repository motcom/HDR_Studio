use crate::renderer::{Lighting, PREVIEW_SCALE_OPTIONS, SHADOW_SAMPLE_OPTIONS};
use std::{fs, io, path::PathBuf};

pub(crate) fn app_data_dir() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("LOCALAPPDATA")?);
    root.is_absolute().then(|| root.join("HDR STUDIO"))
}

pub struct Settings {
    path: PathBuf,
    last_value: Lighting,
    pub error: Option<String>,
}

impl Settings {
    pub fn open() -> Option<Self> {
        let path = app_data_dir()?.join("settings.json");
        Some(Self::read(path))
    }

    fn read(path: PathBuf) -> Self {
        let (last_value, error) = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(value) => (normalize(value), None),
                Err(e) => (
                    Lighting::default(),
                    Some(format!("Could not read settings: {e}")),
                ),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Lighting::default(), None),
            Err(e) => (
                Lighting::default(),
                Some(format!("Could not read settings: {e}")),
            ),
        };
        Self {
            path,
            last_value,
            error,
        }
    }

    pub fn lighting(&self) -> Lighting {
        self.last_value
    }

    pub fn save(&mut self, lighting: Lighting) {
        if self.last_value == lighting {
            return;
        }
        self.last_value = lighting;
        self.error = self
            .write(lighting)
            .err()
            .map(|e| format!("Could not save settings: {e}"));
    }

    fn write(&self, lighting: Lighting) -> io::Result<()> {
        let temporary = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        let bytes = serde_json::to_vec_pretty(&lighting)?;
        if let Some(folder) = self.path.parent() {
            fs::create_dir_all(folder)?;
        }
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, &self.path)
    }
}

fn normalize(mut lighting: Lighting) -> Lighting {
    let defaults = Lighting::default();
    for (value, choices, fallback) in [
        (
            &mut lighting.shadow_samples,
            &SHADOW_SAMPLE_OPTIONS,
            defaults.shadow_samples,
        ),
        (
            &mut lighting.idle_shadow_samples,
            &SHADOW_SAMPLE_OPTIONS,
            defaults.idle_shadow_samples,
        ),
        (
            &mut lighting.preview_scale,
            &PREVIEW_SCALE_OPTIONS,
            defaults.preview_scale,
        ),
        (
            &mut lighting.idle_preview_scale,
            &PREVIEW_SCALE_OPTIONS,
            defaults.idle_preview_scale,
        ),
    ] {
        if !choices.contains(value) {
            *value = fallback;
        }
    }
    lighting.idle_shadow_samples = lighting.idle_shadow_samples.max(lighting.shadow_samples);
    lighting.idle_preview_scale = lighting.idle_preview_scale.max(lighting.preview_scale);
    lighting.roughness = lighting.roughness.clamp(0.0, 1.0);
    lighting.metallic = lighting.metallic.clamp(0.0, 1.0);
    lighting
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_changes_and_replaces_the_previous_settings_file() {
        let path = PathBuf::from("tests/output")
            .join(format!("preview-settings-{}.json", std::process::id()));
        let mut settings = Settings::read(path.clone());
        let mut lighting = Lighting {
            preview_scale: 25,
            idle_preview_scale: 75,
            shadow_samples: 8,
            idle_shadow_samples: 16,
            floor_shadows: true,
            ..Lighting::default()
        };
        settings.save(lighting);
        assert!(settings.error.is_none());
        assert!(Settings::read(path.clone()).lighting() == lighting);
        lighting.preview_scale = 50;
        settings.save(lighting);
        assert!(settings.error.is_none());
        assert!(Settings::read(path.clone()).lighting() == lighting);
        fs::remove_file(path).unwrap();
    }
}
