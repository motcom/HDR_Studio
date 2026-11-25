use crate::{
    camera::Camera,
    environment::{Document, FalloffCurve, HdrImage, ImageSource},
    light_paint::LightPaint,
    renderer::Lighting,
    scene::{self, Scene},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Take, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

// Version 1: magic, JSON length, Alembic length, JSON, RGB32F images, Ogawa ABC.
const MAGIC: &[u8; 8] = b"HDRLMO\x01\0";
const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct State {
    pub document: Document,
    pub lighting: Lighting,
    pub export_width: u32,
    pub show_preview: bool,
    pub light_paint: LightPaint,
    #[serde(default)]
    pub retained_cameras: Vec<Camera>,
    #[serde(default)]
    pub retained_camera: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            document: Document::default(),
            lighting: Lighting::default(),
            export_width: crate::hdr_editor::DEFAULT_EXPORT_WIDTH,
            show_preview: true,
            light_paint: LightPaint::default(),
            retained_cameras: Vec::new(),
            retained_camera: 0,
        }
    }
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct ModelState {
    pub name: PathBuf,
    pub seconds: f64,
    pub selected_camera: usize,
    pub cameras: Vec<Camera>,
    #[serde(default)]
    pub removed_camera_paths: Vec<String>,
}

#[derive(Clone, PartialEq)]
pub struct Snapshot {
    pub state: State,
    pub model: Option<(Arc<Archive>, ModelState)>,
}

#[derive(Deserialize, Serialize)]
struct Manifest {
    state: State,
    model: Option<ModelState>,
}

pub struct LoadedModel {
    pub scene: Scene,
    pub archive: Arc<Archive>,
    pub seconds: f64,
    pub selected_camera: usize,
    pub removed_camera_paths: Vec<String>,
}

pub struct LoadedProject {
    pub state: State,
    pub model: Option<LoadedModel>,
}

#[derive(PartialEq)]
pub struct Archive {
    file: TemporaryFile,
    pub original: Option<PathBuf>,
    embedded: Option<ModelState>,
}

impl Archive {
    pub fn import(path: &Path, seconds: Option<f64>) -> Result<LoadedModel, String> {
        let seconds = match seconds {
            Some(seconds) => seconds,
            None => scene::default_sample(path)?,
        };
        let (file, original) = Self::copy_import(path).map_err(|e| e.to_string())?;
        let scene = scene::load_imported(path, &file.0, seconds, Some(&original))?;
        Ok(LoadedModel {
            scene,
            archive: Arc::new(Self {
                file,
                original: Some(path.to_owned()),
                embedded: None,
            }),
            seconds,
            selected_camera: 0,
            removed_camera_paths: Vec::new(),
        })
    }

    fn copy_import(path: &Path) -> io::Result<(TemporaryFile, fs::Metadata)> {
        let mut source = File::open(path)?;
        let original = source.metadata()?;
        let (temporary, mut copy) = TemporaryFile::create(&std::env::temp_dir(), "abc")?;
        let copied = io::copy(&mut source, &mut copy)?;
        if copied != original.len() || source.metadata()?.modified()? != original.modified()? {
            return Err(invalid("The Alembic file changed during import"));
        }
        Ok((temporary, original))
    }

    pub fn reload(self: &Arc<Self>, seconds: f64) -> Result<LoadedModel, String> {
        if let Some(path) = &self.original {
            return Self::import(path, Some(seconds));
        }
        let model = self
            .embedded
            .as_ref()
            .ok_or("Missing embedded model information")?;
        let cameras = if seconds == model.seconds {
            model.cameras.clone()
        } else {
            Vec::new()
        };
        let scene = scene::load_embedded(&model.name, &self.file.0, seconds, cameras)?;
        Ok(LoadedModel {
            scene,
            archive: self.clone(),
            seconds,
            selected_camera: 0,
            removed_camera_paths: Vec::new(),
        })
    }
}

