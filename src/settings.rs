#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
#[path = "runtime.rs"]
mod runtime;
#[path = "status.rs"]
mod status;

use eframe::egui;
use hamokisync_lite::protocol::{ClientError, ProtocolClient};
use runtime::{load_preferences, read_saved_password, save_password, save_preferences};
use std::sync::mpsc;
use std::time::{Duration, Instant};

// The tray can deliver several mouse events for one gesture. Gate every
// settings entry point before reading credentials or creating a window.
#[cfg(windows)]
mod single_window {
    use std::ffi::c_void;
    use std::ptr::null;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, FlashWindow, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(
            attributes: *const c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
    }

    pub struct Instance(*mut c_void);

    impl Drop for Instance {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub fn acquire() -> Option<Instance> {
        let name: Vec<u16> = r"Local\HamokiSyncSettings"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if handle.is_null() {
            return None;
        }
        let instance = Instance(handle);
        if !already_exists {
            return Some(instance);
        }
        // A second launch can arrive while the first is still initializing.
        drop(instance);
        let title: Vec<u16> = "HamokiSync"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let hwnd = unsafe { FindWindowW(null(), title.as_ptr()) };
            if !hwnd.is_null() {
                unsafe {
                    ShowWindow(
                        hwnd,
                        if IsIconic(hwnd) != 0 {
                            SW_RESTORE
                        } else {
                            SW_SHOW
                        },
                    );
                    if SetForegroundWindow(hwnd) == 0 {
                        FlashWindow(hwnd, 1);
                    }
                }
                break;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }
}

#[cfg(windows)]
fn register_autostart() -> Result<(), String> {
    use winreg::{enums::*, RegKey};
    let exe = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("hamokisync-daemon.exe");
    if !exe.is_file() {
        return Err("未找到后台程序 hamokisync-daemon.exe".into());
    }
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = hkcu
        .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        .map_err(|e| e.to_string())?;
    run.set_value("HamokiSync", &format!("\"{}\"", exe.display()))
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn register_autostart() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("hamokisync-daemon");
    if !exe.is_file() {
        return Err("未找到后台程序".into());
    }
    let config = directories::BaseDirs::new().ok_or("No config directory")?;
    let dir = config.config_dir().join("autostart");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = exe.to_str().ok_or("Invalid executable path")?;
    if path.contains(['\n', '\r', '"', '\\', '%', '$', '`']) {
        return Err("Unsupported installation path".into());
    }
    std::fs::write(dir.join("hamokisync.desktop"), format!("[Desktop Entry]\nType=Application\nName=HamokiSync\nExec=\"{path}\"\nTerminal=false\nX-GNOME-Autostart-enabled=true\n")).map_err(|e| e.to_string())
}
#[cfg(target_os = "macos")]
fn register_autostart() -> Result<(), String> {
    let daemon = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("hamokisync-daemon");
    if !daemon.is_file() {
        return Err("未找到后台程序".into());
    }
    let home = directories::BaseDirs::new().ok_or("No home directory")?;
    let dir = home.home_dir().join("Library/LaunchAgents");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = daemon
        .to_str()
        .ok_or("Invalid executable path")?
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>io.hamoki.HamokiSync</string>
<key>ProgramArguments</key><array><string>{path}</string></array>
<key>RunAtLoad</key><true/>
<key>LimitLoadToSessionType</key><string>Aqua</string>
</dict></plist>"#
    );
    std::fs::write(dir.join("io.hamoki.HamokiSync.plist"), plist).map_err(|e| e.to_string())
}
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn register_autostart() -> Result<(), String> {
    Ok(())
}

fn launch_daemon() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let daemon = exe.with_file_name(if cfg!(windows) {
        "hamokisync-daemon.exe"
    } else {
        "hamokisync-daemon"
    });
    if !daemon.is_file() {
        return Err("后台程序尚未安装".into());
    }
    std::process::Command::new(daemon)
        .spawn()
        .map_err(|e| e.to_string())
        .map(|_| ())
}

