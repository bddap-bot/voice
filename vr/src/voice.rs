use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio;
use crate::conversation::Trace;
use crate::hub::{self, Hub};
use crate::identity::{identity, includes_phrase};
use crate::relay::{Relay, Token};
use crate::session::{self, Session};
use crate::speaker::{self, Hearing, Mode, Report, Stored};

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
    voiceprint: PathBuf,
    stored: Option<Stored>,
    learning: Option<f32>,
    hearing: Arc<Mutex<Hearing>>,
}

/// What the board shows of Voice ID and Learn my voice.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VoiceId {
    pub stored: bool,
    pub off: bool,
    pub learning: Option<f32>,
}

impl Voice {
    pub fn new(token: Token, cache: PathBuf, voiceprint: PathBuf) -> Voice {
        let stored = Stored::read(&voiceprint);
        let mut voice = Voice { token, cache, trace: Trace::default(), muted: false, awake: None, voiceprint, stored, learning: None, hearing: Arc::default() };
        voice.refilter();
        voice
    }

    pub fn voice_id(&self) -> VoiceId {
        VoiceId { stored: self.stored.is_some(), off: self.stored.as_ref().is_some_and(|stored| stored.off), learning: self.learning }
    }

    /// Points the microphone at the enrollment, the filter for an active voiceprint, or Live directly.
    fn refilter(&mut self) {
        let mode = match (&self.learning, &self.stored) {
            (Some(_), _) => Mode::Learn,
            (None, Some(stored)) if !stored.off => Mode::Filter(stored.print.as_slice().into()),
            _ => Mode::Open,
        };
        if mode != Mode::Open {
            speaker::prepare();
        }
        self.hearing.lock().unwrap().set(mode);
    }

    /// The page's Voice ID button: the stored voiceprint filters or not, and the choice is stored with it.
    pub fn toggle_voice_id(&mut self) {
        let Some(stored) = &mut self.stored else { return };
        stored.off = !stored.off;
        eprintln!("{}", if stored.off { "voice ID off: conversations hear everyone" } else { "voice ID on: other voices are filtered out" });
        Stored::write(self.stored.as_ref(), &self.voiceprint);
        self.refilter();
    }

    /// The page's Learn my voice button: stops a running enrollment, forgets a stored voiceprint, or starts learning.
    /// While it learns, Live hears silence.
    pub fn toggle_learning(&mut self) {
        if self.learning.is_some() {
            self.stop_learning("voice learning stopped");
        } else if self.stored.is_some() {
            self.stored = None;
            Stored::write(None, &self.voiceprint);
            eprintln!("voice forgotten: conversations hear everyone");
        } else if self.muted {
            eprintln!("unmute the microphone to learn your voice");
        } else {
            self.learning = Some(0.0);
            eprintln!("learning your voice: talk normally for about ten seconds");
        }
        self.refilter();
    }

    fn stop_learning(&mut self, reason: &str) {
        if self.learning.take().is_some() {
            eprintln!("{reason}");
            self.refilter();
        }
    }

    /// Takes the filter's reports: enrollment progress, a learned voiceprint, or a failure.
    fn take_reports(&mut self) {
        let reports = std::mem::take(&mut self.hearing.lock().unwrap().reports);
        for report in reports {
            match report {
                Report::Progress(progress) if self.learning.is_some() => self.learning = Some(progress),
                Report::Learned(print) if self.learning.is_some() => {
                    self.stored = Some(Stored { model: speaker::page().sha256.to_owned(), print, off: false });
                    Stored::write(self.stored.as_ref(), &self.voiceprint);
                    self.stop_learning("voice learned: other voices are filtered out");
                }
                Report::Error(error) if self.learning.is_some() => self.stop_learning(&format!("could not learn your voice: {error}")),
                _ => {}
            }
        }
    }

    pub fn awake(&self) -> bool {
        self.awake.is_some()
    }

    pub fn muted(&self) -> bool {
        self.muted
    }

    /// The newest display a hub reply carried this session, once.
    pub fn take_display(&mut self) -> Option<hub::Display> {
        self.awake.as_mut()?.hub.take_display()
    }

    pub fn summon(&mut self) {
        eprintln!("summoned, opening the session");
        let session = Session::open(&self.token, &self.cache, self.trace.wake(Instant::now()), self.muted, self.hearing.clone());
        self.awake = Some(Awake { session, opened: false, said: String::new(), activity: Instant::now(), mouth: 0.0, hub: Hub::default() });
    }

