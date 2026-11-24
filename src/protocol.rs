use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{Cursor, Read};
use std::time::Duration;
use url::Url;

pub const MAX_JSON: u64 = 256 * 1024;
pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;
pub const MAX_IMAGE_SIDE: u32 = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    InvalidEndpoint,
    InvalidInput,
    InvalidMetadata,
    TooLarge,
    Authentication,
    UnexpectedStatus(u16),
    Network,
    AmbiguousWrite,
    IntegrityMismatch,
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ClientError {}

pub fn digest_hex(bytes: &[u8]) -> String {
    format!("{:X}", Sha256::digest(bytes))
}

pub fn image_profile_name(bytes: &[u8]) -> String {
    let digest = digest_hex(bytes);
    format!("Image_{}.png", &digest[..32])
}

pub fn image_profile_hash(bytes: &[u8]) -> String {
    let hash = digest_hex(bytes);
    digest_hex(format!("{}|{hash}", image_profile_name(bytes)).as_bytes())
}

fn valid_digest(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_filename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 240
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\'])
        && !value.chars().any(|c| c.is_control())
}

fn parse_endpoint(input: &str) -> Result<Url, ClientError> {
    let mut url = Url::parse(input).map_err(|_| ClientError::InvalidEndpoint)?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(ClientError::InvalidEndpoint);
    }
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        return Err(ClientError::InvalidEndpoint);
    }
    url.set_path("/");
    Ok(url)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotKind {
    Text,
    Image,
    File,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub kind: SnapshotKind,
    pub hash: String,
    pub text: String,
    pub has_data: bool,
    pub data_name: Option<String>,
    pub transfer_hash: Option<String>,
    pub size: Option<usize>,
}

impl Snapshot {
    pub fn identity(&self) -> String {
        let name = match self.kind {
            SnapshotKind::Text => "Text",
            SnapshotKind::Image => "Image",
            SnapshotKind::File => "File",
            SnapshotKind::Group => "Group",
        };
        format!("{name}:{}", self.hash.to_uppercase())
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, ClientError> {
        if data.len() > MAX_JSON as usize {
            return Err(ClientError::TooLarge);
        }
        let value: Value =
            serde_json::from_slice(data).map_err(|_| ClientError::InvalidMetadata)?;
        let obj = value.as_object().ok_or(ClientError::InvalidMetadata)?;
        let kind = match obj.get("type").and_then(Value::as_str) {
            Some("Text") => SnapshotKind::Text,
            Some("Image") => SnapshotKind::Image,
            Some("File") => SnapshotKind::File,
            Some("Group") => SnapshotKind::Group,
            _ => return Err(ClientError::InvalidMetadata),
        };
        let text = obj
            .get("text")
            .and_then(Value::as_str)
            .ok_or(ClientError::InvalidMetadata)?
            .to_owned();
        let has_data = obj
            .get("hasData")
            .and_then(Value::as_bool)
            .ok_or(ClientError::InvalidMetadata)?;
        let hash = match obj.get("hash") {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(v)) => v.clone(),
            _ => return Err(ClientError::InvalidMetadata),
        };
        let data_name = match obj.get("dataName") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if valid_filename(s) => Some(s.clone()),
            _ => return Err(ClientError::InvalidMetadata),
        };
        let transfer_hash = match obj.get("transferDataHash") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if valid_digest(s) => Some(s.clone()),
            _ => return Err(ClientError::InvalidMetadata),
        };
        let size = match obj.get("size") {
            None | Some(Value::Null) => None,
            Some(Value::Number(n)) => Some(
                usize::try_from(n.as_u64().ok_or(ClientError::InvalidMetadata)?)
                    .map_err(|_| ClientError::TooLarge)?,
            ),
            _ => return Err(ClientError::InvalidMetadata),
        };
        if (has_data && data_name.is_none()) || (!has_data && kind != SnapshotKind::Text) {
            return Err(ClientError::InvalidMetadata);
        }
        if !hash.is_empty() && !valid_digest(&hash) {
            return Err(ClientError::InvalidMetadata);
        }
        match kind {
            SnapshotKind::Text if !has_data => {
                if text.len() > MAX_TEXT_BYTES
                    || (!hash.is_empty()
                        && !hash.eq_ignore_ascii_case(&digest_hex(text.as_bytes())))
                {
                    return Err(ClientError::IntegrityMismatch);
                }
            }
            SnapshotKind::Text => {
                if !valid_digest(&hash)
                    || text.encode_utf16().count() != 10240
                    || size
                        .filter(|s| *s > 10240 && *s <= MAX_TEXT_BYTES)
                        .is_none()
                    || !data_name.as_deref().unwrap_or_default().ends_with(".txt")
                    || transfer_hash
                        .as_deref()
                        .map(|v| v.eq_ignore_ascii_case(&hash))
                        != Some(true)
                {
                    return Err(ClientError::InvalidMetadata);
                }
            }
            SnapshotKind::Image => {
                if !valid_digest(&hash)
                    || !data_name
                        .as_deref()
                        .unwrap_or_default()
                        .starts_with("Image_")
                    || !data_name.as_deref().unwrap_or_default().ends_with(".png")
                    || size.is_some_and(|s| s > MAX_IMAGE_BYTES)
                {
                    return Err(ClientError::InvalidMetadata);
                }
            }
            _ => {} // Files and Group profiles are never applied to OS clipboard.
        }
        Ok(Self {
            kind,
            hash,
            text,
            has_data,
            data_name,
            transfer_hash,
            size,
        })
    }
}

