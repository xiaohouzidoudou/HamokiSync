use arboard::{Clipboard, ImageData};
use directories::ProjectDirs;
use hamokisync_lite::{
    history::{HistoryItem, ItemData},
    protocol::{
        digest_hex, image_profile_hash, validate_png, ClientError, ProtocolClient, Snapshot,
        MAX_IMAGE_BYTES, MAX_IMAGE_PIXELS, MAX_IMAGE_SIDE, MAX_TEXT_BYTES,
    },
};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;

pub enum NetCmd {
    Connect {
        endpoint: String,
        user: String,
        password: String,
    },
    Poll {
        revision: u64,
    },
    Upload(HistoryItem),
    Download {
        snapshot: Snapshot,
        revision: u64,
    },
    Stop,
    Quit,
}

pub enum NetEvent {
    Connected(Result<Snapshot, ClientError>),
    Polled {
        revision: u64,
        result: Result<Snapshot, ClientError>,
    },
    Published {
        identity: String,
        result: Result<String, ClientError>,
    },
    Downloaded {
        snapshot: Snapshot,
        revision: u64,
        result: Result<Vec<u8>, ClientError>,
    },
}

pub fn start_network_actor(events: Sender<NetEvent>) -> Sender<NetCmd> {
    let (sender, commands) = mpsc::channel();
    thread::Builder::new()
        .name("hamoki-network".to_owned())
        .spawn(move || {
            let mut client: Option<ProtocolClient> = None;
            while let Ok(cmd) = commands.recv() {
                match cmd {
                    NetCmd::Connect {
                        endpoint,
                        user,
                        password,
                    } => {
                        client = None;
                        let ready =
                            ProtocolClient::new(&endpoint, &user, &password).and_then(|c| {
                                let result = c.latest()?;
                                client = Some(c);
                                Ok(result)
                            });
                        let _ = events.send(NetEvent::Connected(ready));
                    }
                    NetCmd::Poll { revision } => {
                        if let Some(client) = &client {
                            let _ = events.send(NetEvent::Polled {
                                revision,
                                result: client.latest(),
                            });
                        }
                    }
                    NetCmd::Upload(item) => {
                        if let Some(active_client) = &client {
                            let identity = item.fingerprint;
                            let result = match item.data {
                                ItemData::Text(text) => active_client.publish_text(&text),
                                ItemData::Png(bytes) => active_client.publish_png(&bytes),
                            };
                            // If a PUT failed, it may have reached the
                            // server. Stop before processing queued uploads:
                            // never overwrite a newer remote copy blindly.
                            if result.is_err() {
                                client = None;
                            }
                            let _ = events.send(NetEvent::Published { identity, result });
                        }
                    }
                    NetCmd::Download { snapshot, revision } => {
                        if let Some(client) = &client {
                            let result = client.download(&snapshot);
                            let _ = events.send(NetEvent::Downloaded {
                                snapshot,
                                revision,
                                result,
                            });
                        }
                    }
                    NetCmd::Stop => client = None,
                    NetCmd::Quit => break,
                }
            }
        })
        .expect("Failed to start local network worker");
    sender
}

/// Do not run this except after explicit user consent + Start.
/// FLTK's OS clipboard notification avoids polling/reading idle clipboard.
pub fn capture_clipboard(clipboard: &mut Clipboard) -> Option<HistoryItem> {
    if let Ok(image) = clipboard.get_image() {
        let width = image.width;
        let height = image.height;
        let area = width.checked_mul(height)?;
        if width == 0
            || height == 0
            || width > MAX_IMAGE_SIDE as usize
            || height > MAX_IMAGE_SIDE as usize
            || area > MAX_IMAGE_PIXELS as usize
            || image.bytes.len() != area.checked_mul(4)?
        {
            return None;
        }
        let png = encode_rgba_to_png(width as u32, height as u32, &image.bytes)?;
        let identity = image_profile_hash(&png);
        return Some(HistoryItem {
            fingerprint: format!("Image:{identity}"),
            data: ItemData::Png(png),
        });
    }
    let text = clipboard.get_text().ok()?;
    if text.is_empty() || text.len() > MAX_TEXT_BYTES {
        return None;
    }
    // Some file managers expose file:// URI text as a fallback for copied
    // files/folders. The sync product must not send files as text.
    if text.starts_with("file://")
        || text.starts_with("x-special/")
        || text.contains("\0")
        || text.starts_with("copy\nfile://")
        || text.starts_with("cut\nfile://")
    {
        return None;
    }
    let hash = digest_hex(text.as_bytes());
    Some(HistoryItem {
        fingerprint: format!("Text:{hash}"),
        data: ItemData::Text(text),
    })
}