pub fn save(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    let manifest = Manifest {
        state: snapshot.state.clone(),
        model: snapshot.model.as_ref().map(|(_, model)| model.clone()),
    };
    let metadata = serde_json::to_vec(&manifest)?;
    if metadata.len() as u64 > MAX_METADATA_BYTES {
        return Err(invalid("Project metadata is too large"));
    }
    let mut abc = snapshot
        .model
        .as_ref()
        .map(|(archive, _)| File::open(&archive.file.0))
        .transpose()?;
    let abc_len = match &abc {
        Some(abc) => abc.metadata()?.len(),
        None => 0,
    };
    let directory = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let (temporary, file) = TemporaryFile::create(directory, "tmp")?;
    let mut writer = BufWriter::new(file);
    writer.write_all(MAGIC)?;
    writer.write_all(&(metadata.len() as u64).to_le_bytes())?;
    writer.write_all(&abc_len.to_le_bytes())?;
    writer.write_all(&metadata)?;
    write_images(&mut writer, &snapshot.state.document)?;
    if let Some(abc) = &mut abc
        && io::copy(abc, &mut writer)? != abc_len
    {
        return Err(invalid("The embedded Alembic file has changed"));
    }
    writer.flush()?;
    writer.get_ref().sync_all()?;
    drop(writer);
    fs::rename(&temporary.0, path)
}

pub fn load(path: &Path) -> Result<LoadedProject, String> {
    let (manifest, file) = read(path).map_err(|e| e.to_string())?;
    let model = if let Some((model, file)) = manifest.model.zip(file) {
        let scene =
            scene::load_embedded(&model.name, &file.0, model.seconds, model.cameras.clone())?;
        let seconds = model.seconds;
        let selected_camera = model.selected_camera;
        let removed_camera_paths = model.removed_camera_paths.clone();
        Some(LoadedModel {
            scene,
            archive: Arc::new(Archive {
                file,
                original: None,
                embedded: Some(model),
            }),
            seconds,
            selected_camera,
            removed_camera_paths,
        })
    } else {
        None
    };
    Ok(LoadedProject {
        state: manifest.state,
        model,
    })
}

fn read(path: &Path) -> io::Result<(Manifest, Option<TemporaryFile>)> {
    let file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut reader = BufReader::new(file);
    let mut magic = [0; 8];
    reader.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(invalid("Unsupported HDR STUDIO project format or version"));
    }
    let metadata_len = u64::from_le_bytes(read_bytes(&mut reader)?);
    let abc_len = u64::from_le_bytes(read_bytes(&mut reader)?);
    let image_len = file_len
        .checked_sub(24)
        .and_then(|bytes| bytes.checked_sub(metadata_len))
        .and_then(|bytes| bytes.checked_sub(abc_len))
        .filter(|_| metadata_len <= MAX_METADATA_BYTES)
        .ok_or_else(|| invalid("Invalid LMO section sizes"))?;
    let mut metadata = vec![0; metadata_len as usize];
    reader.read_exact(&mut metadata)?;
    let mut manifest: Manifest = serde_json::from_slice(&metadata)?;
    if manifest.model.is_some() != (abc_len > 0)
        || (manifest.model.is_some() && !manifest.state.retained_cameras.is_empty())
        || !crate::hdr_editor::EXPORT_WIDTHS.contains(&manifest.state.export_width)
    {
        return Err(invalid("Invalid LMO project settings"));
    }
    validate_cameras(
        &manifest.state.retained_cameras,
        manifest.state.retained_camera,
    )?;
    if let Some(model) = &manifest.model {
        validate_model(model)?;
    }
    let mut images = (&mut reader).take(image_len);
    read_images(&mut images, &mut manifest.state.document)?;
    if images.limit() != 0 {
        return Err(invalid("Unexpected trailing HDR data in LMO"));
    }
    let archive = if abc_len != 0 {
        let (temporary, mut file) = TemporaryFile::create(&std::env::temp_dir(), "abc")?;
        if io::copy(&mut reader.take(abc_len), &mut file)? != abc_len {
            return Err(invalid("Truncated Alembic data in LMO"));
        }
        Some(temporary)
    } else {
        None
    };
    Ok((manifest, archive))
}

fn validate_model(model: &ModelState) -> io::Result<()> {
    if model.name.file_name() != Some(model.name.as_os_str()) || !model.seconds.is_finite() {
        return Err(invalid("Invalid model data in LMO"));
    }
    validate_cameras(&model.cameras, model.selected_camera)
}

fn validate_cameras(cameras: &[Camera], selected: usize) -> io::Result<()> {
    if selected >= cameras.len().max(1)
        || cameras.iter().any(|camera| {
            !camera.view.is_finite()
                || !camera.projection.is_finite()
                || !camera.aspect.is_finite()
                || camera.aspect <= 0.0
                || !camera.focal.is_finite()
        })
    {
        return Err(invalid("Invalid saved camera information"));
    }
    Ok(())
}