/// Only HTTPS remote URLs and loopback HTTP are accepted. Never log secrets.
/// Every request rejects redirects and limits response body size.
pub struct ProtocolClient {
    endpoint: Url,
    basic_authorization: String,
    http: ureq::Agent,
}

impl ProtocolClient {
    pub fn new(endpoint: &str, user: &str, password: &str) -> Result<Self, ClientError> {
        let endpoint = parse_endpoint(endpoint)?;
        if user.is_empty()
            || user.contains(':')
            || user.contains('\n')
            || password.is_empty()
            || password.contains('\0')
        {
            return Err(ClientError::InvalidInput);
        }
        let basic_authorization =
            format!("Basic {}", STANDARD.encode(format!("{user}:{password}")));
        let http = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(60))
            .build();
        Ok(Self {
            endpoint,
            basic_authorization,
            http,
        })
    }

    fn url(&self, path: &str) -> String {
        self.endpoint
            .join(path)
            .expect("constant validated path")
            .to_string()
    }

    fn read_bounded(&self, path: &str, limit: u64) -> Result<Vec<u8>, ClientError> {
        let response = self
            .http
            .get(&self.url(path))
            .set("Authorization", &self.basic_authorization)
            .call()
            .map_err(request_error)?;
        let mut body = Vec::new();
        response
            .into_reader()
            .take(limit + 1)
            .read_to_end(&mut body)
            .map_err(|_| ClientError::Network)?;
        if body.len() as u64 > limit {
            return Err(ClientError::TooLarge);
        }
        Ok(body)
    }

    fn put(&self, path: &str, mime: &str, content: &[u8]) -> Result<(), ClientError> {
        let response = self
            .http
            .put(&self.url(path))
            .set("Content-Type", mime)
            .set("Authorization", &self.basic_authorization)
            .send_bytes(content)
            .map_err(|error| match error {
                ureq::Error::Transport(_) => ClientError::AmbiguousWrite,
                e => request_error(e),
            })?;
        match response.status() {
            200 | 204 => Ok(()),
            status => Err(ClientError::UnexpectedStatus(status)),
        }
    }

    pub fn latest(&self) -> Result<Snapshot, ClientError> {
        Snapshot::from_bytes(&self.read_bounded("SyncClipboard.json", MAX_JSON)?)
    }

    /// Official inline TextProfile. For longer UTF-16 text the official
    /// two-PUT upload is used instead of silently truncating the content.
    pub fn publish_text(&self, text: &str) -> Result<String, ClientError> {
        if text.is_empty() || text.len() > MAX_TEXT_BYTES {
            return Err(ClientError::TooLarge);
        }
        let hash = digest_hex(text.as_bytes());
        let units = text.encode_utf16().count();
        let metadata = if units <= 10240 {
            json!({"type":"Text","hash":hash,"text":text,"hasData":false})
        } else {
            let filename = format!("Text_{}.txt", &hash[..32]);
            let mut prefix = String::new();
            let mut prefix_units = 0usize;
            for ch in text.chars() {
                if prefix_units + ch.len_utf16() > 10240 {
                    break;
                }
                prefix.push(ch);
                prefix_units += ch.len_utf16();
            }
            if prefix_units != 10240 {
                // UTF16 preview would split surrogate pair; fail closed.
                return Err(ClientError::InvalidInput);
            }
            self.put(
                &format!("file/{filename}"),
                "application/octet-stream",
                text.as_bytes(),
            )?;
            json!({"type":"Text","hash":hash,"text":prefix,"hasData":true,
                   "dataName":filename,"transferDataHash":hash,"size":units})
        };
        self.put(
            "SyncClipboard.json",
            "application/json",
            &serde_json::to_vec(&metadata).map_err(|_| ClientError::InvalidInput)?,
        )?;
        Ok(format!("Text:{hash}"))
    }

    pub fn publish_png(&self, png: &[u8]) -> Result<String, ClientError> {
        validate_png(png)?;
        let sha = digest_hex(png);
        let name = image_profile_name(png);
        let identity = image_profile_hash(png);
        self.put(&format!("file/{name}"), "application/octet-stream", png)?;
        let metadata = json!({"type":"Image","hash":identity,"text":name,
                              "hasData":true,"dataName":name,
                              "transferDataHash":sha,"size":png.len()});
        self.put(
            "SyncClipboard.json",
            "application/json",
            &serde_json::to_vec(&metadata).map_err(|_| ClientError::InvalidInput)?,
        )?;
        Ok(format!("Image:{identity}"))
    }

    pub fn download(&self, snapshot: &Snapshot) -> Result<Vec<u8>, ClientError> {
        if !snapshot.has_data || !matches!(snapshot.kind, SnapshotKind::Text | SnapshotKind::Image)
        {
            return Err(ClientError::InvalidInput);
        }
        let name = snapshot
            .data_name
            .as_deref()
            .filter(|name| valid_filename(name))
            .ok_or(ClientError::InvalidInput)?;
        let limit = if snapshot.kind == SnapshotKind::Image {
            MAX_IMAGE_BYTES
        } else {
            MAX_TEXT_BYTES
        };
        let bytes = self.read_bounded(&format!("file/{name}"), limit as u64)?;
        let sha = digest_hex(&bytes);
        if snapshot
            .transfer_hash
            .as_deref()
            .map(|h| h.eq_ignore_ascii_case(&sha))
            == Some(false)
        {
            return Err(ClientError::IntegrityMismatch);
        }
        if snapshot.kind == SnapshotKind::Text {
            let text = std::str::from_utf8(&bytes).map_err(|_| ClientError::IntegrityMismatch)?;
            if text.encode_utf16().count() != snapshot.size.unwrap_or_default()
                || !text.starts_with(&snapshot.text)
                || !snapshot.hash.eq_ignore_ascii_case(&sha)
            {
                return Err(ClientError::IntegrityMismatch);
            }
        } else {
            if snapshot.size.is_some_and(|len| len != bytes.len())
                || snapshot.text != name
                || !snapshot
                    .hash
                    .eq_ignore_ascii_case(&digest_hex(format!("{name}|{sha}").as_bytes()))
            {
                return Err(ClientError::IntegrityMismatch);
            }
            validate_png(&bytes)?;
        }
        Ok(bytes)
    }
}

