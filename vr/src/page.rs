use std::io::{ErrorKind, Read};
use std::net::{TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::{Message, WebSocket};

pub struct Frame {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Frame {
    pub fn parse(mut bytes: Vec<u8>, eye: [u32; 2]) -> Result<Frame, String> {
        if bytes.len() < 8 {
            return Err(format!("frame of {} bytes has no header", bytes.len()));
        }
        let [x, y, width, height] = [0, 1, 2, 3].map(|index| u16::from_le_bytes([bytes[index * 2], bytes[index * 2 + 1]]) as u32);
        if x + width > eye[0] || y + height > eye[1] {
            return Err(format!("frame rect {width}×{height} at {x},{y} exceeds the {}×{} eye", eye[0], eye[1]));
        }
        let expected = 8 + (2 * width * height * 4) as usize;
        if bytes.len() != expected {
            return Err(format!("frame of {} bytes, expected {expected}", bytes.len()));
        }
        bytes.drain(..8);
        Ok(Frame { x, y, width, height, pixels: bytes })
    }
}

pub struct Page {
    listener: TcpListener,
    nonce: String,
    origin: String,
    hello: Value,
    socket: Option<WebSocket<TcpStream>>,
    browser: Option<Child>,
}

pub fn nonce() -> String {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes)).expect("/dev/urandom");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn origin(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split('/').next().unwrap_or_default();
    format!("{}://{host}", url.split_once("://").map_or("https", |(scheme, _)| scheme))
}

pub fn page_url(page: &str, port: u16, nonce: &str) -> String {
    let separator = if page.contains('?') { '&' } else { '?' };
    format!("{page}{separator}vr={port}.{nonce}")
}

pub const BROWSER_FLAGS: &[&str] = &[
    "--headless=new",
    "--ignore-gpu-blocklist",
    "--use-gl=angle",
    "--use-angle=gl-egl",
    "--disable-features=LocalNetworkAccessChecksWebSockets",
    "--autoplay-policy=no-user-gesture-required",
    "--use-fake-ui-for-media-stream",
    "--disable-background-timer-throttling",
    "--disable-renderer-backgrounding",
    "--no-first-run",
    "--no-default-browser-check",
];

impl Page {
    pub fn open(browser: &str, page: &str, profile: &Path, hello: Value) -> std::io::Result<Page> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let nonce = nonce();
        let url = page_url(page, listener.local_addr()?.port(), &nonce);
        let mut command = Command::new(browser);
        command.args(BROWSER_FLAGS).arg(format!("--user-data-dir={}", profile.display())).arg(&url).stdin(Stdio::null()).stdout(Stdio::null());
        unsafe {
            command.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                Ok(())
            });
        }
        let child = command.spawn()?;
        Ok(Page { listener, nonce, origin: origin(page), hello, socket: None, browser: Some(child) })
    }

    pub fn browser_exited(&mut self) -> Option<std::process::ExitStatus> {
        self.browser.as_mut().and_then(|child| child.try_wait().ok().flatten())
    }

    fn accept(&mut self) {
        let Ok((stream, _)) = self.listener.accept() else { return };
        match self.handshake(stream) {
            Ok(socket) => {
                eprintln!("page connected");
                self.socket = Some(socket);
            }
            Err(error) => eprintln!("rejected a page connection: {error}"),
        }
    }

    fn handshake(&self, stream: TcpStream) -> Result<WebSocket<TcpStream>, String> {
        stream.set_nonblocking(false).map_err(|error| error.to_string())?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(|error| error.to_string())?;
        let origin = self.origin.clone();
        let check = move |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
            match request.headers().get("origin").and_then(|value| value.to_str().ok()) {
                Some(seen) if seen == origin => Ok(response),
                _ => Err(Response::builder().status(403).body(None).unwrap()),
            }
        };
        let mut socket = tungstenite::accept_hdr(stream, check).map_err(|error| error.to_string())?;
        let hello: Value = match socket.read().map_err(|error| error.to_string())? {
            Message::Text(text) => serde_json::from_str(&text).map_err(|error| error.to_string())?,
            _ => return Err("first message is not a hello".into()),
        };
        if hello["type"] != "hello" || hello["nonce"] != self.nonce.as_str() {
            return Err("wrong nonce".into());
        }
        socket.send(Message::Text(self.hello.to_string())).map_err(|error| error.to_string())?;
        socket.get_ref().set_nonblocking(true).map_err(|error| error.to_string())?;
        Ok(socket)
    }

    pub fn latest_frame(&mut self) -> Option<Vec<u8>> {
        self.accept();
        let socket = self.socket.as_mut()?;
        let mut frame = None;
        loop {
            match socket.read() {
                Ok(Message::Binary(bytes)) => frame = Some(bytes),
                Ok(_) => {}
                Err(tungstenite::Error::Io(error)) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => {
                    eprintln!("page disconnected: {error}");
                    self.socket = None;
                    break;
                }
            }
        }
        frame
    }

    pub fn wait(&self, timeout: Duration) {
        let fd = self.socket.as_ref().map_or(self.listener.as_raw_fd(), |socket| socket.get_ref().as_raw_fd());
        let mut readable = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        unsafe { libc::poll(&mut readable, 1, timeout.as_millis().max(1) as i32) };
    }

    pub fn send(&mut self, message: Value) {
        let Some(socket) = self.socket.as_mut() else { return };
        match socket.send(Message::Text(message.to_string())) {
            Ok(()) => {}
            Err(tungstenite::Error::Io(error)) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => {
                eprintln!("page disconnected: {error}");
                self.socket = None;
            }
        }
    }

    pub fn pose(&mut self, eyes: [[f32; 3]; 2], head: [f32; 3]) {
        self.send(json!({ "type": "pose", "eyes": eyes, "head": head }));
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        if let Some(mut child) = self.browser.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_learns_its_host_through_one_query_parameter() {
        assert_eq!(page_url("https://example.org/voice/", 4123, "ab"), "https://example.org/voice/?vr=4123.ab");
        assert_eq!(page_url("http://127.0.0.1:5173/?x=1", 9, "c"), "http://127.0.0.1:5173/?x=1&vr=9.c");
        assert_eq!(origin("https://example.org/voice/"), "https://example.org");
        assert_eq!(origin("http://127.0.0.1:5173/"), "http://127.0.0.1:5173");
        assert_eq!(nonce().len(), 32);
        assert_ne!(nonce(), nonce());
    }

    fn frame(rect: [u16; 4], pixels: usize) -> Vec<u8> {
        rect.iter().flat_map(|value| value.to_le_bytes()).chain(std::iter::repeat_n(7, pixels)).collect()
    }

    #[test]
    fn a_frame_carries_one_rect_read_from_both_eyes() {
        let parsed = Frame::parse(frame([3, 2, 5, 6], 2 * 5 * 6 * 4), [16, 8]).unwrap();
        assert_eq!((parsed.x, parsed.y, parsed.width, parsed.height, parsed.pixels.len()), (3, 2, 5, 6, 240));
        assert_eq!(Frame::parse(frame([0, 0, 0, 0], 0), [16, 8]).unwrap().pixels.len(), 0);
        assert!(Frame::parse(frame([3, 2, 5, 6], 2 * 5 * 6 * 4 - 1), [16, 8]).is_err());
        assert!(Frame::parse(frame([12, 0, 5, 1], 40), [16, 8]).is_err());
        assert!(Frame::parse(frame([0, 4, 1, 5], 40), [16, 8]).is_err());
        assert!(Frame::parse(vec![0; 7], [16, 8]).is_err());
    }
}
