use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

const ALPN: &[u8] = b"voice-web/1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_FRAME: usize = 32 << 20;
const MAX_ASSET: usize = 256 << 20;
const STOP_TIMEOUT: Duration = Duration::from_secs(1);

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
    wire: Box<dyn Wire + Send>,
    cache: PathBuf,
    broken: bool,
}

/// How frames reach the voice server and come back: an iroh stream, or in tests a scripted server.
trait Wire {
    fn send(&mut self, frame: &[u8]) -> Result<(), String>;
    fn recv(&mut self, deadline: Instant) -> Result<Vec<u8>, String>;
    fn listen(&mut self) -> mpsc::Receiver<Result<Vec<u8>, String>>;
}

struct Iroh {
    runtime: Arc<tokio::runtime::Runtime>,
    endpoint: iroh::Endpoint,
    connection: iroh::endpoint::Connection,
    send: Option<iroh::endpoint::SendStream>,
    recv: Option<iroh::endpoint::RecvStream>,
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

fn dial(address: &iroh::EndpointAddr) -> Result<Iroh, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().map_err(|error| format!("relay runtime: {error}"))?;
    let endpoint = runtime.block_on(iroh::Endpoint::builder(iroh::endpoint::presets::N0).bind()).map_err(|error| format!("relay endpoint: {error}"))?;
    let opened = runtime.block_on(async {
        let connection = endpoint.connect(address.clone(), ALPN).await.map_err(|error| format!("relay transport failed: {error}"))?;
        let (send, recv) = connection.open_bi().await.map_err(|error| format!("relay stream: {error}"))?;
        Ok::<_, String>((connection, send, recv))
    });
    match opened {
        Ok((connection, send, recv)) => Ok(Iroh { runtime: Arc::new(runtime), endpoint, connection, send: Some(send), recv: Some(recv) }),
        Err(error) => {
            runtime.block_on(endpoint.close());
            Err(error)
        }
    }
}

impl Relay {
    pub fn connect(token: &Token, cache: &Path) -> Result<Relay, String> {
        Relay::connect_while(token, cache, |_| false)
    }

    /// Dials until the transport opens, asking `again` with each failure whether to dial once more; each dial binds a fresh endpoint, so a network that came up since is used.
    pub fn connect_while(token: &Token, cache: &Path, mut again: impl FnMut(&str) -> bool) -> Result<Relay, String> {
        let id: iroh::EndpointId = token.endpoint_id.parse().map_err(|_| "the token's endpoint id is invalid")?;
        let mut address = iroh::EndpointAddr::new(id);
        if let Some(url) = &token.relay_url {
            address = address.with_relay_url(url.parse::<iroh::RelayUrl>().map_err(|_| "the token's relay url is invalid")?);
        }
        let cache = cache.join(&token.endpoint_id);
        std::fs::create_dir_all(&cache).map_err(|error| format!("{}: {error}", cache.display()))?;
        let mut relay = loop {
            match dial(&address) {
                Ok(dialed) => break Relay { wire: Box::new(dialed), cache: cache.clone(), broken: false },
                Err(error) if again(&error) => tracing::warn!("{error}, dialing again"),
                Err(error) => return Err(error),
            }
        };
        relay.send(json!({ "auth": token.secret }).to_string().as_bytes())?;
        let reply: Value = serde_json::from_slice(&relay.recv(Instant::now() + REQUEST_TIMEOUT)?).map_err(|_| "authentication reply is not JSON")?;
        if reply["ok"].as_bool() != Some(true) {
            return Err("token rejected".into());
        }
        Ok(relay)
    }

    fn send(&mut self, frame: &[u8]) -> Result<(), String> {
        if self.broken {
            return Err("the relay stream lost its framing".into());
        }
        self.wire.send(frame)
    }

    fn recv(&mut self, deadline: Instant) -> Result<Vec<u8>, String> {
        if self.broken {
            return Err("the relay stream lost its framing".into());
        }
        let frame = self.wire.recv(deadline);
        self.broken = frame.is_err();
        frame
    }

    /// Hands every later frame to a channel that a waiting reader can give up on at any moment; the channel closes when the stream ends.
    pub fn listen(&mut self) -> mpsc::Receiver<Result<Vec<u8>, String>> {
        self.wire.listen()
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
        if !hex(content_hash) {
            return Err(format!("{kind} {id}: the catalog's content hash is not hex"));
        }
        if let Ok(bytes) = std::fs::read(self.cache.join(format!("{content_hash}.{extension}"))) {
            return Ok(bytes);
        }
        fields["id"] = id.into();
        fields["encodings"] = json!(["gzip"]);
        self.send(format!("{kind}\n{fields}").as_bytes())?;
        let received = self.receive(kind, id, extension);
        // The server streams a transfer to its end once started, so a reader that stops early leaves its frames ahead of every later reply.
        if received.as_ref().is_err_and(|(finished, _)| !finished) {
            self.broken = true;
        }
        received.map_err(|(_, error)| error)
    }