fn request_error(error: ureq::Error) -> ClientError {
    match error {
        ureq::Error::Status(401 | 403, _) => ClientError::Authentication,
        ureq::Error::Status(code, _) => ClientError::UnexpectedStatus(code),
        ureq::Error::Transport(_) => ClientError::Network,
    }
}

/// PNG dimensions and decode limit checked BEFORE handing to clipboard.
pub fn validate_png(data: &[u8]) -> Result<(), ClientError> {
    if data.len() > MAX_IMAGE_BYTES {
        return Err(ClientError::TooLarge);
    }
    if data.len() < 24 || &data[..8] != b"\x89PNG\r\n\x1a\n" || &data[12..16] != b"IHDR" {
        return Err(ClientError::InvalidInput);
    }
    let w = u32::from_be_bytes(
        data[16..20]
            .try_into()
            .map_err(|_| ClientError::InvalidInput)?,
    );
    let h = u32::from_be_bytes(
        data[20..24]
            .try_into()
            .map_err(|_| ClientError::InvalidInput)?,
    );
    if w == 0
        || h == 0
        || w > MAX_IMAGE_SIDE
        || h > MAX_IMAGE_SIDE
        || u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS
    {
        return Err(ClientError::TooLarge);
    }
    let mut reader = png::Decoder::new(Cursor::new(data))
        .read_info()
        .map_err(|_| ClientError::IntegrityMismatch)?;
    let decoded = reader.output_buffer_size();
    if decoded > 80 * 1024 * 1024 {
        return Err(ClientError::TooLarge);
    }
    let mut buf = vec![0; decoded];
    reader
        .next_frame(&mut buf)
        .map_err(|_| ClientError::IntegrityMismatch)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_rejects_plain_http_remote_and_traversal() {
        assert!(ProtocolClient::new("http://sync.example.com", "test", "test").is_err());
        assert!(ProtocolClient::new("https://sync.example.com/path", "test", "test").is_err());
        assert!(
            ProtocolClient::new("https://test:secret@sync.example.com", "test", "test").is_err()
        );
        assert!(ProtocolClient::new("http://127.0.0.1:59312", "test", "test").is_ok());
    }
    #[test]
    fn compatible_inline_text_profile() {
        let metadata = json!({"type":"Text","hash":digest_hex(b"test line"),
                    "text":"test line","hasData":false});
        let snapshot = Snapshot::from_bytes(&serde_json::to_vec(&metadata).unwrap()).unwrap();
        assert_eq!(
            snapshot.identity(),
            format!("Text:{}", digest_hex(b"test line"))
        );
    }
    #[test]
    fn refuse_bogus_hash_or_oversized_image() {
        let bogus = br#"{"type":"Text","hash":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","text":"bad","hasData":false}"#;
        assert_eq!(
            Snapshot::from_bytes(bogus),
            Err(ClientError::IntegrityMismatch)
        );
        let fake = vec![0; MAX_IMAGE_BYTES + 1];
        assert_eq!(validate_png(&fake), Err(ClientError::TooLarge));
    }
}
