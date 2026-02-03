#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(clippy::unwrap_used)]

/// Hide app from macOS dock (accessory app behavior)
#[cfg(target_os = "macos")]
fn set_macos_accessory_app() {
    // Use objc2 approach for setting activation policy
    extern "C" {
        fn objc_getClass(name: *const i8) -> *mut std::ffi::c_void;
        fn sel_registerName(name: *const i8) -> *mut std::ffi::c_void;
    }
    
    #[allow(improper_ctypes)]
    extern "C" {
        fn objc_msgSend(obj: *mut std::ffi::c_void, sel: *mut std::ffi::c_void, ...) -> *mut std::ffi::c_void;
    }
    
    unsafe {
        let cls = objc_getClass(b"NSApplication\0".as_ptr() as *const i8);
        if cls.is_null() { return; }
        
        let shared_sel = sel_registerName(b"sharedApplication\0".as_ptr() as *const i8);
        let app = objc_msgSend(cls, shared_sel);
        if app.is_null() { return; }
        
        let policy_sel = sel_registerName(b"setActivationPolicy:\0".as_ptr() as *const i8);
        // NSApplicationActivationPolicyAccessory = 1
        let _: *mut std::ffi::c_void = {
            type MsgSendFn = unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, i64) -> *mut std::ffi::c_void;
            let func: MsgSendFn = std::mem::transmute(objc_msgSend as *const ());
            func(app, policy_sel, 1)
        };
    }
}

#[cfg(not(target_os = "macos"))]
fn set_macos_accessory_app() {}

/// Activate the app and bring to front on macOS
#[cfg(target_os = "macos")]
fn activate_macos_app() {
    extern "C" {
        fn objc_getClass(name: *const i8) -> *mut std::ffi::c_void;
        fn sel_registerName(name: *const i8) -> *mut std::ffi::c_void;
    }
    
    #[allow(improper_ctypes)]
    extern "C" {
        fn objc_msgSend(obj: *mut std::ffi::c_void, sel: *mut std::ffi::c_void, ...) -> *mut std::ffi::c_void;
    }
    
    unsafe {
        let cls = objc_getClass(b"NSApplication\0".as_ptr() as *const i8);
        if cls.is_null() { return; }
        
        let shared_sel = sel_registerName(b"sharedApplication\0".as_ptr() as *const i8);
        let app = objc_msgSend(cls, shared_sel);
        if app.is_null() { return; }
        
        // activateIgnoringOtherApps:YES
        let activate_sel = sel_registerName(b"activateIgnoringOtherApps:\0".as_ptr() as *const i8);
        let _: *mut std::ffi::c_void = {
            type MsgSendFn = unsafe extern "C" fn(*mut std::ffi::c_void, *mut std::ffi::c_void, i8) -> *mut std::ffi::c_void;
            let func: MsgSendFn = std::mem::transmute(objc_msgSend as *const ());
            func(app, activate_sel, 1) // YES = 1
        };
    }
}

#[cfg(not(target_os = "macos"))]
fn activate_macos_app() {}

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use dirs::config_dir;
use egui_winit::winit;
use winit::raw_window_handle::HasWindowHandle as _;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

// ======== GUI Config ========

#[derive(serde::Deserialize, Default)]
struct GuiConfig {
    #[serde(default)]
    gui: GuiSettings,
}

#[derive(serde::Deserialize, Default)]
struct GuiSettings {
    #[serde(default)]
    proxy_binary: String,
    #[serde(default)]
    config_path: String,
}

fn load_gui_config() -> GuiConfig {
    // Try to find gui-config.toml in various locations
    let candidates = [
        // Next to the executable
        std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("gui-config.toml"))),
        // In current working directory
        Some(PathBuf::from("gui-config.toml")),
        // In cortex-proxy-gui directory (dev)
        Some(PathBuf::from("cortex-proxy-gui/gui-config.toml")),
        // In user config directory
        config_dir().map(|d| d.join("cortex-proxy/gui-config.toml")),
    ];
    
    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            if let Ok(contents) = fs::read_to_string(&candidate) {
                if let Ok(config) = toml::from_str(&contents) {
                    return config;
                }
            }
        }
    }
    GuiConfig::default()
}

