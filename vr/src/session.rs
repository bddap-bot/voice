use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use str0m::change::SdpAnswer;
use str0m::channel::ChannelId;
use str0m::media::{Direction, MediaKind, MediaTime, Mid};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event as RtcEvent, IceConnectionState, Input, Output, Rtc};

use crate::audio::{Audio, PlaybackBuffer, RATE};
use crate::conversation::Wake;
use crate::identity::identity;
use crate::hub::Command;
use crate::relay::{message, split, Relay, Token};
use crate::speaker::{Ear, Hearing};

const FRAME: usize = RATE as usize / 50;
const TICK: Duration = Duration::from_millis(20);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(40);
/// The page's wait for `session.started`.
const START_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(10);
/// Most of the microphone queued for sending; anything older is dropped so speech goes out live.
const BACKLOG: usize = 3 * FRAME;

#[derive(Debug, PartialEq)]
pub enum Event {
    Open,
    Heard(String),
    Spoke(String),
    Delegated(String),
    Hub(Vec<u8>),
    HubError { id: String, message: String },
    ContextUsage(f64),
    Closed(String),
}

/// One Live conversation on its own relay connection and thread; dropping it ends the session and releases the microphone.
pub struct Session {
    stop: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    events: mpsc::Receiver<Event>,
    commands: mpsc::Sender<Command>,
    played: Arc<Mutex<PlaybackBuffer>>,
}

