use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::audio;
use crate::conversation::Trace;
use crate::hub::{self, Hub};
use crate::identity::{identity, includes_phrase};
use crate::relay::{Relay, Token};
use crate::session::{self, Session};

/// The page's wait for quiet after the sign-off.
const SIGN_OFF_QUIET: Duration = Duration::from_secs(2);
/// The page's per-frame easing of the mouth toward its target.
const MOUTH_EASING: f32 = 0.65;

/// The open session and what the page tracks alongside it.
struct Awake {
    session: Session,
    opened: bool,
    said: String,
    activity: Instant,
    mouth: f32,
    hub: Hub,
}

/// The conversation side of the overlay: awake exactly while a session is open or opening.
pub struct Voice {
    token: Token,
    cache: PathBuf,
    trace: Trace,
    muted: bool,
    awake: Option<Awake>,
}

impl Voice {
    pub fn new(token: Token, cache: PathBuf) -> Voice {
        Voice { token, cache, trace: Trace::default(), muted: false, awake: None }
    }

    pub fn awake(&self) -> bool {
        self.awake.is_some()
    }

    pub fn summon(&mut self) {
        eprintln!("summoned, opening the session");
        let session = Session::open(&self.token, &self.cache, self.trace.wake(Instant::now()), self.muted);
        self.awake = Some(Awake { session, opened: false, said: String::new(), activity: Instant::now(), mouth: 0.0, hub: Hub::default() });
    }

    /// Ends the session and releases the microphone; an opened session becomes memory for the next.
    pub fn dismiss(&mut self, reason: &str) {
        if self.awake.take().is_some_and(|ended| ended.opened) {
            self.trace.slept(Instant::now());
        }
        eprintln!("{reason}, dormant");
    }

    /// The microphone stops or starts and nothing else changes; the choice holds across sessions.
    pub fn toggle_mute(&mut self) -> bool {
        self.muted = !self.muted;
        eprintln!("microphone {}", if self.muted { "muted" } else { "unmuted" });
        if let Some(awake) = &self.awake {
            awake.session.mute(self.muted);
        }
        self.muted
    }

    /// As the page's Reset: ends the session and forgets the conversation carried between sessions, here and on the server.
    pub fn reset(&mut self) {
        self.awake = None;
        self.trace = Trace::default();
        eprintln!("reset, dormant");
        let (token, cache) = (self.token.clone(), self.cache.clone());
        std::thread::spawn(move || match Relay::connect(&token, &cache).and_then(|mut relay| relay.forget()) {
            Ok(()) => eprintln!("the server forgot the conversation"),
            Err(error) => eprintln!("reset incomplete: {error}"),
        });
    }

    /// Takes the session's news into the trace, and goes dormant on the sign-off, inactivity, or a session ended from the other side.
    pub fn step(&mut self) {
        let Some(awake) = &mut self.awake else { return };
        let sign_off = &identity().sign_off;
        let mut ended = None;
        for event in awake.session.events() {
            match event {
                session::Event::Open => {
                    eprintln!("session open");
                    awake.opened = true;
                    awake.activity = Instant::now();
                }
                session::Event::Heard(delta) => {
                    eprintln!("heard {delta:?}");
                    self.trace.heard(&delta, Instant::now());
                    awake.hub.heard(&delta, Instant::now());
                    awake.activity = Instant::now();
                }
                session::Event::Spoke(delta) => {
                    eprintln!("spoke {delta:?}");
                    self.trace.spoke(&delta);
                    awake.hub.spoke();
                    awake.activity = Instant::now();
                    awake.said.push_str(&delta);
                    let keep = 4 * sign_off.len();
                    if let Some((cut, _)) = awake.said.char_indices().find(|&(at, _)| awake.said.len() - at <= keep) {
                        awake.said.drain(..cut);
                    }
                    if !awake.hub.sleeping() && includes_phrase(&awake.said, sign_off) {
                        eprintln!("sign-off heard");
                        awake.hub.sleep();
                        awake.session.listen_for_quiet();
                    }
                }
                session::Event::Delegated(id) => awake.hub.delegated(&id, &mut self.trace, Instant::now()),
                session::Event::Hub(body) => awake.hub.reply(&body, &mut self.trace, Instant::now()),
                session::Event::HubError { id, message } => awake.hub.failed(&id, &message),
                session::Event::ContextUsage(ratio) => awake.hub.usage(ratio),
                session::Event::Closed(reason) => {
                    ended = Some(format!("session closed: {reason}"));
                    break;
                }
            }
        }
        if awake.hub.take_listen() {
            awake.session.listen_for_quiet();
        }
        awake.hub.step(&mut self.trace, Instant::now(), awake.session.quiet(hub::TURN_QUIET));
        if awake.hub.take_listen() {
            awake.session.listen_for_quiet();
        }
        for command in awake.hub.drain() {
            awake.session.send(command);
        }
        if ended.is_none() && awake.hub.sleeping() && awake.session.quiet(SIGN_OFF_QUIET) {
            ended = Some("signed off".into());
        }
        if ended.is_none() && awake.opened && awake.activity.elapsed() >= identity().inactivity {
            ended = Some("inactivity".into());
        }
        if let Some(reason) = ended {
            self.dismiss(&reason);
        }
    }

    /// The mouth opening, easing toward what the reply's output energy asks for.
    pub fn mouth(&mut self) -> f32 {
        let Some(awake) = &mut self.awake else { return 0.0 };
        awake.mouth += (audio::mouth(awake.session.level()) - awake.mouth) * MOUTH_EASING;
        awake.mouth
    }
}
