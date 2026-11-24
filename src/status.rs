use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Default)]
pub struct RuntimeStatus {
    pub code: String,
    pub updated: u64,
}

fn status_file() -> Option<PathBuf> {
    ProjectDirs::from("io", "Hamoki", "HamokiSync")
        .map(|d| d.config_dir().join("connection-status.json"))
}
pub fn write(code: &str) {
    let Some(path) = status_file() else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let status = RuntimeStatus {
        code: code.to_string(),
        updated: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };
    if let Ok(data) = serde_json::to_vec(&status) {
        let _ = fs::write(path, data);
    }
}
pub fn read() -> String {
    let Some(path) = status_file() else {
        return "未知状态".into();
    };
    let Ok(data) = fs::read(path) else {
        return "等待连接状态".into();
    };
    let Ok(status) = serde_json::from_slice::<RuntimeStatus>(&data) else {
        return "等待连接状态".into();
    };
    if status.updated + 40
        < SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    {
        return "后台状态未知（可能已退出）".into();
    }
    match status.code.as_str() {
        "connected" => "已连接，正在同步".into(),
        "needs_password" => "尚未配置同步密码".into(),
        "authentication" => "密码或账号认证失败（服务器返回 401）".into(),
        "network" => "网络连接失败，正在重试".into(),
        "clipboard" => "系统剪贴板不可用，正在重试".into(),
        "upload_paused" => "上传结果不确定，重新建立同步基线".into(),
        "connecting" => "正在连接同步服务器".into(),
        "stopped" => "同步后台已退出".into(),
        other => format!("后台状态：{other}"),
    }
}