fn write_images(writer: &mut impl Write, document: &Document) -> io::Result<()> {
    for light in &document.lights {
        if let Some(source) = &light.image_layer.image {
            let image = &source.original;
            writer.write_all(&image.width.to_le_bytes())?;
            writer.write_all(&image.height.to_le_bytes())?;
            writer.write_all(bytemuck::cast_slice(&image.pixels))?;
        } else {
            writer.write_all(&[0; 8])?;
        }
        if let Some(child) = &light.composite {
            write_images(writer, child)?;
        }
    }
    Ok(())
}

fn read_images(reader: &mut Take<impl Read>, document: &mut Document) -> io::Result<()> {
    if !document.validate_project_ids() {
        return Err(invalid("Invalid light IDs in LMO"));
    }
    for light in &mut document.lights {
        for curve in [
            &light.falloff,
            &light.gradient_curve,
            &light.gradient_background.value,
            &light.gradient_background.alpha,
        ]
        .into_iter()
        .chain(&light.box_vertical)
        {
            validate_curve(curve)?;
        }
        let stops = &light.gradient_background.colors;
        if !(2..=crate::environment::MAX_COLOR_STOPS).contains(&stops.len())
            || stops[0].position != 0.0
            || stops[stops.len() - 1].position != 1.0
            || stops.windows(2).any(|pair| {
                pair[0].position.partial_cmp(&pair[1].position) != Some(std::cmp::Ordering::Less)
            })
            || stops
                .iter()
                .flat_map(|stop| stop.color)
                .any(|channel| !(0.0..=1.0).contains(&channel))
        {
            return Err(invalid("Invalid gradient background color ramp in LMO"));
        }
        let width = u32::from_le_bytes(read_bytes(reader)?);
        let height = u32::from_le_bytes(read_bytes(reader)?);
        if (width, height) != (0, 0) {
            let pixels = u64::from(width) * u64::from(height);
            if width == 0 || height.checked_mul(2) != Some(width) || pixels > reader.limit() / 12 {
                return Err(invalid("Invalid HDR image size in LMO"));
            }
            let count = usize::try_from(pixels).map_err(io::Error::other)?;
            let mut pixels = Vec::new();
            pixels.try_reserve_exact(count).map_err(io::Error::other)?;
            pixels.resize(count, [0.0; 3]);
            reader.read_exact(bytemuck::cast_slice_mut(&mut pixels))?;
            if pixels
                .iter()
                .flatten()
                .any(|&value| !(0.0..=1.0e20).contains(&value))
            {
                return Err(invalid("Invalid HDR radiance in LMO"));
            }
            light.image_layer.image = Some(ImageSource::shared(HdrImage {
                width,
                height,
                pixels,
            }));
        }
        if let Some(child) = &mut light.composite {
            read_images(reader, child)?;
        }
    }
    Ok(())
}

fn validate_curve(curve: &FalloffCurve) -> io::Result<()> {
    if curve.points.len() < 2
        || !curve.points.iter().flatten().all(|value| value.is_finite())
        || curve.points.windows(2).any(|pair| pair[0][0] >= pair[1][0])
    {
        return Err(invalid("Invalid light falloff curve in LMO"));
    }
    Ok(())
}

fn read_bytes<const N: usize>(reader: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(PartialEq)]
struct TemporaryFile(PathBuf);