    /// The transfer of `id` the server answers with, which is the file it holds now: a catalog read before a republish names an older hash.
    /// Its error says whether the server ended the transfer.
    fn receive(&mut self, kind: &str, id: &str, extension: &str) -> Result<Vec<u8>, (bool, String)> {
        let deadline = Instant::now() + TRANSFER_TIMEOUT;
        let mut start: Option<Start> = None;
        let mut compressed = Vec::new();
        loop {
            let frame = self.recv(deadline).map_err(|error| (false, error))?;
            let (verb, body) = split(&frame);
            let Some(step) = verb.strip_prefix(kind).and_then(|rest| rest.strip_prefix('-')) else { continue };
            match step {
                "start" => {
                    let value: Start = serde_json::from_slice(body).map_err(|_| (false, format!("{kind} {id}: invalid transfer start")))?;
                    if value.id != id {
                        continue;
                    }
                    if start.is_some() || !hex(&value.content_hash) || value.encoding != "gzip" || value.size == 0 || value.original_size == 0 || value.size > MAX_ASSET || value.original_size > MAX_ASSET {
                        return Err((false, format!("{kind} {id}: invalid transfer")));
                    }
                    compressed.reserve(value.size);
                    start = Some(value);
                }
                "chunk" => {
                    let (chunk_id, chunk) = split(body);
                    if chunk_id != id {
                        continue;
                    }
                    let size = start.as_ref().ok_or_else(|| (false, format!("{kind} {id}: chunk before start")))?.size;
                    compressed.extend_from_slice(chunk);
                    if compressed.len() > size {
                        return Err((false, format!("{kind} {id}: exceeds its advertised size")));
                    }
                }
                "end" if body == id.as_bytes() => {
                    let start = start.ok_or_else(|| (true, format!("{kind} {id}: end before start")))?;
                    if compressed.len() != start.size {
                        return Err((true, format!("{kind} {id}: incomplete transfer")));
                    }
                    let mut bytes = Vec::with_capacity(start.original_size);
                    flate2::read::GzDecoder::new(compressed.as_slice()).take(start.original_size as u64 + 1).read_to_end(&mut bytes).map_err(|error| (true, format!("{kind} {id}: {error}")))?;
                    if bytes.len() != start.original_size {
                        return Err((true, format!("{kind} {id}: invalid decompressed size")));
                    }
                    let cached = self.cache.join(format!("{}.{extension}", start.content_hash));
                    // The preview thread and a picked appearance can fetch the same asset at once.
                    static WRITES: AtomicU64 = AtomicU64::new(0);
                    let temporary = cached.with_extension(format!("{extension}.{}-{}.tmp", std::process::id(), WRITES.fetch_add(1, Ordering::Relaxed)));
                    if let Err(error) = std::fs::write(&temporary, &bytes).and_then(|()| std::fs::rename(&temporary, &cached)) {
                        tracing::warn!("{} not cached: {error}", cached.display());
                    }
                    return Ok(bytes);
                }
                "error" => {
                    let value: Value = serde_json::from_slice(body).unwrap_or_default();
                    if value["code"].as_str() != Some("busy") && value["id"].as_str().is_none_or(|other| other == id) {
                        return Err((true, format!("{kind} {id}: {}", message(body))));
                    }
                }
                _ => {}
            }
        }
    }
}

fn hex(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl Wire for Iroh {
    fn send(&mut self, frame: &[u8]) -> Result<(), String> {
        let length = u32::try_from(frame.len()).map_err(|_| "frame too large")?;
        let send = self.send.as_mut().expect("the send stream leaves only when the relay drops");
        self.runtime.block_on(async {
            send.write_all(&length.to_le_bytes()).await?;
            send.write_all(frame).await
        })
        .map_err(|error| format!("relay write: {error}"))
    }

    fn recv(&mut self, deadline: Instant) -> Result<Vec<u8>, String> {
        let Some(recv) = self.recv.as_mut() else {
            return Err("the relay stream lost its framing".into());
        };
        self.runtime.block_on(async { tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), read_frame(recv)).await.map_err(|_| "the relay went quiet".to_owned())? })
    }

    fn listen(&mut self) -> mpsc::Receiver<Result<Vec<u8>, String>> {
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
}

impl Drop for Iroh {
    /// Delivers what was sent, then closes the connection, so the server ends this connection's session now rather than at an idle timeout.
    /// The endpoint's close waits for the peer to acknowledge, up to iroh's own probe timeout; that wait runs off the dropping thread, which is often the render loop.
    fn drop(&mut self) {
        let (runtime, connection, endpoint) = (self.runtime.clone(), self.connection.clone(), self.endpoint.clone());
        let mut send = self.send.take();
        std::thread::spawn(move || {
            runtime.block_on(async {
                if let Some(send) = send.as_mut() {
                    if send.finish().is_ok() {
                        let _ = tokio::time::timeout(STOP_TIMEOUT, send.stopped()).await;
                    }
                }
                connection.close(iroh::endpoint::VarInt::from_u32(0), b"done");
                endpoint.close().await;
            })
        });
    }
}

