use crate::environment::{Document, HdrImage, IMAGE_PREVIEW_WIDTH};
use eframe::egui;
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const SYNC_INTERVAL: Duration = Duration::from_millis(120);
const REPLACE_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const PREVIEW_NAME: &str = "hdr_studio_vray.hdr";
const TARGET_NAME: &str = "target.json";

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum LivePreviewSize {
    Small = 350,
    Medium = 512,
    #[default]
    Standard = IMAGE_PREVIEW_WIDTH as isize,
}

impl LivePreviewSize {
    fn dimensions(self) -> (u32, u32) {
        let width = self as u32;
        (width, width / 2)
    }

    fn label(self) -> String {
        let (width, height) = self.dimensions();
        format!("{width} × {height}")
    }
}

#[derive(Debug)]
enum PublishError {
    ReplaceBusy(std::io::Error),
    Failed(String),
}

impl fmt::Display for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReplaceBusy(error) => write!(f, "{error}"),
            Self::Failed(error) => f.write_str(error),
        }
    }
}

#[derive(Clone)]
enum Task {
    Live {
        announce: bool,
        size: LivePreviewSize,
    },
    Render {
        path: PathBuf,
        width: u32,
    },
}

struct Worker {
    task: Task,
    result: mpsc::Receiver<Result<(), PublishError>>,
}

#[derive(Default)]
pub struct VrayLink {
    live_size: LivePreviewSize,
    path: Option<PathBuf>,
    lock: Option<Arc<File>>,
    observed: Option<Document>,
    next_write: Option<Instant>,
    pending: bool,
    worker: Option<Worker>,
    render: Option<Task>,
    rendered: Option<PathBuf>,
    announce_live: bool,
    saved: u64,
    status: Option<String>,
}