// ======== Proxy Config Discovery (same order as proxy) ========

fn find_config_path() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(idx) = args.iter().position(|a| a == "--config") {
        if let Some(path) = args.get(idx + 1) {
            let p = PathBuf::from(path);
            if p.exists() {
                return Some(p);
            }
        }
    }
    if let Ok(path) = std::env::var("CORTEX_PROXY_CONFIG") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    for path_opt in [
        config_dir().map(|d| d.join("cortex-proxy/config.toml")),
        dirs::home_dir().map(|d| d.join(".config/cortex-proxy/config.toml")),
        Some(PathBuf::from("cortex-proxy.toml")),
    ] {
        if let Some(p) = path_opt {
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

fn default_config_path() -> PathBuf {
    config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cortex-proxy/config.toml")
}

fn example_config_text() -> &'static str {
    include_str!("../../cortex-proxy.example.toml")
}

/// Try to find bundled example config in app Resources folder (macOS)
fn bundled_example_config() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        // macOS: exe is in Contents/MacOS, resources in Contents/Resources
        if let Some(macos_dir) = exe.parent() {
            if let Some(contents_dir) = macos_dir.parent() {
                let resources = contents_dir.join("Resources/cortex-proxy.example.toml");
                if resources.exists() {
                    return Some(resources);
                }
            }
        }
        // Same directory as exe (Windows/Linux)
        if let Some(dir) = exe.parent() {
            let example = dir.join("cortex-proxy.example.toml");
            if example.exists() {
                return Some(example);
            }
        }
    }
    None
}

fn find_in_path(bin: &str) -> Option<PathBuf> {
    let mut candidates = vec![bin.to_string()];
    if cfg!(windows) && !bin.ends_with(".exe") {
        candidates.push(format!("{bin}.exe"));
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            for name in &candidates {
                let candidate = dir.join(name);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
            None
        })
    })
}

fn resolve_proxy_bin() -> String {
    // Check PATH first
    if let Some(path) = find_in_path("cortex-proxy") {
        return path.display().to_string();
    }
    
    // Check if bundled in same directory as GUI executable (macOS app bundle)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join("cortex-proxy");
            if bundled.exists() {
                return bundled.display().to_string();
            }
            // Windows
            let bundled_exe = dir.join("cortex-proxy.exe");
            if bundled_exe.exists() {
                return bundled_exe.display().to_string();
            }
        }
    }
    
    // Return empty string if not found - user must configure
    String::new()
}

// ======== Tray + GUI App ========

#[derive(Debug)]
enum UserEvent {
    Redraw(Duration),
    Tray(TrayIconEvent),
    Menu(MenuEvent),
}

struct AppState {
    config_path: PathBuf,
    config_path_input: String,
    config_text: String,
    proxy_bin: String,
    status: String,
    last_started: Option<Instant>,
    child: Option<Child>,
    log_rx: Option<mpsc::Receiver<String>>,
    logs: VecDeque<String>,
    show_window: bool,
    should_quit: bool,
}

impl AppState {
    fn new() -> Self {
        // Load GUI config first
        let gui_config = load_gui_config();
        
        // Use GUI config values if set, otherwise fall back to discovery
        let proxy_bin = if !gui_config.gui.proxy_binary.is_empty() {
            gui_config.gui.proxy_binary
        } else {
            resolve_proxy_bin()
        };
        
        let config_path = if !gui_config.gui.config_path.is_empty() {
            PathBuf::from(&gui_config.gui.config_path)
        } else {
            find_config_path().unwrap_or_else(default_config_path)
        };
        
        let config_text = fs::read_to_string(&config_path).unwrap_or_else(|_| {
            // No config found: start with example content
            example_config_text().to_string()
        });
        
        Self {
            config_path_input: config_path.display().to_string(),
            config_path,
            config_text,
            proxy_bin,
            status: "Stopped".to_string(),
            last_started: None,
            child: None,
            log_rx: None,
            logs: VecDeque::with_capacity(300),
            show_window: true,
            should_quit: false,
        }
    }

