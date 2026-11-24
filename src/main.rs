mod abc;
mod app;
mod camera;
mod environment;
mod hdr_editor;
mod light_paint;
mod preview;
mod project;
mod renderer;
mod scene;
mod settings;
mod vray_link;

#[cfg(test)]
mod tests;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(std::ffi::OsStr::new("--inspect")) {
        let path = std::path::PathBuf::from(
            args.next()
                .ok_or("Usage: HDR_STUDIO --inspect file.abc [seconds]")?,
        );
        let seconds = args
            .next()
            .map(|s| s.to_string_lossy().parse::<f64>())
            .transpose()?
            .unwrap_or(scene::default_sample(&path)?);
        let scene = scene::load(&path, seconds)?;
        let (min, max) = scene::bounds(&scene.vertices);
        println!(
            "{} nodes, {} visible meshes, {} triangles, {} cameras",
            scene.nodes.len(),
            scene.meshes,
            scene.vertices.len() / 3,
            scene.cameras.len()
        );
        println!("Bounds: {min:?} .. {max:?}");
        for c in scene.cameras {
            println!(
                "Camera: {} | {:.3} mm | aspect {:.6} | {}",
                c.path, c.focal, c.aspect, c.source
            );
        }
        for warning in scene.warnings {
            println!("Note: {warning}");
        }
        return Ok(());
    }
    let initial = first.map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 500.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "HDR STUDIO — NewProject",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, initial)?))),
    )?;
    Ok(())
}