/// A relay to a scripted server, which answers each request frame with the frames it returns.
#[cfg(test)]
pub fn scripted(cache: &Path, server: impl FnMut(&str, &Value) -> Vec<Vec<u8>> + Send + 'static) -> Relay {
    struct Script<F> {
        server: F,
        queued: std::collections::VecDeque<Vec<u8>>,
    }
    impl<F: FnMut(&str, &Value) -> Vec<Vec<u8>>> Wire for Script<F> {
        fn send(&mut self, frame: &[u8]) -> Result<(), String> {
            let (verb, body) = split(frame);
            let answer = (self.server)(verb, &serde_json::from_slice(body).unwrap_or_default());
            self.queued.extend(answer);
            Ok(())
        }
        fn recv(&mut self, _: Instant) -> Result<Vec<u8>, String> {
            self.queued.pop_front().ok_or_else(|| "the relay went quiet".to_owned())
        }
        fn listen(&mut self) -> mpsc::Receiver<Result<Vec<u8>, String>> {
            mpsc::channel().1
        }
    }
    std::fs::create_dir_all(cache).unwrap();
    Relay { wire: Box::new(Script { server, queued: Default::default() }), cache: cache.to_owned(), broken: false }
}

/// The frames the server sends `bytes` in as the `kind` transfer of `id`, and their content hash.
#[cfg(test)]
pub fn served(kind: &str, id: &str, bytes: &[u8]) -> (String, Vec<Vec<u8>>) {
    use sha2::Digest;
    use std::io::Write;
    let hash: String = sha2::Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect();
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(bytes).unwrap();
    let compressed = gzip.finish().unwrap();
    let start = json!({ "id": id, "size": compressed.len(), "originalSize": bytes.len(), "contentHash": hash, "encoding": "gzip" });
    let mut frames = vec![format!("{kind}-start\n{start}").into_bytes()];
    for chunk in compressed.chunks(1000) {
        frames.push([format!("{kind}-chunk\n{id}\n").as_bytes(), chunk].concat());
    }
    frames.push(format!("{kind}-end\n{id}").into_bytes());
    (hash, frames)
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

    #[test]
    fn a_puppet_republished_since_the_catalog_was_read_arrives_as_the_server_now_serves_it() {
        let cache = std::env::temp_dir().join(format!("voice-vr-relay-republished-{}", std::process::id()));
        let (hash, frames) = served("puppet", "a", b"republished");
        let mut relay = scripted(&cache, move |verb, _| if verb == "puppet" { frames.clone() } else { Vec::new() });
        let read_before = Avatar { id: "a".into(), content_hash: "0123abcd".into() };
        assert_eq!(relay.puppet(&read_before).unwrap(), b"republished");
        assert_eq!(std::fs::read(cache.join(format!("{hash}.vrm"))).unwrap(), b"republished", "cached under the hash it was served with");
        std::fs::remove_dir_all(&cache).unwrap();
    }

    #[test]
    fn a_transfer_given_up_midway_fails_the_next_request_at_once_instead_of_waiting_behind_its_frames() {
        let cache = std::env::temp_dir().join(format!("voice-vr-relay-abandoned-{}", std::process::id()));
        let (_, mut frames) = served("puppet", "a", &[7; 5000]);
        frames[0] = format!("puppet-start\n{}", json!({ "id": "a", "size": 9, "originalSize": 5000, "contentHash": "00", "encoding": "gzip" })).into_bytes();
        let requests = Arc::new(AtomicU64::new(0));
        let counted = requests.clone();
        let mut relay = scripted(&cache, move |verb, request| {
            counted.fetch_add(1, Ordering::Relaxed);
            match (verb, request["id"].as_str()) {
                ("puppet", Some("a")) => frames.clone(),
                _ => vec![format!("puppet-error\n{}", json!({ "id": request["id"], "code": "busy", "message": "request rejected; current transfer continues" })).into_bytes()],
            }
        });
        assert!(relay.puppet(&Avatar { id: "a".into(), content_hash: "00".into() }).unwrap_err().contains("exceeds its advertised size"));
        assert!(relay.puppet(&Avatar { id: "b".into(), content_hash: "01".into() }).unwrap_err().contains("lost its framing"));
        assert_eq!(requests.load(Ordering::Relaxed), 1, "nothing is asked of a stream still carrying another transfer");
        std::fs::remove_dir_all(&cache).unwrap();
    }
}