impl VrayLink {
    fn start_in(directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("publisher.lock"))?;
        lock.try_lock()?;
        Ok(Self {
            path: Some(directory.join(PREVIEW_NAME)),
            lock: Some(Arc::new(lock)),
            announce_live: true,
            ..Self::default()
        })
    }

    pub fn is_rendering(&self) -> bool {
        self.render.is_some()
            || self
                .worker
                .as_ref()
                .is_some_and(|worker| matches!(worker.task, Task::Render { .. }))
    }

    pub fn controls(&mut self, ui: &mut egui::Ui, size: egui::Vec2, export_width: u32, busy: bool) {
        ui.label("Live preview size");
        let mut live_size = self.live_size;
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = size.y;
            egui::ComboBox::from_id_salt("vray-live-preview-size")
                .width(size.x)
                .selected_text(live_size.label())
                .show_ui(ui, |ui| {
                    for option in [
                        LivePreviewSize::Small,
                        LivePreviewSize::Medium,
                        LivePreviewSize::Standard,
                    ] {
                        ui.selectable_value(&mut live_size, option, option.label());
                    }
                });
        });
        self.set_live_size(live_size);
        if self.path.is_some() {
            let rendering = self.is_rendering();
            if ui
                .add_enabled(
                    self.worker.is_none() && !rendering,
                    egui::Button::new("Stop V-Ray sync").min_size(size),
                )
                .clicked()
            {
                *self = Self {
                    live_size,
                    ..Self::default()
                };
            }
        } else if ui
            .add(egui::Button::new("Link Maya V-Ray").min_size(size))
            .clicked()
        {
            match crate::settings::app_data_dir()
                .ok_or_else(|| io::Error::other("Windows Local App Data is unavailable"))
                .and_then(|directory| Self::start_in(&directory.join("vray-sync")))
            {
                Ok(link) => *self = Self { live_size, ..link },
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.status =
                        Some("Another HDR STUDIO window is already linked to Maya.".into());
                }
                Err(error) => self.status = Some(format!("Could not start V-Ray sync: {error}")),
            }
        }
        let linked = self.path.is_some();
        let rendering = self.is_rendering();
        let manual = if self.rendered.is_some() {
            "Resume Live Sync"
        } else {
            "Sync now"
        };
        if ui
            .add_enabled(
                linked && !rendering,
                egui::Button::new(manual).min_size(size),
            )
            .clicked()
        {
            self.announce_live |= self.rendered.take().is_some();
            self.pending = true;
            self.next_write = None;
        }
        if ui
            .add_enabled(
                linked && !busy && !rendering && self.saved > 0,
                egui::Button::new("Render HDR for Maya…").min_size(size),
            )
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Radiance HDR", &["hdr"])
                .set_file_name("lighting_vray.hdr")
                .save_file()
        {
            match self.render_path(path) {
                Ok(path) => {
                    self.render = Some(Task::Render {
                        path,
                        width: export_width,
                    })
                }
                Err(error) => self.status = Some(error),
            }
        }
        if let Some(path) = &self.rendered {
            ui.small(format!("Rendered HDR · {}", path.display()));
        }
        if let Some(status) = &self.status {
            ui.small(status);
        }
    }

    fn set_live_size(&mut self, size: LivePreviewSize) {
        if self.live_size != size {
            self.live_size = size;
            self.pending = true;
        }
    }

    fn render_path(&self, path: PathBuf) -> Result<PathBuf, String> {
        let mut path = std::path::absolute(path).map_err(|error| error.to_string())?;
        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("hdr"))
        {
            let mut name = path
                .file_name()
                .ok_or("Choose an HDR filename")?
                .to_os_string();
            name.push(".hdr");
            path.set_file_name(name);
            if path.exists()
                && rfd::MessageDialog::new()
                    .set_title("Replace rendered HDR?")
                    .set_description(path.display().to_string())
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show()
                    != rfd::MessageDialogResult::Yes
            {
                return Err("HDR render canceled".into());
            }
        }
        let preview_directory = self
            .path
            .as_ref()
            .and_then(|path| path.parent())
            .ok_or("Start V-Ray sync before rendering for Maya")?;
        let parent = path.parent().ok_or("Choose an HDR output directory")?;
        if fs::canonicalize(parent).map_err(|error| error.to_string())?
            == fs::canonicalize(preview_directory).map_err(|error| error.to_string())?
        {
            return Err("Choose an output location outside the V-Ray sync cache.".into());
        }
        Ok(path)
    }

    pub fn update(&mut self, ctx: &egui::Context, document: &Document, importing: bool) {
        let Some(path) = &self.path else {
            return;
        };
        let now = Instant::now();
        if self.observed.as_ref() != Some(document) {
            self.observed = Some(document.clone());
            self.pending = true;
        }
        if let Some(worker) = &self.worker {
            self.status = Some(match (&worker.task, worker.result.try_recv()) {
                (Task::Live { .. }, Ok(Ok(()))) => {
                    self.announce_live = false;
                    self.saved += 1;
                    format!("V-Ray HDR saved ({})", self.saved)
                }
                (Task::Live { .. }, Ok(Err(PublishError::ReplaceBusy(error)))) => {
                    self.pending = true;
                    self.next_write = Some(now + REPLACE_RETRY_INTERVAL);
                    format!("Retrying V-Ray HDR save: {error}")
                }
                (Task::Render { path, .. }, Ok(Ok(()))) => {
                    self.rendered = Some(path.clone());
                    self.announce_live = false;
                    "Rendered HDR ready for Maya".into()
                }
                (_, Ok(Err(error))) => format!("V-Ray HDR update failed: {error}"),
                (_, Err(mpsc::TryRecvError::Disconnected)) => "V-Ray sync worker stopped".into(),
                (_, Err(mpsc::TryRecvError::Empty)) => return,
            });
            self.worker = None;
        }
        if importing {
            return;
        }
        let task = if let Some(render) = self.render.take() {
            render
        } else {
            if self.rendered.is_some() || !self.pending {
                return;
            }
            if let Some(deadline) = self.next_write
                && deadline > now
            {
                ctx.request_repaint_after(deadline - now);
                return;
            }
            self.pending = false;
            self.next_write = Some(now + SYNC_INTERVAL);
            Task::Live {
                announce: self.announce_live,
                size: self.live_size,
            }
        };

        let snapshot = document.clone();
        let path = path.clone();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.worker = Some(Worker {
            task: task.clone(),
            result: rx,
        });
        self.status = Some(
            match task {
                Task::Live { .. } => "Updating V-Ray HDR…",
                Task::Render { .. } => "Rendering HDR for Maya…",
            }
            .into(),
        );
        let lock = self.lock.clone();
        std::thread::spawn(move || {
            let result = match task {
                Task::Live { announce, size } => {
                    let (width, height) = size.dimensions();
                    publish(snapshot.compose_preview(width, height), &path).and_then(|()| {
                        if announce {
                            announce_target(&path, None)
                        } else {
                            Ok(())
                        }
                    })
                }
                Task::Render {
                    path: output,
                    width,
                } => publish(snapshot.compose(width, width / 2), &output).and_then(|()| {
                    announce_target(&path, Some(&output)).map_err(|error| {
                        PublishError::Failed(format!(
                            "Saved {}, but could not notify Maya: {error}",
                            output.display()
                        ))
                    })
                }),
            };
            drop(lock);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
}

fn publish(mut image: HdrImage, path: &Path) -> Result<(), PublishError> {
    // V-Ray's spherical placement centers -X where HDR STUDIO centers +X.
    let width = image.width as usize;
    for row in image.pixels.chunks_exact_mut(width) {
        row.rotate_left(width / 2);
    }
    write_atomic(path, |open| {
        image.write_hdr(open).map_err(PublishError::Failed)
    })
}

fn announce_target(preview: &Path, rendered: Option<&Path>) -> Result<(), PublishError> {
    #[derive(serde::Serialize)]
    struct Target<'a> {
        rendered_hdr: Option<&'a Path>,
    }

    write_atomic(&preview.with_file_name(TARGET_NAME), |open| {
        let file = open().map_err(|error| PublishError::Failed(error.to_string()))?;
        serde_json::to_writer(
            file,
            &Target {
                rendered_hdr: rendered,
            },
        )
        .map_err(|error| PublishError::Failed(error.to_string()))
    })
}

fn write_atomic(
    path: &Path,
    write: impl FnOnce(&mut dyn FnMut() -> io::Result<File>) -> Result<(), PublishError>,
) -> Result<(), PublishError> {
    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
    let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary =
        path.with_file_name(format!(".hdr-studio-{}-{sequence}.tmp", std::process::id()));
    let mut created = false;
    let result = write(&mut || {
        let file = File::create_new(&temporary)?;
        created = true;
        Ok(file)
    })
    .and_then(|()| replace(&temporary, path));
    if result.is_err() && created {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn replace(temporary: &Path, path: &Path) -> Result<(), PublishError> {
    let mut retries = 3;
    loop {
        match fs::rename(temporary, path) {
            Ok(()) => return Ok(()),
            // Maya may briefly have the previous file open on Windows.
            Err(error) if cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32 | 33)) => {
                if retries == 0 {
                    return Err(PublishError::ReplaceBusy(error));
                }
                retries -= 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(PublishError::Failed(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announces_rendered_hdr_only_after_saving_and_resumes_live_sync() {
        let root = Path::new("tests/output").join(format!("vray-target-{}", std::process::id()));
        let cache = root.join("vray-sync");
        let mut link = VrayLink::start_in(&cache).unwrap();
        assert!(
            matches!(VrayLink::start_in(&cache), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
        );
        let source = cache.join(PREVIEW_NAME);
        let target = cache.join(TARGET_NAME);
        let read_target =
            || serde_json::from_slice::<serde_json::Value>(&fs::read(&target).unwrap()).unwrap();
        let ctx = egui::Context::default();
        let mut document = Document::default();
        let finish = |link: &mut VrayLink, document: &Document| {
            let deadline = Instant::now() + Duration::from_secs(10);
            link.update(&ctx, document, false);
            while link.worker.is_some() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
                link.update(&ctx, document, false);
            }
            assert!(link.worker.is_none());
        };
        finish(&mut link, &document);
        assert_eq!(HdrImage::load(&source).unwrap().width, 700);
        assert!(read_target()["rendered_hdr"].is_null());

        let output = std::path::absolute(root.join("Final 照明.hdr")).unwrap();
        assert!(link.render_path(source.clone()).is_err());
        link.render = Some(Task::Render {
            path: output.clone(),
            width: 512,
        });
        finish(&mut link, &document);
        assert_eq!(HdrImage::load(&output).unwrap().width, 512);
        assert_eq!(read_target()["rendered_hdr"].as_str(), output.to_str());
        assert_eq!(link.rendered.as_ref(), Some(&output));

        link.render = Some(Task::Render {
            path: root.join("missing").join("failed.hdr"),
            width: 512,
        });
        finish(&mut link, &document);
        assert_eq!(read_target()["rendered_hdr"].as_str(), output.to_str());
        document.add_light();
        link.set_live_size(LivePreviewSize::Medium);
        finish(&mut link, &document);
        assert_eq!(link.saved, 1);
        assert_eq!(HdrImage::load(&source).unwrap().width, 700);
        assert_eq!(read_target()["rendered_hdr"].as_str(), output.to_str());

        link.announce_live = link.rendered.take().is_some();
        link.pending = true;
        link.next_write = None;
        finish(&mut link, &document);
        assert!(read_target()["rendered_hdr"].is_null());
        assert_eq!(link.saved, 2);
        let live = HdrImage::load(&source).unwrap();
        assert_eq!((live.width, live.height), (512, 256));

        drop(link);
        for path in [source, target, output, cache.join("publisher.lock")] {
            fs::remove_file(path).unwrap();
        }
        fs::remove_dir(cache).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn writes_the_latest_light_edit_at_the_new_live_size() {
        let directory = Path::new("tests/output");
        fs::create_dir_all(directory).unwrap();
        let path = directory.join(format!("vray-preview-{}.hdr", std::process::id()));
        let ctx = egui::Context::default();
        let mut document = Document::default();
        let (previous_write, worker) = mpsc::channel();
        let mut link = VrayLink {
            path: Some(path.clone()),
            observed: Some(document.clone()),
            worker: Some(Worker {
                task: Task::Live {
                    announce: false,
                    size: LivePreviewSize::Standard,
                },
                result: worker,
            }),
            ..VrayLink::default()
        };
        let light = document.add_light();
        document.lights[light].intensity = 0.0;
        link.update(&ctx, &document, false);
        document.lights[light].u = 0.2;
        document.lights[light].width_degrees = 60.0;
        document.lights[light].intensity = 4.0;
        link.update(&ctx, &document, false);
        link.set_live_size(LivePreviewSize::Small);
        assert!(link.pending);

        previous_write.send(Ok(())).unwrap();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::PointerButton {
                    pos: egui::pos2(100.0, 100.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                assert!(ctx.input(|input| input.pointer.primary_down()));
                link.update(ctx, &document, false);
            },
        );
        assert!(!link.pending);
        link.worker
            .take()
            .unwrap()
            .result
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let image = HdrImage::load(&path).unwrap();
        assert_eq!((image.width, image.height), (350, 175));
        assert!(image.pixels.iter().any(|pixel| pixel[0] > 1.0));
        fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn retries_a_locked_hdr_after_the_reader_closes_it() {
        use std::os::windows::fs::OpenOptionsExt;

        let directory = Path::new("tests/output");
        fs::create_dir_all(directory).unwrap();
        let path = directory.join(format!("vray-locked-{}.hdr", std::process::id()));
        fs::write(&path, b"previous").unwrap();
        let reader = fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ, without FILE_SHARE_DELETE.
            .open(&path)
            .unwrap();
        let mut link = VrayLink {
            path: Some(path.clone()),
            ..VrayLink::default()
        };
        let ctx = egui::Context::default();
        let document = Document::default();
        let deadline = Instant::now() + Duration::from_secs(10);
        link.update(&ctx, &document, false);
        while link.worker.is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            link.update(&ctx, &document, false);
        }
        assert!(link.pending);
        assert_eq!(fs::read(&path).unwrap(), b"previous");
        drop(reader);

        while link.saved == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            link.update(&ctx, &document, false);
        }
        assert_eq!(link.saved, 1);
        assert_eq!(HdrImage::load(&path).unwrap().width, 700);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn replaces_the_linked_hdr_with_the_vray_panorama() {
        let directory = Path::new("tests/output");
        fs::create_dir_all(directory).unwrap();
        let path = directory.join(format!("vray-publish-{}.hdr", std::process::id()));
        let make_image = |red: f32| HdrImage {
            width: 4,
            height: 2,
            pixels: [red, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]
                .map(|value| [value, 0.0, 0.0])
                .to_vec(),
        };
        publish(make_image(1.0), &path).unwrap();
        publish(make_image(9.0), &path).unwrap();
        assert!(publish(make_image(f32::NAN), &path).is_err());
        let pixels = HdrImage::load(&path).unwrap().pixels;
        assert_eq!(
            pixels.iter().map(|pixel| pixel[0]).collect::<Vec<_>>(),
            [3.0, 4.0, 9.0, 2.0, 7.0, 8.0, 5.0, 6.0],
        );
        fs::remove_file(path).unwrap();
    }
}
