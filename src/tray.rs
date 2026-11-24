use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

pub enum TrayAction {
    Open,
    Exit,
}

pub struct Tray {
    _handle: TrayIcon,
    open_id: MenuId,
    exit_id: MenuId,
}

impl Tray {
    pub fn new() -> Result<Self, String> {
        let menu = Menu::new();
        let open = MenuItem::new("打开 HamokiSync 设置", true, None);
        let exit = MenuItem::new("退出（停止同步）", true, None);
        menu.append(&open).map_err(|e| e.to_string())?;
        menu.append(&exit).map_err(|e| e.to_string())?;
        let icon = sync_icon([27, 158, 114])?;
        let handle = TrayIconBuilder::new()
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_tooltip("HamokiSync — 文字与图片同步")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            _handle: handle,
            open_id: open.id().clone(),
            exit_id: exit.id().clone(),
        })
    }

    pub fn refresh_status(&self, label: &str) -> Result<(), String> {
        let color = if label.starts_with("已连接") {
            [27, 158, 114]
        } else if label.contains("失败") || label.contains("不可用") {
            [220, 67, 72]
        } else {
            [119, 130, 142]
        };
        self._handle
            .set_icon(Some(sync_icon(color)?))
            .map_err(|e| e.to_string())?;
        self._handle
            .set_tooltip(Some(format!("HamokiSync — {label}")))
            .map_err(|e| e.to_string())
    }

    pub fn next_action(&self) -> Option<TrayAction> {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.open_id {
                return Some(TrayAction::Open);
            }
            if event.id == self.exit_id {
                return Some(TrayAction::Exit);
            }
        }
        if let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if matches!(
                event,
                TrayIconEvent::Click { .. } | TrayIconEvent::DoubleClick { .. }
            ) {
                return Some(TrayAction::Open);
            }
        }
        None
    }
}

// Supersampled rounded-square badge and two clockwise arrows. Native RGBA,
// no image files, dependencies, background tasks or network access required.
fn sync_icon(color: [u8; 3]) -> Result<Icon, String> {
    const N: usize = 32;
    const SS: usize = 4;
    let mut pixels = vec![0u8; N * N * 4];
    let arrow = |x: f64, y: f64| {
        let theta = y.atan2(x).to_degrees().rem_euclid(360.0);
        let radius = x.hypot(y);
        // Rounded arc stem, directed clockwise.
        let r = 6.65;
        let a0 = 192.0_f64.to_radians();
        let a1 = 318.0_f64.to_radians();
        let start = (r * a0.cos(), r * a0.sin());
        let end = (r * a1.cos(), r * a1.sin());
        let shaft = ((192.0..=318.0).contains(&theta) && (radius - r).abs() <= 1.35)
            || (x - start.0).hypot(y - start.1) <= 1.35
            || (x - end.0).hypot(y - end.1) <= 1.35;
        let dx = x - end.0;
        let dy = y - end.1;
        let along = dx * -a1.sin() + dy * a1.cos();
        let across = dx * a1.cos() + dy * a1.sin();
        let head = (-1.7..=3.25).contains(&along) && across.abs() <= 3.05 * (3.25 - along) / 4.95;
        shaft || head
    };
    for y in 0..N {
        for x in 0..N {
            let (mut covered, mut white) = (0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = ((x * SS + sx) as f64 + 0.5) * 24.0 / (N * SS) as f64 - 12.0;
                    let py = ((y * SS + sy) as f64 + 0.5) * 24.0 / (N * SS) as f64 - 12.0;
                    // 22 x 22 rounded square, 5 unit corner radius.
                    let qx = (px.abs() - 6.0).max(0.0);
                    let qy = (py.abs() - 6.0).max(0.0);
                    if qx.hypot(qy) <= 5.0 {
                        covered += 1;
                        if arrow(px, py) || arrow(-px, -py) {
                            white += 1;
                        }
                    }
                }
            }
            if covered > 0 {
                let i = (y * N + x) * 4;
                for c in 0..3 {
                    pixels[i + c] =
                        ((color[c] as u32 * (covered - white) + 255 * white + covered / 2)
                            / covered) as u8;
                }
                pixels[i + 3] = ((255 * covered + (SS * SS / 2) as u32) / (SS * SS) as u32) as u8;
            }
        }
    }
    Icon::from_rgba(pixels, N as u32, N as u32).map_err(|e| e.to_string())
}