    fn append_log(&mut self, line: String) {
        if self.logs.len() >= 300 {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
    }

    fn load_config(&mut self) {
        self.sync_config_path();
        match fs::read_to_string(&self.config_path) {
            Ok(s) => self.config_text = s,
            Err(e) => self.append_log(format!("Failed to read config: {e}")),
        }
    }

    fn save_config(&mut self) {
        self.sync_config_path();
        if let Some(parent) = self.config_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match fs::write(&self.config_path, &self.config_text) {
            Ok(_) => self.append_log("Config saved.".to_string()),
            Err(e) => self.append_log(format!("Failed to save config: {e}")),
        }
    }

    fn copy_example_config(&mut self) {
        self.sync_config_path();
        if let Some(parent) = self.config_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match fs::write(&self.config_path, example_config_text()) {
            Ok(_) => {
                self.append_log("Wrote example config.".to_string());
                self.load_config();
            }
            Err(e) => self.append_log(format!("Failed to write example config: {e}")),
        }
    }

    fn start_proxy(&mut self) {
        if self.child.is_some() {
            self.append_log("Proxy already managed by this GUI.".to_string());
            return;
        }
        
        // Check if already running externally
        let port = self.proxy_port();
        if check_proxy_health(port) {
            self.status = format!("Running (external) :{}", port);
            self.append_log(format!("Proxy already running on port {}.", port));
            return;
        }
        
        // Validate binary path
        if self.proxy_bin.trim().is_empty() {
            self.append_log("Proxy binary path is empty.".to_string());
            self.append_log("Set the path above or install cortex-proxy in your PATH.".to_string());
            return;
        }
        if !PathBuf::from(&self.proxy_bin).exists() {
            self.append_log(format!("Proxy binary not found: {}", self.proxy_bin));
            self.append_log("Update the path above or install cortex-proxy in your PATH.".to_string());
            return;
        }
        
        // Validate config
        self.sync_config_path();
        if !self.config_path.exists() {
            if let Some(parent) = self.config_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(e) = fs::write(&self.config_path, example_config_text()) {
                self.append_log(format!("Failed to create config: {e}"));
                return;
            }
            self.config_text = example_config_text().to_string();
            self.append_log(format!("Created example config at {}", self.config_path.display()));
            self.append_log("Edit the config (add your Snowflake credentials) and click Start again.".to_string());
            return;
        }

        // Start the proxy
        self.append_log(format!("Starting: {} --config {}", self.proxy_bin, self.config_path.display()));
        let mut cmd = Command::new(&self.proxy_bin);
        cmd.arg("--config").arg(&self.config_path);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        match cmd.spawn() {
            Ok(mut child) => {
                self.status = "Starting...".to_string();
                self.last_started = Some(Instant::now());
                let (tx, rx) = mpsc::channel();
                if let Some(stdout) = child.stdout.take() {
                    let tx = tx.clone();
                    std::thread::spawn(move || {
                        let reader = BufReader::new(stdout);
                        for line in reader.lines().flatten() {
                            let _ = tx.send(line);
                        }
                    });
                }
                if let Some(stderr) = child.stderr.take() {
                    let tx = tx.clone();
                    std::thread::spawn(move || {
                        let reader = BufReader::new(stderr);
                        for line in reader.lines().flatten() {
                            let _ = tx.send(line);
                        }
                    });
                }
                self.child = Some(child);
                self.log_rx = Some(rx);
                self.append_log("Proxy process spawned. Waiting for health check...".to_string());
            }
            Err(e) => self.append_log(format!("Failed to start proxy: {e}")),
        }
    }

    fn stop_proxy(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.status = "Stopped".to_string();
        self.last_started = None;
        self.append_log("Proxy stopped.".to_string());
    }

    fn poll_child(&mut self) {
        // Collect logs first
        if let Some(rx) = &self.log_rx {
            let mut pending = Vec::new();
            while let Ok(line) = rx.try_recv() {
                pending.push(line);
            }
            for line in pending {
                self.append_log(line);
            }
        }

        // Check if our child process exited
        if let Some(child) = &mut self.child {
            if let Ok(Some(status)) = child.try_wait() {
                self.append_log(format!("Proxy exited: {}", status));
                self.last_started = None;
                self.child = None;
            }
        }

        // Update status based on actual port state
        self.refresh_status();
    }

    fn refresh_status(&mut self) {
        let port = self.proxy_port();
        let healthy = check_proxy_health(port);

        if self.child.is_some() && healthy {
            // GUI started the proxy and it's responding
            self.status = format!("Running :{}", port);
        } else if self.child.is_some() && !healthy {
            // GUI started it but it's not responding yet (starting up or crashed)
            if let Some(started) = self.last_started {
                if started.elapsed().as_secs() < 3 {
                    self.status = "Starting...".to_string();
                } else {
                    self.status = "Not responding".to_string();
                }
            }
        } else if healthy {
            // Healthy but not our child - external process
            self.status = format!("Running (external) :{}", port);
        } else {
            self.status = "Stopped".to_string();
            self.last_started = None;
        }
    }

    fn is_running(&self) -> bool {
        check_proxy_health(self.proxy_port())
    }

    fn toggle_proxy(&mut self) {
        if self.is_running() {
            // Try to stop
            if self.child.is_some() {
                self.stop_proxy();
            } else {
                self.append_log(
                    "Proxy running externally. Stop it from its own terminal/process.".to_string(),
                );
            }
        } else {
            self.start_proxy();
        }
    }

    fn sync_config_path(&mut self) {
        let input = self.config_path_input.trim();
        if !input.is_empty() {
            self.config_path = PathBuf::from(input);
        }
    }

    fn proxy_port(&self) -> u16 {
        parse_port_from_toml(&self.config_text).unwrap_or(8766)
    }
}

// ======== Winit + Egui Glow ========

struct GlutinWindowContext {
    window: winit::window::Window,
    gl_context: glutin::context::PossiblyCurrentContext,
    gl_display: glutin::display::Display,
    gl_surface: glutin::surface::Surface<glutin::surface::WindowSurface>,
}

impl GlutinWindowContext {
    unsafe fn new(event_loop: &winit::event_loop::ActiveEventLoop) -> Self {
        use glutin::context::NotCurrentGlContext as _;
        use glutin::display::GetGlDisplay as _;
        use glutin::display::GlDisplay as _;
        use glutin::prelude::GlSurface as _;

        let winit_window_builder = winit::window::WindowAttributes::default()
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize {
                width: 900.0,
                height: 680.0,
            })
            .with_title("Cortex Proxy")
            .with_visible(true);

