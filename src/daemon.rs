#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
#[path = "runtime.rs"]
mod runtime;
#[path = "status.rs"]
mod status;
#[cfg(any(windows, target_os = "linux"))]
mod tray;

use arboard::Clipboard;
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use hamokisync_lite::{
    history::{HistoryItem, ItemData},
    policy::{Decision, Identity, SyncPolicy},
    protocol::{Snapshot, SnapshotKind},
};
use runtime::{capture_clipboard, decode_rgba, load_preferences, read_saved_password, Preferences};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
fn pump_windows_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
#[cfg(not(windows))]
fn pump_windows_messages() {}

#[cfg(any(windows, target_os = "linux"))]
fn launch_tray() {
    std::thread::spawn(|| {
        let Ok(tray) = tray::Tray::new() else { return };
        let mut last_status = String::new();
        let mut last_check = Instant::now() - Duration::from_secs(1);
        loop {
            pump_windows_messages();
            if last_check.elapsed() >= Duration::from_secs(1) {
                let current = status::read();
                if current != last_status && tray.refresh_status(&current).is_ok() {
                    last_status = current;
                }
                last_check = Instant::now();
            }
            if let Some(event) = tray.next_action() {
                match event {
                    tray::TrayAction::Open => {
                        if let Ok(exe) = std::env::current_exe() {
                            let _ =
                                std::process::Command::new(exe.with_file_name(if cfg!(windows) {
                                    "hamokisync-settings.exe"
                                } else {
                                    "hamokisync-settings"
                                }))
                                .spawn();
                        }
                    }
                    tray::TrayAction::Exit => {
                        status::write("stopped");
                        std::process::exit(0);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(70));
        }
    });
}
#[cfg(not(any(windows, target_os = "linux")))]
fn launch_tray() {}

#[cfg(unix)]
fn single_instance() -> bool {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }
    let lock_path = std::env::temp_dir().join(format!(
        "hamokisync-daemon-{}.lock",
        std::env::var("USER").unwrap_or_default()
    ));
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(lock_path)
    else {
        return false;
    };
    // Kernel releases the advisory lock when the process exits. Retain FD.
    if unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
        return false;
    }
    std::mem::forget(file);
    true
}

#[cfg(windows)]
fn single_instance() -> bool {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateMutexW(
            attributes: *const c_void,
            initial_owner: i32,
            name: *const u16,
        ) -> *mut c_void;
        fn GetLastError() -> u32;
    }
    let name: Vec<u16> = r"Local\HamokiSyncDaemon"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return false;
    }
    let already_exists = unsafe { GetLastError() } == 183;
    // Retain the process-owned HANDLE until exit.
    !already_exists
}

#[cfg(windows)]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetClipboardSequenceNumber() -> u32;
}

#[cfg(windows)]
fn clipboard_sequence() -> Option<u32> {
    let sequence = unsafe { GetClipboardSequenceNumber() };
    (sequence != 0).then_some(sequence)
}

#[cfg(not(windows))]
fn clipboard_sequence() -> Option<u32> {
    None
}