impl TemporaryFile {
    fn create(directory: &Path, extension: &str) -> io::Result<(Self, File)> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(
                ".hdr-studio-project-{}-{id}.{extension}",
                std::process::id()
            ));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((Self(path), file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::{BlendMode, LightShape};

    #[test]
    fn restores_editable_lighting_geometry_and_saved_camera_without_source_files() {
        let fixture = Path::new("tests/fixtures/basic.abc");
        let directory = Path::new("tests/output");
        fs::create_dir_all(directory).unwrap();
        let (source, mut source_file) = TemporaryFile::create(directory, "abc").unwrap();
        let abc_bytes = fs::read(fixture).unwrap();
        source_file.write_all(&abc_bytes).unwrap();
        drop(source_file);
        let imported = Archive::import(&source.0, Some(0.0)).unwrap();

        let mut state = State {
            export_width: 4096,
            light_paint: LightPaint::Diffuse,
            ..State::default()
        };
        state.lighting.default_light = false;
        state.lighting.roughness = 0.37;
        state.lighting.canvas_exposure_ev = -2.0;
        state.lighting.render_preview_exposure_ev = 1.5;
        let mut children = Document::default();
        let image = children.add_light();
        children.lights[image].shape = LightShape::Image;
        children.lights[image].image_layer.source = "offline.exr".into();
        children.lights[image].image_layer.image = Some(ImageSource::shared(HdrImage {
            width: 2,
            height: 1,
            pixels: vec![[7.125, 0.25, 0.0], [0.5, 1.5, 3.0]],
        }));
        let group = state.document.add_light();
        state.document.lights[group].name = "Studio".into();
        state.document.lights[group].shape = LightShape::CompositeLight;
        state.document.lights[group].blend = BlendMode::Over;
        state.document.lights[group].composite = Some(Box::new(children));
        let background = state.document.add_light();
        let gradient = &mut state.document.lights[background];
        gradient.shape = LightShape::GradientBackground;
        gradient.gradient_background.colors[0].color = [0.25, 0.5, 1.0];
        gradient.gradient_background.alpha =
            FalloffCurve::from_preset(crate::environment::FalloffPreset::Linear);
        let mut cameras = imported.scene.cameras.clone();
        cameras[0].projection.z_axis.x += 0.000_123_456_789_012_345;
        cameras[0].source = "Saved render gate".into();
        let snapshot = Snapshot {
            state,
            model: Some((
                imported.archive,
                ModelState {
                    name: "model.abc".into(),
                    seconds: 0.0,
                    selected_camera: 0,
                    cameras: cameras.clone(),
                    removed_camera_paths: vec![cameras[0].path.clone()],
                },
            )),
        };

        drop(source);
        let (file, handle) = TemporaryFile::create(directory, "lmo").unwrap();
        drop(handle);
        save(&file.0, &snapshot).unwrap();
        let loaded = load(&file.0).unwrap();
        let model = loaded.model.unwrap();
        assert_eq!(fs::read(&model.archive.file.0).unwrap(), abc_bytes);
        assert_eq!(model.scene.vertices.len(), imported.scene.vertices.len());
        assert_eq!(
            scene::bounds(&model.scene.vertices),
            scene::bounds(&imported.scene.vertices)
        );
        assert!(model.scene.cameras == cameras);
        assert_eq!(model.removed_camera_paths, [cameras[0].path.clone()]);
        assert_eq!(model.selected_camera, 0);
        assert_eq!(loaded.state.export_width, 4096);
        assert!(loaded.state.light_paint == LightPaint::Diffuse);
        assert!(loaded.state.lighting == snapshot.state.lighting);
        let composite = &loaded.state.document.lights[0];
        assert_eq!(composite.name, "Studio");
        assert_eq!(
            composite.composite.as_ref().unwrap().lights[0]
                .image_layer
                .source,
            "offline.exr"
        );
        assert_eq!(
            loaded.state.document.compose(24, 12).pixels,
            snapshot.state.document.compose(24, 12).pixels
        );
        assert!(model.archive.reload(0.0).unwrap().scene.cameras == cameras);

        let camera_only = Snapshot {
            state: State {
                retained_cameras: cameras,
                ..snapshot.state
            },
            model: None,
        };
        save(&file.0, &camera_only).unwrap();
        let loaded = load(&file.0).unwrap();
        assert!(loaded.model.is_none());
        assert!(loaded.state.retained_cameras == camera_only.state.retained_cameras);
        assert_eq!(
            loaded.state.document.compose(24, 12).pixels,
            camera_only.state.document.compose(24, 12).pixels
        );
    }

    #[test]
    fn saves_empty_projects_and_rejects_truncated_files() {
        let directory = Path::new("tests/output");
        fs::create_dir_all(directory).unwrap();
        let (file, handle) = TemporaryFile::create(directory, "lmo").unwrap();
        drop(handle);
        let snapshot = Snapshot {
            state: State::default(),
            model: None,
        };
        save(&file.0, &snapshot).unwrap();
        let loaded = load(&file.0).unwrap();
        assert!(loaded.model.is_none());
        assert!(loaded.state == snapshot.state);
        OpenOptions::new()
            .write(true)
            .open(&file.0)
            .unwrap()
            .set_len(25)
            .unwrap();
        assert!(load(&file.0).is_err());
    }
}
