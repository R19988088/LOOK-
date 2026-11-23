// release 版不要弹黑色控制台窗口；debug 版留着，方便看日志和 panic
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod book;
mod decode;
mod ui;

use std::sync::Arc;
use std::time::Duration;

use ui::{Actions, Viewer};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

struct Gfx {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
}

impl Gfx {
    fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window.clone())
            .expect("创建 surface 失败");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("没有可用的 GPU 适配器");

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("comicpp"),
            required_features: wgpu::Features::empty(),
            // 长条漫画动辄 8000px 高，纹理上限直接吃适配器上限
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .expect("创建 GPU 设备失败");

        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface 配置失败");
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);

        let egui_ctx = egui::Context::default();
        egui_ctx.set_visuals(egui::Visuals::dark());
        install_fonts(&egui_ctx);

        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            Some(8192),
        );

        let renderer = egui_wgpu::Renderer::new(&device, config.format, Default::default());

        Self {
            surface,
            device,
            queue,
            config,
            egui_ctx,
            egui_state,
            renderer,
        }
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
    }
}

/// 参考项目的做法：给无标题栏窗口套上系统窗口外观（DWM 负责圆角与投影）。
#[cfg(target_os = "windows")]
fn apply_window_appearance(window: &Window, light: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_BORDER_COLOR: u32 = 34;

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attr: u32, pv: *const core::ffi::c_void, cb: u32)
            -> i32;
    }

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(w) = handle.as_raw() else {
        return;
    };
    let hwnd = w.hwnd.get();
    let dark: i32 = if light { 0 } else { 1 };
    // COLORREF（0x00BBGGRR）：DWM 描边不吃 alpha，用与浅底预混的深灰做出半透明观感
    let border: u32 = if light { 0x0076_7070 } else { 0x000C_0A0A };
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark as *const i32).cast(),
            4,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            (&border as *const u32).cast(),
            4,
        );
    }
}

fn install_fonts(ctx: &egui::Context) {
    // 西文/数字优先用 Segoe UI（比等线干净得多），中文回退微软雅黑 Light。
    let latin: &[&str] = &[
        r"C:\Windows\Fonts\segoeui.ttf",
        "/System/Library/Fonts/SFNS.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    let cjk: &[&str] = &[
        r"C:\Windows\Fonts\msyhl.ttc",
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\Deng.ttf",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
    ];

    let mut fonts = egui::FontDefinitions::default();
    let mut installed: Vec<&str> = Vec::new();
    for (key, list) in [("ui", latin), ("cjk", cjk)] {
        for path in list {
            if let Ok(bytes) = std::fs::read(path) {
                let mut data = egui::FontData::from_owned(bytes);
                data.index = 0;
                fonts.font_data.insert(key.to_owned(), data.into());
                installed.push(key);
                break;
            }
        }
    }

    let prop = fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default();
    for key in installed.iter().rev() {
        prop.insert(0, (*key).to_owned());
    }
    let mono = fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default();
    for key in installed.iter().rev() {
        mono.push((*key).to_owned());
    }
    ctx.set_fonts(fonts);
}

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    gfx: Option<Gfx>,
    viewer: Viewer,
    pending: Actions,
    /// 性能自测：自动连续翻页并记录帧时间
    auto: bool,
    frames: u64,
    acc: f64,
    worst: f64,
    /// 鼠标当前命中的窗口边缘（用于无边框窗口的自定义 resize）
    resize_plan: Option<ResizeDirection>,
}

impl App {
    fn render(&mut self) -> Option<Duration> {
        let t0 = std::time::Instant::now();
        let window = self.window.clone()?;
        let gfx = self.gfx.as_mut()?;

        let ppp = window.scale_factor() as f32;
        let ctx = gfx.egui_ctx.clone();
        ctx.set_pixels_per_point(ppp);
        let raw_input = gfx.egui_state.take_egui_input(&window);
        let mut full_output = ctx.run_ui(raw_input, |ui| {
            self.pending = self.viewer.ui(ui);
        });
        gfx.egui_state
            .handle_platform_output(&window, full_output.platform_output);

        let paint_jobs = gfx
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [gfx.config.width, gfx.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut textures_delta = std::mem::take(&mut full_output.textures_delta);
        for (id, deltas) in &textures_delta.set {
            for delta in deltas {
                gfx.renderer
                    .update_texture(&gfx.device, &gfx.queue, *id, delta);
            }
        }

        let output = match gfx.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gfx.surface.configure(&gfx.device, &gfx.config);
                return Some(Duration::ZERO);
            }
            _ => return Some(Duration::ZERO),
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gfx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        let user_bufs =
            gfx.renderer
                .update_buffers(&gfx.device, &gfx.queue, &mut encoder, &paint_jobs, &screen);

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.05,
                            b: 0.06,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            gfx.renderer.render(&mut pass, &paint_jobs, &screen);
        }

        gfx.queue
            .submit(user_bufs.into_iter().chain(std::iter::once(encoder.finish())));
        gfx.queue.present(output);

        for id in &textures_delta.free {
            gfx.renderer.free_texture(id);
        }
        textures_delta.clear();