impl Session {
    pub fn open(token: &Token, cache: &Path, wake: Wake, muted: bool, hearing: Arc<Mutex<Hearing>>) -> Session {
        let stop = Arc::new(AtomicBool::new(false));
        let muted = Arc::new(AtomicBool::new(muted));
        let (event_sender, events) = mpsc::channel();
        let (commands, command_receiver) = mpsc::channel();
        let played = Arc::new(Mutex::new(PlaybackBuffer::default()));
        let link = Link { token: token.clone(), cache: cache.to_owned(), stop: stop.clone(), muted: muted.clone(), events: event_sender.clone(), commands: command_receiver, played: played.clone(), hearing };
        std::thread::spawn(move || {
            let reason = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| link.run(wake))) {
                Ok(Ok(reason) | Err(reason)) => reason,
                Err(_) => "the session thread panicked".into(),
            };
            let _ = event_sender.send(Event::Closed(reason));
        });
        Session { stop, muted, events, commands, played }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    pub fn events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    pub fn mute(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    /// The output energy the mouth follows.
    pub fn level(&self) -> f32 {
        self.played.lock().unwrap().level()
    }

    pub fn listen_for_quiet(&self) {
        self.played.lock().unwrap().listen_for_quiet();
    }

    pub fn quiet(&self, duration: Duration) -> bool {
        self.played.lock().unwrap().quiet((duration.as_secs_f64() * RATE as f64) as usize)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

struct Link {
    token: Token,
    cache: PathBuf,
    stop: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    events: mpsc::Sender<Event>,
    commands: mpsc::Receiver<Command>,
    played: Arc<Mutex<PlaybackBuffer>>,
    hearing: Arc<Mutex<Hearing>>,
}

fn offer_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!("vr-{}-{nanos:x}", std::process::id())
}

/// The address the default route leaves from; the host candidate must name a concrete address.
fn local_address() -> Result<SocketAddr, String> {
    let probe = UdpSocket::bind("0.0.0.0:0").and_then(|socket| socket.connect("8.8.8.8:80").map(|()| socket)).map_err(|error| format!("no route for the peer connection: {error}"))?;
    Ok(SocketAddr::new(probe.local_addr().map_err(|error| error.to_string())?.ip(), 0))
}

fn stopped(stop: &AtomicBool) -> bool {
    stop.load(Ordering::Relaxed)
}

impl Link {
    /// Runs the session until it ends; the result is why.
    fn run(self, wake: Wake) -> Result<String, String> {
        let mut relay = Relay::connect(&self.token, &self.cache)?;
        if stopped(&self.stop) {
            return Ok("dismissed".into());
        }
        let mut audio = Audio::open(self.played.clone())?;
        let mut capturing = !self.muted.load(Ordering::Relaxed);
        audio.capture(capturing)?;
        let socket = UdpSocket::bind(local_address()?).map_err(|error| format!("peer socket: {error}"))?;
        let local = socket.local_addr().map_err(|error| error.to_string())?;
        let mut rtc = Rtc::builder().clear_codecs().enable_opus(true, false).build(Instant::now());
        rtc.add_local_candidate(Candidate::host(local, "udp").map_err(|error| format!("candidate: {error}"))?);
        let mut change = rtc.sdp_api();
        let mid = change.add_media(MediaKind::Audio, Direction::SendRecv, None, None, None);
        let channel = change.add_channel("oai-events".into());
        let (offer, pending) = change.apply().ok_or("no offer to make")?;
        let id = offer_id();
        let mut body = serde_json::to_value(&wake).unwrap();
        body["id"] = id.clone().into();
        body["sdp"] = offer.to_sdp_string().into();
        relay.post("offer", Some(&body))?;
        let frames = relay.listen();
        let result = self.answer(&frames, &id, &mut audio, &mut capturing).and_then(|sdp| {
            rtc.sdp_api().accept_answer(pending, SdpAnswer::from_sdp_string(&sdp).map_err(|error| format!("answer: {error}"))?).map_err(|error| format!("answer: {error}"))?;
            self.converse(&mut rtc, &socket, &mut relay, &frames, &mut audio, mid, channel, &mut capturing)
        });
        drop(audio);
        if let Err(error) = relay.post("cancel", None) {
            eprintln!("cancel not sent: {error}");
        }
        result
    }

    /// Opens or releases the capture device when Mute mic changed.
    fn follow_mute(&self, audio: &mut Audio, capturing: &mut bool) -> Result<(), String> {
        let wanted = !self.muted.load(Ordering::Relaxed);
        if wanted != *capturing {
            audio.capture(wanted)?;
            *capturing = wanted;
        }
        Ok(())
    }

    fn answer(&self, frames: &mpsc::Receiver<Result<Vec<u8>, String>>, id: &str, audio: &mut Audio, capturing: &mut bool) -> Result<String, String> {
        let deadline = Instant::now() + ANSWER_TIMEOUT;
        loop {
            if stopped(&self.stop) {
                return Err("dismissed".into());
            }
            self.follow_mute(audio, capturing)?;
            if Instant::now() >= deadline {
                return Err("no answer to the offer".into());
            }
            let frame = match frames.recv_timeout(POLL) {
                Ok(frame) => frame?,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err("the relay closed".into()),
            };
            let (verb, body) = split(&frame);
            let value: Value = serde_json::from_slice(body).unwrap_or_default();
            let mine = value["offer_id"].as_str().or(value["id"].as_str()) == Some(id);
            match verb {
                "answer" if mine => return value["sdp"].as_str().map(str::to_owned).ok_or_else(|| "the answer has no sdp".into()),
                "offer-error" if mine => return Err(format!("offer: {}", message(body))),
                _ => {}
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn converse(&self, rtc: &mut Rtc, socket: &UdpSocket, relay: &mut Relay, frames: &mpsc::Receiver<Result<Vec<u8>, String>>, audio: &mut Audio, mid: Mid, channel: ChannelId, capturing: &mut bool) -> Result<String, String> {
        let mut encoder = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip).map_err(|error| format!("opus encoder: {error}"))?;
        let mut decoder = opus::Decoder::new(RATE, opus::Channels::Mono).map_err(|error| format!("opus decoder: {error}"))?;
        let local = socket.local_addr().map_err(|error| error.to_string())?;
        let started = Instant::now();
        let mut open = false;
        let mut connected = false;
        let mut next_tick = Instant::now();
        let mut rtp_time: u64 = fastrand::u32(..) as u64;
        let mut packet = vec![0u8; 4000];
        let mut decoded = vec![0f32; 5760];
        let mut buffer = vec![0u8; 2000];
        let mut ear = Ear::new(self.hearing.clone());
        loop {
            let timeout = loop {
                match rtc.poll_output().map_err(|error| format!("peer connection: {error}"))? {
                    Output::Timeout(at) => break at,
                    Output::Transmit(transmit) => {
                        if let Err(error) = socket.send_to(&transmit.contents, transmit.destination) {
                            eprintln!("peer send: {error}");
                        }
                    }
                    Output::Event(event) => match event {
                        RtcEvent::Connected => {
                            connected = true;
                            audio.captured.lock().unwrap().clear();
                        }
                        RtcEvent::IceConnectionStateChange(IceConnectionState::Disconnected) => return Ok("the connection dropped".into()),
                        RtcEvent::MediaData(data) => match decoder.decode_float(&data.data, &mut decoded, false) {
                            Ok(samples) => self.played.lock().unwrap().push(&decoded[..samples]),
                            Err(error) => eprintln!("opus decode: {error}"),
                        },
                        RtcEvent::ChannelData(data) if data.id == channel => {
                            let Ok(event) = serde_json::from_slice::<Value>(&data.data) else { continue };
                            match event["type"].as_str().unwrap_or("") {
                                "session.started" if !open => {
                                    open = true;
                                    let mut channel = rtc.channel(channel).ok_or("the event channel closed")?;
                                    let identity = identity();
                                    for (kind, id, content) in [("session.instructions.append", "identity", &identity.instructions), ("session.commentary.append", "wake", &identity.woken)] {
                                        channel.write(false, json!({ "type": kind, "event_id": id, "delegation_id": null, "content": content }).to_string().as_bytes()).map_err(|error| format!("event channel: {error}"))?;
                                    }
                                    let _ = self.events.send(Event::Open);
                                }
                                "session.input_transcript.delta" => {
                                    self.played.lock().unwrap().clear();
                                    let _ = self.events.send(Event::Heard(event["delta"].as_str().unwrap_or("").to_owned()));
                                }
                                "session.output_transcript.delta" => {
                                    let _ = self.events.send(Event::Spoke(event["delta"].as_str().unwrap_or("").to_owned()));
                                }
                                "session.delegation.created" => {
                                    if let Some(id) = event["delegation"]["id"].as_str() {
                                        let _ = self.events.send(Event::Delegated(id.to_owned()));
                                    }
                                }
                                "session.usage.updated" => {
                                    if let Some(ratio) = event["context_window"]["usage_ratio"].as_f64() {
                                        let _ = self.events.send(Event::ContextUsage(ratio));
                                    }
                                }
                                "session.closed" => return Ok("Session ended".into()),
                                "error" => return Err(format!("Live protocol error: {}", event["error"]["message"].as_str().unwrap_or("unknown"))),
                                _ => {}
                            }
                        }
                        RtcEvent::ChannelClose(id) if id == channel => return Ok("Session ended".into()),
                        _ => {}
                    },
                }
            };
            if stopped(&self.stop) {
                return Ok("dismissed".into());
            }
            if !open && started.elapsed() >= START_TIMEOUT {
                return Err("Live session timed out".into());
            }
            self.follow_mute(audio, capturing)?;
            if !*capturing {
                ear.reset();
            }
            loop {
                match frames.try_recv() {
                    Ok(Err(error)) => return Ok(format!("the relay closed: {error}")),
                    Err(mpsc::TryRecvError::Disconnected) => return Ok("the relay closed".into()),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Ok(Ok(frame)) => match split(&frame) {
                        ("hub", body) => {
                            let _ = self.events.send(Event::Hub(body.to_vec()));
                        }
                        ("hub-error", body) => {
                            let value: Value = serde_json::from_slice(body).unwrap_or_default();
                            if let Some(id) = value["id"].as_str() {
                                let _ = self.events.send(Event::HubError { id: id.to_owned(), message: message(body) });
                            }
                        }
                        _ => {}
                    },
                }
            }
            for command in self.commands.try_iter() {
                match command {
                    Command::Frame(verb, body) => {
                        if let Err(error) = relay.post(verb, Some(&body)) {
                            return Ok(format!("the relay closed: {error}"));
                        }
                    }
                    Command::Tell(event) => match rtc.channel(channel).filter(|_| open) {
                        Some(mut channel) => {
                            channel.write(false, event.to_string().as_bytes()).map_err(|error| format!("event channel: {error}"))?;
                        }
                        None => eprintln!("not told, the event channel is not open: {}", event["event_id"]),
                    },
                }
            }
            let now = Instant::now();
            if now >= next_tick {
                next_tick = (next_tick + TICK).max(now - TICK);
                let mut frame = [0f32; FRAME];
                {
                    let mut captured = audio.captured.lock().unwrap();
                    let stale = captured.len().saturating_sub(BACKLOG);
                    captured.drain(..stale);
                    if captured.len() >= FRAME {
                        for (slot, sample) in frame.iter_mut().zip(captured.drain(..FRAME)) {
                            *slot = sample;
                        }
                    }
                }
                if *capturing {
                    ear.hear(&mut frame);
                }
                let length = encoder.encode_float(&frame, &mut packet).map_err(|error| format!("opus encode: {error}"))?;
                rtp_time += FRAME as u64;
                if connected {
                    if let Some(writer) = rtc.writer(mid) {
                        let pt = writer.payload_params().next().map(|params| params.pt());
                        if let Some(pt) = pt {
                            writer.write(pt, now, MediaTime::new(rtp_time, str0m::media::Frequency::FORTY_EIGHT_KHZ), packet[..length].to_vec()).map_err(|error| format!("audio send: {error}"))?;
                        }
                    }
                }
                continue;
            }
            let wait = timeout.min(next_tick).saturating_duration_since(now).min(POLL);
            if wait.is_zero() {
                rtc.handle_input(Input::Timeout(Instant::now())).map_err(|error| format!("peer connection: {error}"))?;
                continue;
            }
            socket.set_read_timeout(Some(wait)).map_err(|error| error.to_string())?;
            let input = match socket.recv_from(&mut buffer) {
                Ok((length, source)) => match buffer[..length].try_into() {
                    Ok(contents) => Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source, destination: local, contents }),
                    Err(_) => continue,
                },
                Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Input::Timeout(Instant::now()),
                Err(error) => return Err(format!("peer socket: {error}")),
            };
            rtc.handle_input(input).map_err(|error| format!("peer connection: {error}"))?;
        }
    }
}
