use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

const ALPN: &[u8] = b"voice-web/1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_FRAME: usize = 32 << 20;
const MAX_ASSET: usize = 256 << 20;
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Deserialize)]
pub struct Token {
    endpoint_id: String,
    relay_url: Option<String>,
    secret: String,
}

impl Token {
    pub fn decode(raw: &str) -> Result<Token, String> {
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(raw.trim().trim_end_matches('=')).map_err(|_| "the token is not base64url")?;
        serde_json::from_slice(&bytes).map_err(|_| "the token is missing pieces".to_owned())
    }
}

#[derive(Clone, Deserialize)]
pub struct Avatar {
    pub id: String,
    #[serde(rename = "contentHash")]
    pub content_hash: String,
}

#[derive(Deserialize)]
pub struct Catalog {
    pub avatars: Vec<Avatar>,
}

#[derive(Deserialize)]
pub struct ClipEntry {
    pub action: String,
    pub name: String,
    pub format: String,
    #[serde(rename = "contentHash")]
    pub content_hash: String,
}

#[derive(Deserialize)]
struct Clips {
    clips: Vec<ClipEntry>,
}

#[derive(Deserialize)]
struct Start {
    id: String,
    size: usize,
    #[serde(rename = "originalSize")]
    original_size: usize,
    #[serde(rename = "contentHash")]
    content_hash: String,
    encoding: String,
}

pub struct Relay {
    runtime: tokio::runtime::Runtime,
    endpoint: iroh::Endpoint,
    connection: iroh::endpoint::Connection,
    send: iroh::endpoint::SendStream,
    recv: Option<iroh::endpoint::RecvStream>,
    cache: PathBuf,
    broken: bool,
}

async fn read_frame(recv: &mut iroh::endpoint::RecvStream) -> Result<Vec<u8>, String> {
    let mut length = [0u8; 4];
    recv.read_exact(&mut length).await.map_err(|error| format!("relay read: {error}"))?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_FRAME {
        return Err(format!("relay frame of {length} bytes"));
    }
    let mut frame = vec![0u8; length];
    recv.read_exact(&mut frame).await.map_err(|error| format!("relay read: {error}"))?;
    Ok(frame)
}

pub fn split(frame: &[u8]) -> (&str, &[u8]) {
    let end = frame.iter().position(|&byte| byte == b'\n').unwrap_or(frame.len());
    (std::str::from_utf8(&frame[..end]).unwrap_or(""), frame.get(end + 1..).unwrap_or(&[]))
}

pub fn message(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body).ok().and_then(|value| value["message"].as_str().map(str::to_owned)).unwrap_or_else(|| String::from_utf8_lossy(body).into_owned())
}