        let config_template_builder = glutin::config::ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false);

        let (window, gl_config) = glutin_winit::DisplayBuilder::new()
            .with_preference(glutin_winit::ApiPreference::FallbackEgl)
            .with_window_attributes(Some(winit_window_builder.clone()))
            .build(event_loop, config_template_builder, |mut config_iterator| {
                config_iterator
                    .next()
                    .expect("failed to find a matching configuration")
            })
            .expect("failed to create gl_config");

        let gl_display = gl_config.display();
        let window = window.expect("window not created");
        let raw_window_handle = Some(
            window
                .window_handle()
                .expect("no window handle")
                .as_raw(),
        );
        let context_attributes =
            glutin::context::ContextAttributesBuilder::new().build(raw_window_handle);
        let fallback_context_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::Gles(None))
            .build(raw_window_handle);
        let not_current_gl_context = gl_display
            .create_context(&gl_config, &context_attributes)
            .unwrap_or_else(|_| {
                gl_config
                    .display()
                    .create_context(&gl_config, &fallback_context_attributes)
                    .expect("failed to create gl context")
            });

        let (width, height): (u32, u32) = window.inner_size().into();
        let width = std::num::NonZeroU32::new(width).unwrap_or(std::num::NonZeroU32::MIN);
        let height = std::num::NonZeroU32::new(height).unwrap_or(std::num::NonZeroU32::MIN);
        let surface_attributes = glutin::surface::SurfaceAttributesBuilder::<glutin::surface::WindowSurface>::new()
            .build(
                window
                    .window_handle()
                    .expect("no window handle")
                    .as_raw(),
                width,
                height,
            );
        let gl_surface = gl_display
            .create_window_surface(&gl_config, &surface_attributes)
            .unwrap();
        let gl_context = not_current_gl_context.make_current(&gl_surface).unwrap();

        gl_surface
            .set_swap_interval(&gl_context, glutin::surface::SwapInterval::Wait(std::num::NonZeroU32::MIN))
            .unwrap();

        Self {
            window,
            gl_context,
            gl_display,
            gl_surface,
        }
    }

    fn window(&self) -> &winit::window::Window {
        &self.window
    }

    fn resize(&self, physical_size: winit::dpi::PhysicalSize<u32>) {
        use glutin::surface::GlSurface as _;
        self.gl_surface.resize(
            &self.gl_context,
            physical_size.width.try_into().unwrap(),
            physical_size.height.try_into().unwrap(),
        );
    }

    fn swap_buffers(&self) -> glutin::error::Result<()> {
        use glutin::surface::GlSurface as _;
        self.gl_surface.swap_buffers(&self.gl_context)
    }

    fn get_proc_address(&self, addr: &std::ffi::CStr) -> *const std::ffi::c_void {
        use glutin::display::GlDisplay as _;
        self.gl_display.get_proc_address(addr)
    }
}

