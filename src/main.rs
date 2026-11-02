// Release builds on Windows are GUI apps: they have no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui_wgpu::{SurfaceErrorAction, WgpuConfiguration, WgpuSetup};
use eframe::wgpu;
use melodia::config::{APP_NAME, AppConfig};
use melodia::ui::MusicApp;
#[cfg(windows)]
use melodia::webplayer::{EmbeddedPlayer, HostRequest, WebPlayerHost};
use melodia::{backend, icon, util};

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
/// Renderer: `wgpu` (DirectX 12 / Vulkan / Metal, default) or `glow` (OpenGL).
const RENDERER_ENV: &str = "MELODIA_RENDERER";
/// Development only: makes all animations instant.
const NO_ANIMATIONS_ENV: &str = "MELODIA_NO_ANIMATIONS";

fn main() -> eframe::Result {
    util::startup_mark("main started");
    let config = AppConfig::load();
    init_logging(&config.log_path());
    util::startup_mark("configuration loaded and logging started");
    let renderer = match std::env::var(RENDERER_ENV) {
        Ok(value)
            if matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "glow" | "opengl"
            ) =>
        {
            eframe::Renderer::Glow
        }
        _ => eframe::Renderer::Wgpu,
    };
    log::info!(
        "{APP_NAME} {} starting (demo: {}, renderer: {renderer:?}, config: {})",
        env!("CARGO_PKG_VERSION"),
        config.demo,
        config.config_dir.display()
    );

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_app_id("melodia")
            // The app draws its own title bar and window buttons.
            .with_decorations(false)
            .with_inner_size([1120.0, 720.0])
            .with_min_inner_size([880.0, 600.0])
            .with_icon(window_icon()),
        renderer,
        centered: true,
        wgpu_options: wgpu_options(),
        ..Default::default()
    };

    let demo = config.demo;
    util::startup_mark("opening the window");
    let result = eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            util::startup_mark("window and renderer ready");
            log_renderer(cc);
            #[cfg_attr(not(windows), expect(unused_mut))]
            let mut config = config;
            // «Play in this app»: the window hosts the player page.
            #[cfg(windows)]
            let web_player = {
                let ctx = cc.egui_ctx.clone();
                let (host, requests) = WebPlayerHost::new(move || ctx.request_repaint());
                let key_ctx = cc.egui_ctx.clone();
                if std::env::var_os("MELODIA_WEB_PROBE").is_some() {
                    probe_web_player(&host);
                }
                config.web_player = Some(host);
                EmbeddedPlayer::new(requests, config.web_data_dir(), move || {
                    key_ctx.request_repaint();
                })
            };
            let ctx = cc.egui_ctx.clone();
            let backend = backend::spawn(config, move || ctx.request_repaint());
            util::startup_mark("backend thread launched");
            let mut music = MusicApp::new(backend, demo);
            music.install(&cc.egui_ctx);
            util::startup_mark("interface installed (theme and logo textures)");
            // For the smoke test: instant transitions (as in the UI tests), so that the test can
            // take each screenshot immediately.
            if std::env::var_os(NO_ANIMATIONS_ENV).is_some() {
                cc.egui_ctx
                    .all_styles_mut(|style| style.animation_time = 0.0);
            }
            util::startup_mark("creation finished");
            Ok(Box::new(App {
                music,
                frames: 0,
                #[cfg(windows)]
                web_player: Some(web_player),
                #[cfg(windows)]
                close_deadline: None,
                #[cfg(windows)]
                close_now: false,
            }))
        }),
    );
    match result {
        // Some drivers cannot run the GPU renderer: use OpenGL.
        Err(eframe::Error::Wgpu(error)) if matches!(renderer, eframe::Renderer::Wgpu) => {
            log::error!("The wgpu renderer could not start ({error}); restarting with OpenGL");
            restart_with_opengl()
        }
        other => other,
    }
}

/// For smoke tests (`MELODIA_WEB_PROBE=1`): opens the player page immediately, without a Spotify
/// session, and logs its messages.
#[cfg(windows)]
fn probe_web_player(host: &WebPlayerHost) {
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    host.send(HostRequest::Start {
        generation: 0,
        volume: None,
        events,
    });
    std::thread::spawn(move || {
        while let Some((_, event)) = received.blocking_recv() {
            log::warn!("[web probe] {event:?}");
        }
    });
}

/// The interface, plus the hidden player page on Windows.
struct App {
    music: MusicApp,
    /// The number of frames drawn, counted only up to the frames that are logged.
    frames: u8,
    #[cfg(windows)]
    web_player: Option<EmbeddedPlayer>,
    /// Set while the window waits for the player page before it closes.
    #[cfg(windows)]
    close_deadline: Option<Instant>,
    /// The page is ready (or the wait was too long): the window closes permanently.
    #[cfg(windows)]
    close_now: bool,
}

/// The maximum time that the window waits for the player page before it closes.
#[cfg(windows)]
const CLOSE_GRACE: Duration = Duration::from_secs(3);

impl eframe::App for App {
    #[cfg(windows)]
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // WebView2 runs in the thread of the window: the backend's requests are handled here.
        let Some(web_player) = &mut self.web_player else {
            return;
        };
        web_player.pump();