fn encode_rgba_to_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    if bytes.len() > MAX_IMAGE_BYTES || validate_png(&bytes).is_err() {
        return None;
    }
    Some(bytes)
}

/// Arbitrary remote PNGs can be paletted/RGB/grayscale. Always normalize
/// before setting OS clipboard; never pass an unbounded decompression buffer.
pub fn decode_rgba(data: &[u8]) -> Option<ImageData<'static>> {
    validate_png(data).ok()?;
    let mut decoder = png::Decoder::new(Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let width = reader.info().width as usize;
    let height = reader.info().height as usize;
    let bytes_needed = reader.output_buffer_size();
    if width.checked_mul(height)? > MAX_IMAGE_PIXELS as usize || bytes_needed > 80 * 1024 * 1024 {
        return None;
    }
    let mut buffer = vec![0; bytes_needed];
    let frame = reader.next_frame(&mut buffer).ok()?;
    let input = &buffer[..frame.buffer_size()];
    let mut rgba = Vec::with_capacity(width.checked_mul(height)?.checked_mul(4)?);
    match frame.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(input),
        png::ColorType::Rgb => {
            for px in input.chunks_exact(3) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in input {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for px in input.chunks_exact(2) {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        png::ColorType::Indexed => return None,
    }
    if rgba.len() != width * height * 4 {
        return None;
    }
    Some(ImageData {
        width,
        height,
        bytes: Cow::Owned(rgba),
    })
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Preferences {
    pub endpoint: String,
    pub username: String,
    pub remember: bool,
    pub latest_hotkey: String,
    pub previous_hotkey: String,
}

impl Default for Preferences {
    fn default() -> Self {
        let modifier = if cfg!(target_os = "macos") {
            "Meta+Alt+Shift"
        } else {
            "Ctrl+Alt+Shift"
        };
        Self {
            endpoint: String::new(),
            username: String::new(),
            remember: true,
            latest_hotkey: format!("{modifier}+1"),
            previous_hotkey: format!("{modifier}+2"),
        }
    }
}

fn preferences_path() -> Option<PathBuf> {
    ProjectDirs::from("io", "Hamoki", "HamokiSync").map(|x| x.config_dir().join("settings.json"))
}

pub fn load_preferences() -> Preferences {
    preferences_path()
        .and_then(|x| fs::read(x).ok())
        .and_then(|x| serde_json::from_slice(&x).ok())
        .unwrap_or_default()
}

pub fn save_preferences(prefs: &Preferences) -> std::io::Result<()> {
    let path =
        preferences_path().ok_or_else(|| std::io::Error::other("no user config directory"))?;
    let folder = path.parent().expect("settings has parent");
    fs::create_dir_all(folder)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(folder, fs::Permissions::from_mode(0o700))?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(prefs)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(&path)?;
    }
    fs::rename(tmp, path)
}

fn credential_account(endpoint: &str, user: &str) -> String {
    digest_hex(format!("{endpoint}\n{user}").as_bytes())[..32].to_owned()
}

pub fn read_saved_password(endpoint: &str, user: &str) -> Result<String, String> {
    let entry = keyring::Entry::new("HamokiSync", &credential_account(endpoint, user))
        .map_err(|e| e.to_string())?;
    entry.get_password().map_err(|e| e.to_string())
}

pub fn save_password(endpoint: &str, user: &str, pass: &str) -> Result<(), String> {
    let entry = keyring::Entry::new("HamokiSync", &credential_account(endpoint, user))
        .map_err(|e| e.to_string())?;
    entry.set_password(pass).map_err(|e| e.to_string())
}

pub fn erase_password(endpoint: &str, user: &str) -> Result<(), String> {
    let entry = keyring::Entry::new("HamokiSync", &credential_account(endpoint, user))
        .map_err(|e| e.to_string())?;
    entry.delete_credential().map_err(|e| e.to_string())
}