struct GuiApp {
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
    gl_window: Option<GlutinWindowContext>,
    gl: Option<Arc<glow::Context>>,
    egui_glow: Option<egui_glow::EguiGlow>,
    repaint_delay: Duration,
    tray_icon: Option<TrayIcon>,
    menu_ids: MenuIds,
    state: AppState,
}

#[derive(Default, Clone)]
struct MenuIds {
    show_hide: Option<tray_icon::menu::MenuId>,
    start: Option<tray_icon::menu::MenuId>,
    stop: Option<tray_icon::menu::MenuId>,
    copy_example: Option<tray_icon::menu::MenuId>,
    quit: Option<tray_icon::menu::MenuId>,
}

impl GuiApp {
    fn new(proxy: winit::event_loop::EventLoopProxy<UserEvent>) -> Self {
        Self {
            proxy,
            gl_window: None,
            gl: None,
            egui_glow: None,
            repaint_delay: Duration::MAX,
            tray_icon: None,
            menu_ids: MenuIds::default(),
            state: AppState::new(),
        }
    }

    fn build_tray(&mut self) {
        let menu = Menu::new();
        let show_hide = MenuItem::new("Show/Hide", true, None);
        let start = MenuItem::new("Start Proxy", true, None);
        let stop = MenuItem::new("Stop Proxy", true, None);
        let copy_example = MenuItem::new("Write Example Config", true, None);
        let quit = MenuItem::new("Quit", true, None);

        menu.append(&show_hide).ok();
        menu.append(&start).ok();
        menu.append(&stop).ok();
        menu.append(&PredefinedMenuItem::separator()).ok();
        menu.append(&copy_example).ok();
        menu.append(&PredefinedMenuItem::separator()).ok();
        menu.append(&quit).ok();

        let icon = load_tray_icon();
        let tray_icon = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("Cortex Proxy")
            .with_menu(Box::new(menu))
            .build()
            .ok();

        self.menu_ids = MenuIds {
            show_hide: Some(show_hide.id().clone()),
            start: Some(start.id().clone()),
            stop: Some(stop.id().clone()),
            copy_example: Some(copy_example.id().clone()),
            quit: Some(quit.id().clone()),
        };
        self.tray_icon = tray_icon;
    }
}

impl winit::application::ApplicationHandler<UserEvent> for GuiApp {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        // Hide from dock on macOS AFTER window is created
        set_macos_accessory_app();
        
        let (gl_window, gl) = create_display(event_loop);
        let gl = Arc::new(gl);
        gl_window.window().set_visible(self.state.show_window);

        let egui_glow = egui_glow::EguiGlow::new(event_loop, Arc::clone(&gl), None, None, true);
        let proxy = self.proxy.clone();
        egui_glow
            .egui_ctx
            .set_request_repaint_callback(move |info| {
                let _ = proxy.send_event(UserEvent::Redraw(info.delay));
            });