        // Before it closes, the page pauses the music if it plays here, so that Spotify gets the
        // position (see `docs/webplayer.md`). The window becomes hidden immediately. The
        // process continues until the page sends the pause.
        if ctx.input(|i| i.viewport().close_requested()) && !self.close_now {
            if self.close_deadline.is_none() {
                if web_player.prepare_to_close() {
                    self.close_deadline = Some(Instant::now() + CLOSE_GRACE);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                } else {
                    self.close_now = true;
                }
            }
            if !self.close_now {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        if let Some(deadline) = self.close_deadline
            && !self.close_now
        {
            if web_player.ready_to_close() || Instant::now() >= deadline {
                if !web_player.ready_to_close() {
                    log::warn!("The web player did not answer in time: closing anyway");
                }
                self.close_now = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint_after(Duration::from_millis(30));
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // The first frames, to see when the window really shows something.
        if self.frames < 3 {
            self.frames += 1;
            util::startup_mark(&format!("frame {} started", self.frames));
        }
        self.music.ui(ui, frame);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.music.clear_color(visuals)
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Close the page while the window still exists.
        #[cfg(windows)]
        drop(self.web_player.take());
        // The process ends immediately after this: let the backend save first.
        self.music.shut_down();
    }
}

/// wgpu settings. On Windows, DirectX 12 (if `WGPU_BACKEND` does not tell a different value).
/// wgpu would often select Vulkan, and its drivers can stop or not show frames when the window
/// changes size.
fn wgpu_options() -> WgpuConfiguration {
    let mut options = WgpuConfiguration::default();
    if let WgpuSetup::CreateNew(setup) = &mut options.wgpu_setup
        && cfg!(windows)
        && wgpu::Backends::from_env().is_none()
    {
        setup.instance_descriptor.backends = wgpu::Backends::DX12;
    }
    let recovery = SurfaceRecovery::default();
    options.on_surface_status = Arc::new(move |status| recovery.action(status));
    options
}

/// What to do when a frame cannot show. On a timeout, egui skips the frame and does not try again.
/// After a resize, that can repeat without end, and the last image stays on the screen while the
/// app continues. Thus the function sets up the window surface again (two times a second at most),
/// which also asks for a new frame.
#[derive(Default)]
struct SurfaceRecovery {
    last_reconfigured: Mutex<Option<Instant>>,
}

impl SurfaceRecovery {
    const MIN_INTERVAL: Duration = Duration::from_millis(500);

    fn action(&self, status: &wgpu::CurrentSurfaceTexture) -> SurfaceErrorAction {
        match status {
            wgpu::CurrentSurfaceTexture::Lost => {
                log::warn!("The window surface was lost; creating it again");
                SurfaceErrorAction::RecreateSurface
            }
            wgpu::CurrentSurfaceTexture::Outdated => SurfaceErrorAction::Reconfigure,
            wgpu::CurrentSurfaceTexture::Occluded => SurfaceErrorAction::SkipFrame,
            other => {
                let mut last_reconfigured = util::lock(&self.last_reconfigured);
                let now = Instant::now();
                if last_reconfigured.is_some_and(|at| now.duration_since(at) < Self::MIN_INTERVAL) {
                    return SurfaceErrorAction::SkipFrame;
                }
                *last_reconfigured = Some(now);
                log::warn!(
                    "A frame could not be shown ({other:?}); setting up the window surface again"
                );
                SurfaceErrorAction::Reconfigure
            }
        }
    }
}

/// Writes the renderer that the app uses to the log (useful for display problems).
fn log_renderer(cc: &eframe::CreationContext<'_>) {
    if let Some(render_state) = &cc.wgpu_render_state {
        let info = render_state.adapter.get_info();
        log::info!(
            "Renderer: wgpu {:?} on {} ({:?}; driver {} {})",
            info.backend,
            info.name,
            info.device_type,
            info.driver,
            info.driver_info
        );
    } else if cc.gl.is_some() {
        log::info!("Renderer: OpenGL (glow)");
    }
}

/// Starts a new instance with the OpenGL renderer. One process cannot create the window event loop
/// two times, so this process only exits.
fn restart_with_opengl() -> eframe::Result {
    let to_eframe_error = |e: io::Error| eframe::Error::AppCreation(Box::new(e));
    let exe = std::env::current_exe().map_err(to_eframe_error)?;
    std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(RENDERER_ENV, "glow")
        .spawn()
        .map_err(to_eframe_error)?;
    Ok(())
}

fn window_icon() -> egui::IconData {
    let size = 128;
    egui::IconData {
        rgba: icon::render_rgba(size),
        width: size,
        height: size,
    }
}

/// Logs to stderr and to a file in the configuration directory (release builds on Windows have no
/// console). `RUST_LOG` replaces the level.
fn init_logging(path: &Path) {
    let env = env_logger::Env::default().default_filter_or("warn,melodia=info");
    let mut builder = env_logger::Builder::from_env(env);
    // Milliseconds: the player page and Spotify answer with less than a second between them.
    builder.format_timestamp_millis();
    if let Some(file) = open_log_file(path) {
        builder.target(env_logger::Target::Pipe(Box::new(StderrAndFile(file))));
    }
    let _ = builder.try_init();
}

fn open_log_file(path: &Path) -> Option<File> {
    std::fs::create_dir_all(path.parent()?).ok()?;
    let too_big = std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG_BYTES);
    OpenOptions::new()
        .create(true)
        .write(true)
        .append(!too_big)
        .truncate(too_big)
        .open(path)
        .ok()
}

struct StderrAndFile(File);

impl Write for StderrAndFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        self.0.flush()
    }
}