    /// Ends the session and releases the microphone; an opened session becomes memory for the next.
    pub fn dismiss(&mut self, reason: &str) {
        if self.awake.take().is_some_and(|ended| ended.opened) {
            self.trace.slept(Instant::now());
        }
        self.stop_learning("voice learning stopped");
        eprintln!("{reason}, dormant");
    }

    /// The microphone stops or starts and nothing else changes; the choice holds across sessions.
    pub fn toggle_mute(&mut self) -> bool {
        self.muted = !self.muted;
        eprintln!("microphone {}", if self.muted { "muted" } else { "unmuted" });
        if self.muted {
            self.stop_learning("voice learning stopped");
        }
        if let Some(awake) = &self.awake {
            awake.session.mute(self.muted);
        }
        self.muted
    }

    /// As the page's Reset: ends the session and forgets the conversation carried between sessions, here and on the server.
    pub fn reset(&mut self) {
        self.awake = None;
        self.stop_learning("voice learning stopped");
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
        self.take_reports();
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

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn voice(name: &str) -> (Voice, PathBuf) {
        let directory = std::env::temp_dir().join(format!("voice-vr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let token = Token::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"endpoint_id":"abc","secret":"s"}"#)).unwrap();
        let path = directory.join("voiceprint.json");
        (Voice::new(token, directory.join("assets"), path.clone()), path)
    }

    fn mode(voice: &Voice) -> Mode {
        voice.hearing.lock().unwrap().mode().clone()
    }

    fn report(voice: &mut Voice, report: Report) {
        voice.hearing.lock().unwrap().reports.push(report);
        voice.step();
    }

    #[test]
    fn learn_my_voice_records_a_voiceprint_that_then_filters() {
        let (mut voice, path) = voice("learn");
        assert_eq!((voice.voice_id(), mode(&voice)), (VoiceId::default(), Mode::Open));
        voice.toggle_learning();
        assert_eq!((voice.voice_id().learning, mode(&voice)), (Some(0.0), Mode::Learn));
        report(&mut voice, Report::Progress(0.5));
        assert_eq!(voice.voice_id().learning, Some(0.5));
        report(&mut voice, Report::Learned(vec![0.6, 0.8]));
        assert_eq!(voice.voice_id(), VoiceId { stored: true, off: false, learning: None });
        assert_eq!(mode(&voice), Mode::Filter([0.6, 0.8].as_slice().into()));
        assert_eq!(Stored::read(&path).unwrap().print, [0.6, 0.8]);

        voice.toggle_voice_id();
        assert_eq!((voice.voice_id().off, mode(&voice), Stored::read(&path).unwrap().off), (true, Mode::Open, true));
        voice.toggle_voice_id();
        assert_eq!((voice.voice_id().off, Stored::read(&path).unwrap().off), (false, false));
        assert!(matches!(mode(&voice), Mode::Filter(_)));

        let reloaded = Voice::new(voice.token.clone(), voice.cache.clone(), path.clone());
        assert_eq!((reloaded.voice_id(), mode(&reloaded)), (voice.voice_id(), mode(&voice)), "the voiceprint is kept across runs");

        voice.toggle_learning();
        assert_eq!((voice.voice_id(), mode(&voice), path.exists()), (VoiceId::default(), Mode::Open, false), "a stored voice is forgotten");
        voice.toggle_voice_id();
        assert_eq!(voice.voice_id(), VoiceId::default(), "Voice ID needs a voiceprint");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn learning_stops_on_its_button_on_mute_and_on_dormancy_and_waits_for_the_microphone() {
        let (mut voice, path) = voice("stop");
        voice.toggle_learning();
        voice.toggle_learning();
        assert_eq!((voice.voice_id().learning, mode(&voice)), (None, Mode::Open));
        voice.toggle_learning();
        voice.toggle_mute();
        assert_eq!((voice.voice_id().learning, mode(&voice)), (None, Mode::Open));
        voice.toggle_learning();
        assert_eq!(voice.voice_id().learning, None, "unmute the microphone to learn");
        voice.toggle_mute();
        voice.toggle_learning();
        voice.dismiss("dismissed");
        assert_eq!((voice.voice_id().learning, mode(&voice)), (None, Mode::Open));
        report(&mut voice, Report::Learned(vec![1.0]));
        assert!(!voice.voice_id().stored && !path.exists(), "a voiceprint finished after learning stopped is dropped");
        voice.toggle_learning();
        report(&mut voice, Report::Error("the model is missing".into()));
        assert_eq!((voice.voice_id().learning, mode(&voice)), (None, Mode::Open));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