        self.gl_window = Some(gl_window);
        self.gl = Some(gl);
        self.egui_glow = Some(egui_glow);

        tray_icon::TrayIconEvent::set_event_handler(Some({
            let proxy = self.proxy.clone();
            move |event| {
                let _ = proxy.send_event(UserEvent::Tray(event));
            }
        }));
        tray_icon::menu::MenuEvent::set_event_handler(Some({
            let proxy = self.proxy.clone();
            move |event| {
                let _ = proxy.send_event(UserEvent::Menu(event));
            }
        }));
        self.build_tray();
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        use winit::event::WindowEvent;

        if matches!(event, WindowEvent::CloseRequested | WindowEvent::Destroyed) {
            if let Some(win) = self.gl_window.as_ref().map(|w| w.window()) {
                win.set_visible(false);
                self.state.show_window = false;
            }
            return;
        }

        if matches!(event, WindowEvent::Resized(_)) {
            if let WindowEvent::Resized(physical_size) = &event {
                if let Some(gl_window) = &self.gl_window {
                    gl_window.resize(*physical_size);
                }
            }
        }

        if matches!(event, WindowEvent::RedrawRequested) {
            self.state.poll_child();
            self.draw(event_loop);
            return;
        }

        if let Some(egui_glow) = &mut self.egui_glow {
            let response = egui_glow.on_window_event(self.gl_window.as_ref().unwrap().window(), &event);
            if response.repaint {
                self.gl_window.as_ref().unwrap().window().request_redraw();
            }
        }
    }

    fn user_event(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Redraw(delay) => self.repaint_delay = delay,
            UserEvent::Tray(_event) => {}
            UserEvent::Menu(event) => {
                if Some(&event.id) == self.menu_ids.show_hide.as_ref() {
                    self.state.show_window = !self.state.show_window;
                    if let Some(win) = self.gl_window.as_ref().map(|w| w.window()) {
                        win.set_visible(self.state.show_window);
                        if self.state.show_window {
                            // Bring window to front on macOS
                            win.focus_window();
                            win.request_redraw();
                            // Also activate the app on macOS
                            #[cfg(target_os = "macos")]
                            activate_macos_app();
                        }
                    }
                } else if Some(&event.id) == self.menu_ids.start.as_ref() {
                    self.state.start_proxy();
                } else if Some(&event.id) == self.menu_ids.stop.as_ref() {
                    self.state.stop_proxy();
                } else if Some(&event.id) == self.menu_ids.copy_example.as_ref() {
                    self.state.copy_example_config();
                } else if Some(&event.id) == self.menu_ids.quit.as_ref() {
                    self.state.should_quit = true;
                    event_loop.exit();
                }
            }
        }
    }

    fn new_events(&mut self, event_loop: &winit::event_loop::ActiveEventLoop, cause: winit::event::StartCause) {
        if let winit::event::StartCause::ResumeTimeReached { .. } = cause {
            if let Some(win) = self.gl_window.as_ref().map(|w| w.window()) {
                win.request_redraw();
            }
        }
        event_loop.set_control_flow(if self.repaint_delay.is_zero() {
            winit::event_loop::ControlFlow::Poll
        } else if let Some(instant) = Instant::now().checked_add(self.repaint_delay) {
            winit::event_loop::ControlFlow::WaitUntil(instant)
        } else {
            winit::event_loop::ControlFlow::Wait
        });
    }

    fn exiting(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        if let Some(egui_glow) = &mut self.egui_glow {
            egui_glow.destroy();
        }
        self.state.stop_proxy();
    }
}