        let delay = full_output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay)
            .unwrap_or(Duration::MAX);

        let frame_time = t0.elapsed().as_secs_f64();
        self.frames += 1;
        self.acc += frame_time;
        self.worst = self.worst.max(frame_time);
        if self.frames % 40 == 0 {
            use std::io::Write;
            let line = format!(
                "frames={} avg_ms={:.2} worst_ms={:.2} page={}/{}\n",
                self.frames,
                self.acc / 40.0 * 1000.0,
                self.worst * 1000.0,
                self.viewer.current + 1,
                self.viewer.total_public()
            );
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(r"E:\000 app\comic++\testdata\perf.txt")
                .and_then(|mut f| f.write_all(line.as_bytes()));
            self.acc = 0.0;
            self.worst = 0.0;
        }
        if self.auto {
            if self.frames % 3 == 0 {
                self.viewer.next_public();
            }
            return Some(Duration::ZERO);
        }
        Some(delay)
    }

    fn apply_actions(&mut self, window: &Arc<Window>, a: Actions) {
        if let Some(idx) = a.goto {
            self.viewer.goto_public(idx);
        }
        if a.drag {
            let _ = window.drag_window();
        }
        if a.close {
            std::process::exit(0);
        }
        if a.minimize {
            window.set_minimized(true);
        }
        if a.maximize {
            window.set_maximized(!window.is_maximized());
        }
        if a.open_dialog {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter(
                    "漫画 / 图片",
                    &[
                        "zip", "cbz", "7z", "cb7", "jpg", "jpeg", "png", "webp", "gif", "bmp",
                        "tif", "tiff", "avif",
                    ],
                )
                .pick_file()
            {
                self.viewer.open(&path);
                window.request_redraw();
            }
        }
        if let Some(path) = a.open_path {
            self.viewer.open(&path);
            window.request_redraw();
        }
        if a.toggle_theme {
            self.viewer.toggle_theme();
            #[cfg(target_os = "windows")]
            apply_window_appearance(window, self.viewer.theme_is_light());
            window.request_redraw();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("comic++")
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(1180.0, 690.0))
            .with_min_inner_size(LogicalSize::new(680.0, 460.0));
        // 和参考项目一样：无标题栏窗口留 1px 非客户区，DWM 才会画标准的窗口投影和圆角
        #[cfg(target_os = "windows")]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs.with_undecorated_shadow(true)
        };
        let window = Arc::new(event_loop.create_window(attrs).expect("创建窗口失败"));
        #[cfg(target_os = "windows")]
        apply_window_appearance(&window, self.viewer.theme_is_light());
        self.gfx = Some(Gfx::new(window.clone()));
        self.window = Some(window.clone());
        event_loop.set_control_flow(ControlFlow::Wait);
        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if id != window.id() {
            return;
        }

        let mut wants_repaint = false;
        if let Some(gfx) = self.gfx.as_mut() {
            let resp = gfx.egui_state.on_window_event(&window, &event);
            wants_repaint = resp.repaint;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gfx) = self.gfx.as_mut() {
                    gfx.resize(size);
                }
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => window.request_redraw(),
            WindowEvent::RedrawRequested => {
                let delay = self.render();
                let pending = std::mem::take(&mut self.pending);
                self.apply_actions(&window, pending);
                if let Some(delay) = delay {
                    if delay <= Duration::from_millis(50) {
                        window.request_redraw();
                    }
                }
            }
            WindowEvent::DroppedFile(path) => {
                self.viewer.open(&path);
                window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let sz = window.inner_size();
                let b = 7.0 * window.scale_factor();
                let (w, h) = (sz.width as f64, sz.height as f64);
                let left = position.x < b;
                let right = position.x > w - b;
                let top = position.y < b;
                let bottom = position.y > h - b;
                let dir = match (left, right, top, bottom) {
                    (true, _, true, _) => Some(ResizeDirection::NorthWest),
                    (_, true, true, _) => Some(ResizeDirection::NorthEast),
                    (true, _, _, true) => Some(ResizeDirection::SouthWest),
                    (_, true, _, true) => Some(ResizeDirection::SouthEast),
                    (true, _, _, _) => Some(ResizeDirection::West),
                    (_, true, _, _) => Some(ResizeDirection::East),
                    (_, _, true, _) => Some(ResizeDirection::North),
                    (_, _, _, true) => Some(ResizeDirection::South),
                    _ => None,
                };
                if self.resize_plan != dir {
                    self.resize_plan = dir;
                    window.set_cursor(match dir {
                        Some(ResizeDirection::North | ResizeDirection::South) => CursorIcon::NsResize,
                        Some(ResizeDirection::East | ResizeDirection::West) => CursorIcon::EwResize,
                        Some(ResizeDirection::NorthWest | ResizeDirection::SouthEast) => {
                            CursorIcon::NwseResize
                        }
                        Some(_) => CursorIcon::NeswResize,
                        None => CursorIcon::Default,
                    });
                }
                if wants_repaint {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let Some(dir) = self.resize_plan {
                    let _ = window.drag_resize_window(dir);
                }
                if wants_repaint {
                    window.request_redraw();
                }
            }
            _ => {
                if wants_repaint {
                    window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {}
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--mk7z") {
        let src = &args[i + 1];
        let dst = &args[i + 2];
        match sevenz_rust2::compress_to_path(src, dst) {
            Ok(_) => println!("ok {dst}"),
            Err(e) => println!("err {e}"),
        }
        return;
    }
    let event_loop = EventLoop::new().expect("创建事件循环失败");
    let mut app = App::default();
    if args.iter().any(|a| a == "--auto") {
        app.auto = true;
    }
    for path in args.iter().skip(1).filter(|a| !a.starts_with("--")) {
        app.viewer.open(std::path::Path::new(path));
    }
    event_loop.run_app(&mut app).expect("运行失败");
}
