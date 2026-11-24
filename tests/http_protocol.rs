use hamokisync_lite::protocol::{
    digest_hex, image_profile_hash, ClientError, ProtocolClient, SnapshotKind,
};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

struct Stored {
    profile: Vec<u8>,
    files: HashMap<String, Vec<u8>>,
}

fn read_request(stream: &mut TcpStream) -> (String, String, String, Vec<u8>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = Vec::new();
    let header_end;
    loop {
        let mut buf = [0; 4096];
        let count = stream.read(&mut buf).unwrap();
        assert!(count > 0 && request.len() < 30 * 1024 * 1024);
        request.extend_from_slice(&buf[..count]);
        if let Some(i) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = i + 4;
            break;
        }
    }
    let text = String::from_utf8_lossy(&request[..header_end]);
    let mut lines = text.lines();
    let first = lines.next().unwrap();
    let mut tokens = first.split_whitespace();
    let method = tokens.next().unwrap().to_owned();
    let path = tokens.next().unwrap().to_owned();
    let mut content_length = 0;
    let mut authorization = String::new();
    for line in lines {
        if let Some(v) = line.strip_prefix("Content-Length:") {
            content_length = v.trim().parse::<usize>().unwrap();
        }
        if let Some(v) = line.strip_prefix("Authorization:") {
            authorization = v.trim().to_owned();
        }
        if let Some(v) = line.strip_prefix("authorization:") {
            authorization = v.trim().to_owned();
        }
        if let Some(v) = line.strip_prefix("content-length:") {
            content_length = v.trim().parse::<usize>().unwrap();
        }
    }
    let mut body = request[header_end..].to_vec();
    while body.len() < content_length {
        let mut buf = [0; 8192];
        let n = stream.read(&mut buf).unwrap();
        assert!(n > 0);
        body.extend_from_slice(&buf[..n]);
    }
    body.truncate(content_length);
    (method, path, authorization, body)
}

fn spawn_mock(count: usize) -> (String, Arc<Mutex<Stored>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(Mutex::new(Stored {
        profile: b"{\"type\":\"Text\",\"hash\":\"\",\"text\":\"baseline\",\"hasData\":false}"
            .to_vec(),
        files: HashMap::new(),
    }));
    let shared = Arc::clone(&state);
    let worker = thread::spawn(move || {
        for _ in 0..count {
            let (mut socket, _) = listener.accept().unwrap();
            let (method, path, auth, body) = read_request(&mut socket);
            assert_eq!(auth, "Basic c3ludGhldGljOnBhc3M="); // synthetic:synthetic? custom token below
            let mut s = shared.lock().unwrap();
            let (status, answer) = match (method.as_str(), path.as_str()) {
                ("GET", "/SyncClipboard.json") => (200, s.profile.clone()),
                ("PUT", "/SyncClipboard.json") => {
                    s.profile = body;
                    (204, vec![])
                }
                ("PUT", path) if path.starts_with("/file/") => {
                    s.files.insert(path.to_owned(), body);
                    (204, vec![])
                }
                ("GET", path) if path.starts_with("/file/") => match s.files.get(path) {
                    Some(data) => (200, data.clone()),
                    None => (404, vec![]),
                },
                _ => (404, vec![]),
            };
            let response = format!(
                "HTTP/1.1 {status} OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
                answer.len()
            );
            socket.write_all(response.as_bytes()).unwrap();
            socket.write_all(&answer).unwrap();
        }
    });
    (address, state, worker)
}

fn png_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 2, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[
                2, 4, 6, 255, 8, 10, 12, 255, 14, 16, 18, 255, 20, 22, 24, 255,
            ])
            .unwrap();
    }
    bytes
}

#[test]
fn real_http_inline_text_roundtrip_against_mock() {
    let (url, state, worker) = spawn_mock(3);
    let client = ProtocolClient::new(&url, "synthetic", "pass").unwrap();
    let baseline = client.latest().unwrap();
    assert_eq!(baseline.text, "baseline");
    let identity = client.publish_text("hello from local unit test").unwrap();
    assert_eq!(
        identity,
        format!("Text:{}", digest_hex(b"hello from local unit test"))
    );
    let next = client.latest().unwrap();
    assert_eq!(next.kind, SnapshotKind::Text);
    assert_eq!(next.text, "hello from local unit test");
    worker.join().unwrap();
    assert!(!state.lock().unwrap().profile.is_empty());
}

#[test]
fn real_http_png_roundtrip_and_integrity() {
    let (url, _state, worker) = spawn_mock(4);
    let client = ProtocolClient::new(&url, "synthetic", "pass").unwrap();
    let pixels = png_bytes();
    let identity = client.publish_png(&pixels).unwrap();
    assert_eq!(identity, format!("Image:{}", image_profile_hash(&pixels)));
    let snap = client.latest().unwrap();
    assert_eq!(snap.kind, SnapshotKind::Image);
    assert_eq!(client.download(&snap).unwrap(), pixels);
    worker.join().unwrap();
}

#[test]
fn real_http_long_text_roundtrip() {
    let (url, _state, worker) = spawn_mock(4);
    let client = ProtocolClient::new(&url, "synthetic", "pass").unwrap();
    let text = "A".repeat(10_300);
    let identity = client.publish_text(&text).unwrap();
    assert_eq!(identity, format!("Text:{}", digest_hex(text.as_bytes())));
    let snap = client.latest().unwrap();
    assert!(snap.has_data);
    let downloaded = client.download(&snap).unwrap();
    assert_eq!(downloaded, text.as_bytes());
    worker.join().unwrap();
}

#[test]
fn insecure_remote_url_rejected_before_request() {
    assert!(matches!(
        ProtocolClient::new("http://example.com", "synthetic", "pass"),
        Err(ClientError::InvalidEndpoint)
    ));
}