fn install_system_font(ctx: &egui::Context) {
    let candidates = if cfg!(windows) {
        vec![r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\simsun.ttc"]
    } else if cfg!(target_os = "macos") {
        vec!["/System/Library/Fonts/PingFang.ttc"]
    } else {
        vec![
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        ]
    };
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "system-cjk".into(),
                egui::FontData::from_owned(bytes).into(),
            );
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                family.insert(0, "system-cjk".into());
            }
            ctx.set_fonts(fonts);
            break;
        }
    }
}

struct Settings {
    endpoint: String,
    username: String,
    password: String,
    plain: bool,
    message: String,
    message_until: Option<Instant>,
    verification: Option<mpsc::Receiver<Result<(), String>>>,
}
impl Settings {
    fn new() -> Self {
        let p = load_preferences();
        Self {
            password: read_saved_password(&p.endpoint, &p.username).unwrap_or_default(),
            endpoint: p.endpoint,
            username: p.username,
            plain: false,
            message: String::new(),
            message_until: None,
            verification: None,
        }
    }
}
// The daemon status, not a successful password save, determines the indicator.
fn connection_indicator(detail: &str) -> (&'static str, egui::Color32) {
    if detail == "已连接，正在同步" {
        ("已连接", egui::Color32::from_rgb(34, 158, 91))
    } else {
        ("未连接", egui::Color32::from_rgb(222, 79, 76))
    }
}

#[cfg(test)]
mod indicator_tests {
    use super::connection_indicator;

    #[test]
    fn only_live_daemon_connection_is_green() {
        assert_eq!(connection_indicator("已连接，正在同步").0, "已连接");
        for state in [
            "尚未配置同步密码",
            "密码或账号认证失败（服务器返回 401）",
            "网络连接失败，正在重试",
            "正在连接同步服务器",
            "后台状态未知（可能已退出）",
            "同步后台已退出",
            "等待连接状态",
            "上传结果不确定，重新建立同步基线",
            "密码已保存",
        ] {
            assert_eq!(connection_indicator(state).0, "未连接");
            assert_ne!(
                connection_indicator(state).1,
                connection_indicator("已连接，正在同步").1
            );
        }
    }
}