impl Relay {
    pub fn connect(token: &Token, cache: &Path) -> Result<Relay, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().map_err(|error| format!("relay runtime: {error}"))?;
        let (endpoint, connection, send, recv) = runtime.block_on(async {
            let endpoint = iroh::Endpoint::builder(iroh::endpoint::presets::N0).bind().await.map_err(|error| format!("relay endpoint: {error}"))?;
            let id: iroh::EndpointId = token.endpoint_id.parse().map_err(|_| "the token's endpoint id is invalid")?;
            let mut address = iroh::EndpointAddr::new(id);
            if let Some(url) = &token.relay_url {
                address = address.with_relay_url(url.parse::<iroh::RelayUrl>().map_err(|_| "the token's relay url is invalid")?);
            }
            let connection = endpoint.connect(address, ALPN).await.map_err(|error| format!("relay transport failed: {error}"))?;
            let (send, recv) = connection.open_bi().await.map_err(|error| format!("relay stream: {error}"))?;
            Ok::<_, String>((endpoint, connection, send, recv))
        })?;
        let cache = cache.join(&token.endpoint_id);
        std::fs::create_dir_all(&cache).map_err(|error| format!("{}: {error}", cache.display()))?;
        let mut relay = Relay { runtime, endpoint, connection, send, recv: Some(recv), cache, broken: false };
        relay.send(json!({ "auth": token.secret }).to_string().as_bytes())?;
        let reply: Value = serde_json::from_slice(&relay.recv(Instant::now() + REQUEST_TIMEOUT)?).map_err(|_| "authentication reply is not JSON")?;
        if reply["ok"].as_bool() != Some(true) {
            return Err("token rejected".into());
        }
        Ok(relay)
    }

    fn send(&mut self, frame: &[u8]) -> Result<(), String> {
        let length = u32::try_from(frame.len()).map_err(|_| "frame too large")?;
        let send = &mut self.send;
        self.runtime.block_on(async {
            send.write_all(&length.to_le_bytes()).await?;
            send.write_all(frame).await
        })
        .map_err(|error| format!("relay write: {error}"))
    }

    fn recv(&mut self, deadline: Instant) -> Result<Vec<u8>, String> {
        let Some(recv) = self.recv.as_mut().filter(|_| !self.broken) else {
            return Err("the relay stream lost its framing".into());
        };
        let frame = self.runtime.block_on(async { tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), read_frame(recv)).await.map_err(|_| "the relay went quiet".to_owned())? });
        self.broken = frame.is_err();
        frame
    }

    /// Hands every later frame to a channel that a waiting reader can give up on at any moment; the channel closes when the stream ends.
    pub fn listen(&mut self) -> mpsc::Receiver<Result<Vec<u8>, String>> {
        let (sender, receiver) = mpsc::channel();
        if let Some(mut recv) = self.recv.take() {
            self.runtime.spawn(async move {
                loop {
                    let frame = read_frame(&mut recv).await;
                    let failed = frame.is_err();
                    if sender.send(frame).is_err() || failed {
                        break;
                    }
                }
            });
        }
        receiver
    }

    /// Sends `verb`, with a JSON body when there is one.
    pub fn post(&mut self, verb: &str, body: Option<&Value>) -> Result<(), String> {
        match body {
            Some(body) => self.send(format!("{verb}\n{body}").as_bytes()),
            None => self.send(verb.as_bytes()),
        }
    }

    /// Ends this connection's session, drops its pending requests, and deletes the standing instructions, as the page's Reset does.
    pub fn forget(&mut self) -> Result<(), String> {
        self.exchange(b"forget", "forget", "forget-ok").map(drop)
    }

    fn request(&mut self, verb: &str) -> Result<Vec<u8>, String> {
        self.exchange(verb.as_bytes(), verb, verb)
    }

    fn exchange(&mut self, frame: &[u8], verb: &str, reply_verb: &str) -> Result<Vec<u8>, String> {
        self.send(frame)?;
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            let frame = self.recv(deadline)?;
            let (reply, body) = split(&frame);
            if reply == reply_verb {
                return Ok(body.to_vec());
            }
            if reply == format!("{verb}-error") {
                return Err(format!("{verb}: {}", message(body)));
            }
        }
    }

    pub fn catalog(&mut self) -> Result<Catalog, String> {
        serde_json::from_slice(&self.request("puppets")?).map_err(|error| format!("puppet catalog: {error}"))
    }

    pub fn clips(&mut self) -> Result<Vec<ClipEntry>, String> {
        Ok(serde_json::from_slice::<Clips>(&self.request("clips")?).map_err(|error| format!("clip catalog: {error}"))?.clips)
    }

    pub fn puppet(&mut self, avatar: &Avatar) -> Result<Vec<u8>, String> {
        self.transfer("puppet", &avatar.id, &avatar.content_hash, "vrm", json!({}))
    }

    pub fn motion(&mut self, clip: &ClipEntry) -> Result<Vec<u8>, String> {
        self.transfer("motion", &clip.name, &clip.content_hash, "motion", json!({ "clipHash": clip.content_hash }))
    }

    fn transfer(&mut self, kind: &str, id: &str, content_hash: &str, extension: &str, mut fields: Value) -> Result<Vec<u8>, String> {
        if content_hash.is_empty() || !content_hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("{kind} {id}: the catalog's content hash is not hex"));
        }
        let cached = self.cache.join(format!("{content_hash}.{extension}"));
        if let Ok(bytes) = std::fs::read(&cached) {
            return Ok(bytes);
        }
        fields["id"] = id.into();
        fields["encodings"] = json!(["gzip"]);
        self.send(format!("{kind}\n{fields}").as_bytes())?;
        let deadline = Instant::now() + TRANSFER_TIMEOUT;
        let mut start: Option<Start> = None;
        let mut compressed = Vec::new();
        loop {
            let frame = self.recv(deadline)?;
            let (verb, body) = split(&frame);
            let Some(step) = verb.strip_prefix(kind).and_then(|rest| rest.strip_prefix('-')) else { continue };
            match step {
                "start" => {
                    let value: Start = serde_json::from_slice(body).map_err(|_| format!("{kind} {id}: invalid transfer start"))?;
                    if value.id != id {
                        continue;
                    }
                    if start.is_some() || value.content_hash != content_hash || value.encoding != "gzip" || value.size == 0 || value.original_size == 0 || value.size > MAX_ASSET || value.original_size > MAX_ASSET {
                        return Err(format!("{kind} {id}: invalid transfer"));
                    }
                    compressed.reserve(value.size);
                    start = Some(value);
                }
                "chunk" => {
                    let (chunk_id, chunk) = split(body);
                    if chunk_id != id {
                        continue;
                    }
                    let size = start.as_ref().ok_or_else(|| format!("{kind} {id}: chunk before start"))?.size;
                    compressed.extend_from_slice(chunk);
                    if compressed.len() > size {
                        return Err(format!("{kind} {id}: exceeds its advertised size"));
                    }
                }
                "end" if body == id.as_bytes() => {
                    let start = start.ok_or_else(|| format!("{kind} {id}: end before start"))?;
                    if compressed.len() != start.size {
                        return Err(format!("{kind} {id}: incomplete transfer"));
                    }
                    let mut bytes = Vec::with_capacity(start.original_size);
                    flate2::read::GzDecoder::new(compressed.as_slice()).take(start.original_size as u64 + 1).read_to_end(&mut bytes).map_err(|error| format!("{kind} {id}: {error}"))?;
                    if bytes.len() != start.original_size {
                        return Err(format!("{kind} {id}: invalid decompressed size"));
                    }
                    // The preview thread and a picked appearance can fetch the same asset at once.
                    static WRITES: AtomicU64 = AtomicU64::new(0);
                    let temporary = cached.with_extension(format!("{extension}.{}-{}.tmp", std::process::id(), WRITES.fetch_add(1, Ordering::Relaxed)));
                    if let Err(error) = std::fs::write(&temporary, &bytes).and_then(|()| std::fs::rename(&temporary, &cached)) {
                        eprintln!("{} not cached: {error}", cached.display());
                    }
                    return Ok(bytes);
                }
                "error" => {
                    let value: Value = serde_json::from_slice(body).unwrap_or_default();
                    if value["code"].as_str() != Some("busy") && value["id"].as_str().is_none_or(|other| other == id) {
                        return Err(format!("{kind} {id}: {}", message(body)));
                    }
                }
                _ => {}
            }
        }
    }
}