impl GuiApp {
    fn draw(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let mut quit = false;

        if let Some(egui_glow) = &mut self.egui_glow {
            egui_glow.run(self.gl_window.as_ref().unwrap().window(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.heading("Snowflake Cortex Proxy");
                    ui.separator();

                    let is_running = self.state.is_running();
                    ui.label(format!("Status: {}", self.state.status));
                    if let Some(started) = self.state.last_started {
                        if self.state.child.is_some() {
                            ui.label(format!("Uptime: {}s", started.elapsed().as_secs()));
                        }
                    }

                    // Single toggle button
                    let button_label = if is_running { "Stop" } else { "Start" };
                    if ui.button(button_label).clicked() {
                        self.state.toggle_proxy();
                    }

                    ui.separator();
                    ui.label("Proxy binary:");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.state.proxy_bin);
                        if ui.button("Browse").clicked() {
                            if let Some(path) = rfd::FileDialog::new().pick_file() {
                                self.state.proxy_bin = path.display().to_string();
                            }
                        }
                    });

                    ui.label("Config path:");
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.state.config_path_input);
                        if ui.button("Browse").clicked() {
                            if let Some(path) = rfd::FileDialog::new().save_file() {
                                self.state.config_path_input = path.display().to_string();
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Load").clicked() {
                            self.state.load_config();
                        }
                        if ui.button("Save").clicked() {
                            self.state.save_config();
                        }
                        if ui.button("Write Example").clicked() {
                            self.state.copy_example_config();
                        }
                    });

                    ui.separator();
                    ui.label("Config:");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.state.config_text)
                            .desired_rows(16)
                            .code_editor(),
                    );

                    ui.separator();
                    ui.label("Logs:");
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        for line in &self.state.logs {
                            ui.label(line);
                        }
                    });

                    if ui.button("Quit").clicked() {
                        quit = true;
                    }
                });
            });
        }

        if quit {
            self.state.should_quit = true;
            event_loop.exit();
        }

        unsafe {
            use glow::HasContext as _;
            self.gl.as_ref().unwrap().clear_color(0.08, 0.08, 0.08, 1.0);
            self.gl.as_ref().unwrap().clear(glow::COLOR_BUFFER_BIT);
        }

        if let Some(egui_glow) = &mut self.egui_glow {
            egui_glow.paint(self.gl_window.as_ref().unwrap().window());
        }

        let _ = self.gl_window.as_ref().unwrap().swap_buffers();
    }
}

fn load_tray_icon() -> Icon {
    let bytes = include_bytes!("../../Opencode_cortex_proxy.png");
    let image = image::load_from_memory(bytes).unwrap().to_rgba8();
    // Resize to 22x22 for macOS menu bar (standard size)
    let resized = image::imageops::resize(&image, 22, 22, image::imageops::FilterType::Lanczos3);
    let (width, height) = resized.dimensions();
    Icon::from_rgba(resized.into_raw(), width, height).unwrap()
}

fn parse_port_from_toml(text: &str) -> Option<u16> {
    let val: toml::Value = toml::from_str(text).ok()?;
    val.get("proxy")?
        .get("port")?
        .as_integer()
        .and_then(|p| u16::try_from(p).ok())
}

fn is_port_open(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

/// Check if the proxy is responding via /health endpoint
fn check_proxy_health(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
        use std::io::{Read, Write};
        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
        // Hit the /health endpoint
        let request = "GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        if stream.write_all(request.as_bytes()).is_ok() {
            let mut buf = [0u8; 512];
            if let Ok(n) = stream.read(&mut buf) {
                let response = String::from_utf8_lossy(&buf[..n]);
                // Check for "status":"ok" in the JSON response
                return response.contains("\"status\":\"ok\"") || response.contains("\"status\": \"ok\"");
            }
        }
    }
    false
}

fn create_display(
    event_loop: &winit::event_loop::ActiveEventLoop,
) -> (GlutinWindowContext, glow::Context) {
    let glutin_window_context = unsafe { GlutinWindowContext::new(event_loop) };
    let gl = unsafe {
        glow::Context::from_loader_function(|s| {
            let s = std::ffi::CString::new(s).expect("gl proc address");
            glutin_window_context.get_proc_address(&s)
        })
    };
    (glutin_window_context, gl)
}

fn main() {
    let event_loop = winit::event_loop::EventLoop::<UserEvent>::with_user_event()
        .build()
        .unwrap();
    let proxy = event_loop.create_proxy();
    let mut app = GuiApp::new(proxy);
    event_loop.run_app(&mut app).expect("failed to run app");
}