impl eframe::App for Settings {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_secs(1));
        if let Some(result) = self.verification.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.verification = None;
            self.message_until = None;
            match result {
                Ok(()) => {
                    let mut prefs = load_preferences();
                    prefs.endpoint = self.endpoint.trim().to_owned();
                    prefs.username = self.username.trim().to_owned();
                    self.message =
                        match save_password(&prefs.endpoint, &prefs.username, &self.password)
                            .and_then(|_| save_preferences(&prefs).map_err(|e| e.to_string()))
                        {
                            Ok(()) => match register_autostart().and_then(|_| launch_daemon()) {
                                Ok(()) => {
                                    self.message_until =
                                        Some(Instant::now() + Duration::from_secs(4));
                                    "密码已保存".into()
                                }
                                Err(e) => format!("后台启动失败：{e}"),
                            },
                            Err(e) => format!("凭据保存失败：{e}"),
                        };
                }
                Err(e) => self.message = format!("认证失败：{e}"),
            }
        }
        if self
            .message_until
            .is_some_and(|until| Instant::now() >= until)
        {
            self.message.clear();
            self.message_until = None;
        }
        if self.verification.is_some() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin::same(12)))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                let detail = status::read();
                let (connection_text, connection_color) = connection_indicator(&detail);
                ui.add_enabled_ui(self.verification.is_none(), |ui| {
                    ui.label("服务器地址（HTTPS）");
                    if ui
                        .add_sized(
                            [ui.available_width(), 28.0],
                            egui::TextEdit::singleline(&mut self.endpoint)
                                .hint_text("https://sync.example.com"),
                        )
                        .changed()
                    {
                        self.password.clear();
                    }
                    ui.label("同步账号");
                    if ui
                        .add_sized(
                            [ui.available_width(), 28.0],
                            egui::TextEdit::singleline(&mut self.username)
                                .hint_text("输入自己的账号"),
                        )
                        .changed()
                    {
                        self.password.clear();
                    }
                });
                ui.label(egui::RichText::new("同步密码").size(13.0));
                ui.add_enabled_ui(self.verification.is_none(), |ui| {
                    ui.horizontal(|ui| {
                        let width = (ui.available_width() - 56.0).max(100.0);
                        let input = ui.add_sized(
                            [width, 30.0],
                            egui::TextEdit::singleline(&mut self.password)
                                .password(!self.plain)
                                .hint_text("请输入密码"),
                        );
                        if input.changed() {
                            self.message.clear();
                            self.message_until = None;
                        }
                        if ui
                            .add_sized(
                                [48.0, 30.0],
                                egui::Button::new(if self.plain { "隐藏" } else { "显示" }),
                            )
                            .clicked()
                        {
                            self.plain = !self.plain;
                        }
                    });
                });
                ui.horizontal(|ui| {
                    let (rect, dot) =
                        ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(rect.center(), 4.0, connection_color);
                    dot.on_hover_text(detail.as_str());
                    ui.label(
                        egui::RichText::new(connection_text)
                            .size(12.0)
                            .color(connection_color),
                    )
                    .on_hover_text(detail.as_str());
                    let button_text = if self.verification.is_some() {
                        "验证中…"
                    } else if self.message == "密码已保存" {
                        "已保存"
                    } else if self.message == "密码不能为空" {
                        "请输入密码"
                    } else if self.message.contains("401") {
                        "密码错误 · 重试"
                    } else if self.message.contains("网络") {
                        "网络异常 · 重试"
                    } else if !self.message.is_empty() {
                        "保存失败 · 重试"
                    } else {
                        "保存"
                    };
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let button = ui.add_enabled(
                            self.verification.is_none(),
                            egui::Button::new(button_text).min_size(egui::vec2(120.0, 28.0)),
                        );
                        let button = if self.message.is_empty() {
                            button
                        } else {
                            button.on_hover_text(self.message.as_str())
                        };
                        if button.clicked() {
                            if self.endpoint.trim().is_empty()
                                || self.username.trim().is_empty()
                                || self.password.is_empty()
                            {
                                self.message = "服务器、账号和密码不能为空".into();
                            } else {
                                let endpoint = self.endpoint.trim().to_owned();
                                let username = self.username.trim().to_owned();
                                let pass = self.password.clone();
                                let (tx, rx) = mpsc::channel();
                                self.verification = Some(rx);
                                self.message = "正在验证密码".into();
                                self.message_until = None;
                                std::thread::spawn(move || {
                                    let result = ProtocolClient::new(&endpoint, &username, &pass)
                                        .and_then(|client| client.latest())
                                        .map(|_| ())
                                        .map_err(|e| match e {
                                            ClientError::Authentication => {
                                                "服务器返回 401（密码错误或账户不匹配）".to_owned()
                                            }
                                            ClientError::Network => "网络连接失败".to_owned(),
                                            other => format!("{other:?}"),
                                        });
                                    let _ = tx.send(result);
                                });
                            }
                        }
                    });
                });
            });
    }
}
fn app_icon() -> egui::IconData {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!(
        "../assets/hamokisync-green.png"
    )));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let Ok(mut reader) = decoder.read_info() else {
        return egui::IconData::default();
    };
    let mut buf = vec![0; reader.output_buffer_size()];
    let Ok(info) = reader.next_frame(&mut buf) else {
        return egui::IconData::default();
    };
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()]
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        _ => return egui::IconData::default(),
    };
    egui::IconData {
        rgba,
        width: info.width,
        height: info.height,
    }
}

fn main() -> eframe::Result<()> {
    #[cfg(windows)]
    let Some(_instance) = single_window::acquire() else {
        return Ok(());
    };
    eframe::run_native(
        "HamokiSync",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([380.0, 270.0])
                .with_icon(std::sync::Arc::new(app_icon()))
                .with_resizable(false),
            ..Default::default()
        },
        Box::new(|cc| {
            install_system_font(&cc.egui_ctx);
            Ok(Box::new(Settings::new()))
        }),
    )
}
