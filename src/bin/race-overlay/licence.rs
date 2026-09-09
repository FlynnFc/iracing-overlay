//! Temporary first-run setup. Any non-empty key is accepted by explicit product
//! decision while Polar is being configured. This is NOT a payment/license gate.
//! The marker stores no key and must never become a trusted Polar entitlement.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use egui_overlay::{EguiOverlay, egui_render_three_d::ThreeDBackend, egui_window_glfw_passthrough::GlfwBackend};
use serde::Deserialize;

use crate::ui::licence::{Action, LicensePage};

const MARKER: &str = "version = 1\nsetup_completed = true\n";

#[derive(Debug, Deserialize)]
struct SetupMarker {
    version: u8,
    setup_completed: bool,
}

fn setup_complete(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str::<SetupMarker>(&text).ok())
        .is_some_and(|marker| marker.version == 1 && marker.setup_completed)
}

/// Replace this adapter with Polar validation; do not reuse the mock marker.
fn accept_temporary_key(key: &str, path: &Path) -> std::io::Result<bool> {
    if key.trim().is_empty() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, MARKER)?;
    Ok(true)
}

/// Runs before any overlay runtime exists. Closing the page cancels startup.
/// Preview ignores saved completion and never writes or starts the overlay.
pub fn show_first_start(preview: bool, screenshot: Option<PathBuf>) -> bool {
    let path = crate::config::config_path().with_file_name("license-setup.toml");
    if !preview && setup_complete(&path) {
        return true;
    }
    let accepted = Arc::new(AtomicBool::new(false));
    egui_overlay::start(FirstStart {
        page: LicensePage::default(),
        accepted: Arc::clone(&accepted),
        path,
        preview,
        screenshot,
        frames: 0,
    });
    accepted.load(Ordering::Relaxed)
}

struct FirstStart {
    page: LicensePage,
    accepted: Arc<AtomicBool>,
    path: PathBuf,
    preview: bool,
    screenshot: Option<PathBuf>,
    frames: u32,
}

impl EguiOverlay for FirstStart {
    fn gui_run(
        &mut self,
        egui_context: &egui::Context,
        _default_gfx_backend: &mut ThreeDBackend,
        glfw_backend: &mut GlfwBackend,
    ) {
        match self.page.draw(egui_context) {
            Action::None => {}
            Action::Quit => glfw_backend.window.set_should_close(true),
            Action::Continue if self.preview => self.page.completed = true,
            Action::Continue => match accept_temporary_key(&self.page.key, &self.path) {
                Ok(true) => {
                    self.page.key.clear();
                    self.accepted.store(true, Ordering::Relaxed);
                    glfw_backend.window.set_should_close(true);
                }
                Ok(false) => self.page.error = Some("Enter your license key to continue.".into()),
                Err(_) => {
                    self.page.error = Some("Couldn't save your setup. Check folder access and try again.".into());
                }
            },
        }
    }

    fn run(
        &mut self,
        egui_context: &egui::Context,
        default_gfx_backend: &mut ThreeDBackend,
        glfw_backend: &mut GlfwBackend,
    ) -> Option<(egui::PlatformOutput, Duration)> {
        if self.frames == 0 {
            crate::app::install_fonts(egui_context);
            glfw_backend.window.set_title("Race Overlay — Activate");
            // Unlike the in-race overlay, this is a normal focusable window:
            // typing, clipboard shortcuts, Alt-Tab and the taskbar all work.
            glfw_backend.window.set_floating(false);
            glfw_backend.set_passthrough(false);
            glfw_backend.set_window_size([640.0, 680.0]);
            glfw_backend.glfw.with_primary_monitor(|_, monitor| {
                if let Some(monitor) = monitor {
                    let (x, y, width, height) = monitor.get_workarea();
                    glfw_backend.window.set_pos(x + (width - 640).max(0) / 2, y + (height - 680).max(0) / 2);
                }
            });
            if self.screenshot.is_some() {
                glfw_backend.window.hide();
            } else {
                glfw_backend.window.focus();
            }
        }
        let mut input = glfw_backend.take_raw_input();
        if self.screenshot.is_some() {
            input.events.clear();
            input.events.push(egui::Event::PointerGone);
        }
        default_gfx_backend.prepare_frame(|| {
            let (width, height) = glfw_backend.window.get_framebuffer_size();
            [width.cast_unsigned(), height.cast_unsigned()]
        });
        egui_context.begin_pass(input);
        self.gui_run(egui_context, default_gfx_backend, glfw_backend);
        let egui::FullOutput { platform_output, textures_delta, shapes, pixels_per_point, .. } =
            egui_context.end_pass();
        default_gfx_backend.render_egui(
            egui_context.tessellate(shapes, pixels_per_point),
            textures_delta,
            glfw_backend.window_size_logical,
        );
        self.frames = self.frames.saturating_add(1);
        if self.frames >= 8
            && let Some(path) = &self.screenshot
        {
            crate::app::capture_screenshot(path, default_gfx_backend);
            glfw_backend.window.set_should_close(true);
        }
        if glfw_backend.is_opengl() {
            use egui_overlay::egui_window_glfw_passthrough::glfw::Context as _;
            glfw_backend.window.swap_buffers();
        }
        Some((platform_output, Duration::from_millis(33)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_requires_input_and_remembers_completion_without_storing_the_key() {
        let mut entropy = [0_u8; 8];
        getrandom::fill(&mut entropy).unwrap();
        let dir = std::env::temp_dir().join(format!("race-license-test-{}", u64::from_ne_bytes(entropy)));
        let path = dir.join("license-setup.toml");
        assert!(!setup_complete(&path));
        assert!(!accept_temporary_key(" \t ", &path).unwrap());
        assert!(!path.exists());
        assert!(accept_temporary_key(" any-temporary-key ", &path).unwrap());
        assert!(setup_complete(&path));
        assert!(!std::fs::read_to_string(&path).unwrap().contains("any-temporary-key"));
        for invalid in ["broken", "version = 2\nsetup_completed = true", "version = 1\nsetup_completed = false"] {
            std::fs::write(&path, invalid).unwrap();
            assert!(!setup_complete(&path));
        }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn a_failed_save_does_not_report_acceptance() {
        // A directory cannot be overwritten by a marker file.
        assert!(accept_temporary_key("temporary", &std::env::temp_dir()).is_err());
    }
}