fn identity(item: &HistoryItem) -> Option<Identity> {
    let (kind, hash) = item.fingerprint.split_once(':')?;
    Some(Identity::new(
        if kind == "Image" { "Image" } else { "Text" },
        hash,
    ))
}
fn remote_id(s: &Snapshot) -> Identity {
    Identity::new(
        match s.kind {
            SnapshotKind::Text => "Text",
            SnapshotKind::Image => "Image",
            SnapshotKind::File => "File",
            SnapshotKind::Group => "Group",
        },
        &s.hash,
    )
}
fn put_clipboard(cb: &mut Clipboard, item: &HistoryItem) -> bool {
    match &item.data {
        ItemData::Text(t) => cb.set_text(t.clone()).is_ok(),
        ItemData::Png(b) => decode_rgba(b).is_some_and(|i| cb.set_image(i).is_ok()),
    }
}
fn remote_item(
    client: &hamokisync_lite::protocol::ProtocolClient,
    s: &Snapshot,
) -> Option<HistoryItem> {
    let data = match s.kind {
        SnapshotKind::Text => {
            let text = if s.has_data {
                String::from_utf8(client.download(s).ok()?).ok()?
            } else {
                s.text.clone()
            };
            if text.is_empty() {
                return None;
            }
            ItemData::Text(text)
        }
        SnapshotKind::Image => {
            let bytes = client.download(s).ok()?;
            decode_rgba(&bytes)?;
            ItemData::Png(bytes)
        }
        _ => return None,
    };
    Some(HistoryItem {
        fingerprint: s.identity(),
        data,
    })
}
fn run(mut clipboard: Clipboard, prefs: Preferences, password: String) -> Result<(), String> {
    status::write("connecting");
    let client =
        hamokisync_lite::protocol::ProtocolClient::new(&prefs.endpoint, &prefs.username, &password)
            .map_err(|e| e.to_string())?;
    let initial = client.latest().map_err(|e| {
        status::write(
            if e == hamokisync_lite::protocol::ClientError::Authentication {
                "authentication"
            } else {
                "network"
            },
        );
        e.to_string()
    })?;
    status::write("connected");
    let mut policy = SyncPolicy::new();
    policy.start(true); // User has explicitly installed and configured the background sync.
    policy.see_remote(remote_id(&initial), policy.revision());
    let mut previous: Option<HistoryItem> = None;
    let mut current = capture_clipboard(&mut clipboard);
    let mut last_id = current.as_ref().map(|x| x.fingerprint.clone());
    let mut last_seq = clipboard_sequence();
    let mut next_fallback_capture = Instant::now();
    let mut last_remote = Instant::now();
    let mut last_credential_check = Instant::now();
    let mut last_status_heartbeat = Instant::now();
    let mut network_ok = true;
    let mut write_echo_until = Instant::now();
    let hotkey: Option<HotKey> = "Ctrl+Alt+Z".parse().ok();
    let manager = GlobalHotKeyManager::new().ok();
    let hotkey_id = match (&manager, hotkey) {
        (Some(manager), Some(key)) if manager.register(key).is_ok() => Some(key.id),
        _ => None,
    };
    loop {
        pump_windows_messages();
        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state != HotKeyState::Pressed || hotkey_id != Some(ev.id) {
                continue;
            }
            if let Some(item) = previous.clone() {
                if put_clipboard(&mut clipboard, &item) {
                    previous = current.replace(item.clone());
                    last_id = capture_clipboard(&mut clipboard).map(|x| x.fingerprint);
                    last_seq = clipboard_sequence();
                    write_echo_until = Instant::now() + Duration::from_millis(750);
                    policy.copied(None); // Local restore must not publish.
                }
            }
        }
        let sequence = clipboard_sequence();
        let changed = if let Some(seq) = sequence {
            if Some(seq) != last_seq {
                last_seq = Some(seq);
                true
            } else {
                false
            }
        } else if Instant::now() >= next_fallback_capture {
            next_fallback_capture = Instant::now() + Duration::from_millis(500);
            true
        } else {
            false
        };
        if changed {
            let captured = capture_clipboard(&mut clipboard);
            let candidate_id = captured.as_ref().map(|x| x.fingerprint.clone());
            if candidate_id != last_id {
                last_id = candidate_id;
                if Instant::now() >= write_echo_until {
                    if let Some(item) = captured.clone() {
                        if let Some(id) = identity(&item) {
                            let decision = policy.copied(Some(id.clone()));
                            previous = current.take();
                            current = Some(item.clone());
                            if decision == Decision::Publish {
                                let result = match &item.data {
                                    ItemData::Text(t) => client.publish_text(t),
                                    ItemData::Png(b) => client.publish_png(b),
                                };
                                if result.is_err() {
                                    policy.finish_publish(&id, false);
                                    status::write("upload_paused");
                                    return Err(
                                    "Upload uncertain; stopped to avoid overwriting newer copies"
                                        .into(),
                                );
                                }
                                policy.finish_publish(&id, true);
                            }
                        }
                    } else {
                        policy.copied(None);
                    }
                }
            }
        }
        if last_remote.elapsed() >= Duration::from_millis(1500) {
            let revision = policy.revision();
            match client.latest() {
                Ok(snapshot) => {
                    if !network_ok {
                        network_ok = true;
                        status::write("connected");
                    }
                    if policy.see_remote(remote_id(&snapshot), revision) == Decision::Fetch {
                        if let Some(item) = remote_item(&client, &snapshot) {
                            if policy.can_apply_remote(revision)
                                && put_clipboard(&mut clipboard, &item)
                            {
                                policy.applied_remote(remote_id(&snapshot), revision);
                                previous = current.replace(item);
                                last_id = capture_clipboard(&mut clipboard).map(|x| x.fingerprint);
                                last_seq = clipboard_sequence();
                                write_echo_until = Instant::now() + Duration::from_millis(750);
                            }
                        }
                    }
                }
                Err(error) => {
                    if network_ok {
                        network_ok = false;
                        status::write(
                            if error == hamokisync_lite::protocol::ClientError::Authentication {
                                "authentication"
                            } else {
                                "network"
                            },
                        );
                    }
                }
            }
            last_remote = Instant::now();
        }
        if last_credential_check.elapsed() >= Duration::from_secs(8) {
            let current = load_preferences();
            if current.endpoint != prefs.endpoint
                || current.username != prefs.username
                || read_saved_password(&prefs.endpoint, &prefs.username)
                    .ok()
                    .as_deref()
                    != Some(password.as_str())
            {
                return Err("Configuration changed".into());
            }
            last_credential_check = Instant::now();
        }
        if last_status_heartbeat.elapsed() >= Duration::from_secs(25) {
            status::write(if network_ok { "connected" } else { "network" });
            last_status_heartbeat = Instant::now();
        }
        thread::sleep(Duration::from_millis(180));
    }
}

fn main() {
    if !single_instance() {
        return;
    }
    // The tray must stay available even with no password or network.
    launch_tray();
    loop {
        pump_windows_messages();
        let prefs = load_preferences();
        let password = read_saved_password(&prefs.endpoint, &prefs.username).unwrap_or_default();
        if prefs.endpoint.is_empty() || prefs.username.is_empty() || password.is_empty() {
            status::write("needs_password");
            thread::sleep(Duration::from_secs(5));
            continue;
        }
        if let Ok(clipboard) = Clipboard::new() {
            // Errors never leak password or clipboard contents into logs.
            let _ = run(clipboard, prefs, password);
        } else {
            status::write("clipboard");
        }
        // A fresh server baseline avoids blindly re-uploading old content.
        thread::sleep(Duration::from_secs(5));
    }
}