impl Drop for Relay {
    /// Delivers what was sent, then closes the connection, so the server ends this connection's session now rather than at an idle timeout.
    fn drop(&mut self) {
        let (send, connection, endpoint) = (&mut self.send, &self.connection, &self.endpoint);
        self.runtime.block_on(async {
            if send.finish().is_ok() {
                let _ = tokio::time::timeout(CLOSE_TIMEOUT, send.stopped()).await;
            }
            connection.close(iroh::endpoint::VarInt::from_u32(0), b"done");
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, endpoint.close()).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_decodes_from_unpadded_base64url() {
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"endpoint_id":"abc","relay_url":"https://relay.example/","secret":"s?>"}"#);
        let token = Token::decode(&format!(" {encoded}\n")).unwrap();
        assert_eq!((token.endpoint_id.as_str(), token.relay_url.as_deref(), token.secret.as_str()), ("abc", Some("https://relay.example/"), "s?>"));
        assert!(Token::decode("not a token!").is_err());
    }

    #[test]
    fn a_frame_splits_at_its_first_newline() {
        assert_eq!(split(b"puppet-chunk\nid\n\x00\n"), ("puppet-chunk", &b"id\n\x00\n"[..]));
        assert_eq!(split(b"clips"), ("clips", &b""[..]));
    }
}
