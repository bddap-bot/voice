use font8x8::legacy::BASIC_LEGACY;

use crate::placement::Vec3;
use crate::voice::VoiceId;

pub const WIDTH: f32 = 0.2;
const PIXELS_PER_METRE: f32 = 2000.0;
const ROW: f32 = 0.022;
const GAP: f32 = 0.004;
const NAMES: usize = 5;
/// Dismiss, the microphone and reset, Voice ID, Learn my voice, the names, and the page arrows.
const ROWS: usize = NAMES + 5;
const FIRST_NAME: usize = 4;
pub const HEIGHT: f32 = ROWS as f32 * ROW + (ROWS + 1) as f32 * GAP;
pub const PIXELS: [u32; 2] = [(WIDTH * PIXELS_PER_METRE) as u32, (HEIGHT * PIXELS_PER_METRE) as u32];
const GLYPH_SCALE: u32 = 2;
const PADDING: u32 = 8;

/// Touch depths along the board's normal, positive toward the viewer.
const CONTACT: f32 = 0.01;
const RELEASE: f32 = 0.02;

const BACKGROUND: [u8; 4] = [18, 18, 22, 210];
const BUTTON: [u8; 4] = [58, 58, 68, 255];
const ACTIVE: [u8; 4] = [38, 104, 196, 255];
const PENDING: [u8; 4] = [150, 110, 30, 255];
const DISMISS: [u8; 4] = [150, 42, 42, 255];
const MUTED: [u8; 4] = [140, 47, 57, 255];
const INK: [u8; 4] = [240, 240, 240, 255];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Dismiss,
    Mute,
    Reset,
    VoiceId,
    Learn,
    Appearance(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Press(Press),
    Page(isize),
}

struct Button {
    action: Action,
    /// Left, bottom, right, top, in metres from the board's centre.
    rect: [f32; 4],
}

impl Button {
    fn contains(&self, x: f32, y: f32) -> bool {
        let [left, bottom, right, top] = self.rect;
        (left..=right).contains(&x) && (bottom..=top).contains(&y)
    }
}

pub struct Board {
    names: Vec<String>,
    pub active: usize,
    pub pending: Option<usize>,
    pub muted: bool,
    pub voice: VoiceId,
    page: usize,
    armed: bool,
    dirty: bool,
}

fn row(index: usize, left: f32, right: f32) -> [f32; 4] {
    let top = HEIGHT / 2.0 - GAP - index as f32 * (ROW + GAP);
    [left, top - ROW, right, top]
}

impl Board {
    pub fn new(names: Vec<String>, active: usize) -> Board {
        let mut board = Board { names, active, pending: None, muted: false, voice: VoiceId::default(), page: 0, armed: false, dirty: true };
        board.reset();
        board
    }

    /// As summoned: showing the active appearance's page, and a hand already at the board must withdraw before it presses.
    pub fn reset(&mut self) {
        self.page = self.active / NAMES;
        self.armed = false;
        self.dirty = true;
    }

    fn pages(&self) -> usize {
        self.names.len().div_ceil(NAMES).max(1)
    }

    fn buttons(&self) -> Vec<Button> {
        let (left, right) = (-WIDTH / 2.0 + GAP, WIDTH / 2.0 - GAP);
        let half = (right - left - GAP) / 2.0;
        let mut buttons = vec![
            Button { action: Action::Press(Press::Dismiss), rect: row(0, left, right) },
            Button { action: Action::Press(Press::Mute), rect: row(1, left, left + half) },
            Button { action: Action::Press(Press::Reset), rect: row(1, right - half, right) },
            Button { action: Action::Press(Press::Learn), rect: row(3, left, right) },
        ];
        if self.voice.stored {
            buttons.push(Button { action: Action::Press(Press::VoiceId), rect: row(2, left, right) });
        }
        let first = self.page * NAMES;
        for (slot, index) in (first..self.names.len().min(first + NAMES)).enumerate() {
            buttons.push(Button { action: Action::Press(Press::Appearance(index)), rect: row(FIRST_NAME + slot, left, right) });
        }
        if self.pages() > 1 {
            let arrow = (right - left - 2.0 * GAP) / 3.0;
            buttons.push(Button { action: Action::Page(-1), rect: row(FIRST_NAME + NAMES, left, left + arrow) });
            buttons.push(Button { action: Action::Page(1), rect: row(FIRST_NAME + NAMES, right - arrow, right) });
        }
        buttons
    }

    /// The right controller's touch point in the board's frame (x right, y up, z toward the viewer), or None while untracked.
    /// A press is the tip crossing the board's face from in front; it fires once, and the next needs the tip drawn back.
    /// Crossing the face's plane beside the board disarms it too, so sliding on from the edge presses nothing.
    pub fn touch(&mut self, tip: Option<Vec3>) -> Option<Press> {
        let Some([x, y, z]) = tip else {
            self.armed = false;
            return None;
        };
        if z > RELEASE {
            self.armed = true;
            return None;
        }
        if !self.armed || z > CONTACT {
            return None;
        }
        self.armed = false;
        match self.buttons().into_iter().find(|button| button.contains(x, y))?.action {
            Action::Press(press) => Some(press),
            Action::Page(step) => {
                self.page = (self.page as isize + step).rem_euclid(self.pages() as isize) as usize;
                self.dirty = true;
                None
            }
        }
    }

    pub fn mark(&mut self) {
        self.dirty = true;
    }

    /// The board's premultiplied RGBA image, top row first, when it changed since the last call.
    pub fn take_image(&mut self) -> Option<Vec<u8>> {
        std::mem::take(&mut self.dirty).then(|| self.image())
    }

    fn image(&self) -> Vec<u8> {
        let [width, height] = PIXELS;
        let mut image = Canvas { pixels: vec![0; (width * height * 4) as usize], width };
        image.fill([0, 0, width, height], BACKGROUND);
        for button in self.buttons() {
            let area = pixels(button.rect);
            let (fill, label) = match button.action {
                Action::Press(Press::Dismiss) => (DISMISS, "Dismiss".to_owned()),
                Action::Press(Press::Mute) if self.muted => (MUTED, "Unmute mic".to_owned()),
                Action::Press(Press::Mute) => (BUTTON, "Mute mic".to_owned()),
                Action::Press(Press::Reset) => (BUTTON, "Reset".to_owned()),
                Action::Press(Press::VoiceId) if self.voice.off => (BUTTON, "Voice ID off".to_owned()),
                Action::Press(Press::VoiceId) => (ACTIVE, "Voice ID on".to_owned()),
                Action::Press(Press::Learn) => match self.voice.learning {
                    Some(progress) => (PENDING, format!("Stop learning ({}%)", (progress * 100.0).round())),
                    None if self.voice.stored => (BUTTON, "Forget my voice".to_owned()),
                    None => (BUTTON, "Learn my voice".to_owned()),
                },
                Action::Press(Press::Appearance(index)) => (
                    if self.pending == Some(index) {
                        PENDING
                    } else if index == self.active {
                        ACTIVE
                    } else {
                        BUTTON
                    },
                    self.names[index].clone(),
                ),
                Action::Page(-1) => (BUTTON, "<".to_owned()),
                Action::Page(_) => (BUTTON, ">".to_owned()),
            };
            image.fill(area, fill);
            image.label(area, &label);
        }
        if self.pages() > 1 {
            image.label(pixels(row(FIRST_NAME + NAMES, -WIDTH / 2.0, WIDTH / 2.0)), &format!("{}/{}", self.page + 1, self.pages()));
        }
        image.pixels
    }
}

/// Metres from the board's centre to left, top, right, bottom pixels.
fn pixels([left, bottom, right, top]: [f32; 4]) -> [u32; 4] {
    let x = |x: f32| ((x + WIDTH / 2.0) * PIXELS_PER_METRE).round() as u32;
    let y = |y: f32| ((HEIGHT / 2.0 - y) * PIXELS_PER_METRE).round() as u32;
    [x(left), y(top), x(right), y(bottom)]
}

struct Canvas {
    pixels: Vec<u8>,
    width: u32,
}

impl Canvas {
    fn fill(&mut self, [left, top, right, bottom]: [u32; 4], color: [u8; 4]) {
        let alpha = color[3] as u32;
        let premultiplied = [0, 1, 2].map(|channel| (color[channel] as u32 * alpha / 255) as u8);
        for y in top..bottom {
            for x in left..right {
                let at = ((y * self.width + x) * 4) as usize;
                self.pixels[at..at + 3].copy_from_slice(&premultiplied);
                self.pixels[at + 3] = color[3];
            }
        }
    }

    /// One line centred in the area, cut short with ".." when it does not fit.
    fn label(&mut self, [left, top, right, bottom]: [u32; 4], text: &str) {
        let glyph = 8 * GLYPH_SCALE;
        let fits = ((right - left).saturating_sub(2 * PADDING) / glyph) as usize;
        let mut characters: Vec<char> = text.chars().map(|c| if c.is_ascii() && !c.is_ascii_control() { c } else { '?' }).collect();
        if characters.len() > fits {
            characters.truncate(fits.saturating_sub(2));
            while characters.last() == Some(&' ') {
                characters.pop();
            }
            characters.extend(['.', '.']);
        }
        let x0 = left + (right - left - characters.len() as u32 * glyph) / 2;
        let y0 = top + (bottom - top).saturating_sub(glyph) / 2;
        for (index, character) in characters.into_iter().enumerate() {
            for (line, bits) in BASIC_LEGACY[character as usize].iter().enumerate() {
                for column in 0..8 {
                    if bits >> column & 1 == 1 {
                        let x = x0 + index as u32 * glyph + column * GLYPH_SCALE;
                        let y = y0 + line as u32 * GLYPH_SCALE;
                        self.fill([x, y, x + GLYPH_SCALE, y + GLYPH_SCALE], INK);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::{below_wrist, local_tip, Pose, TIP};

    fn names(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("Avatar {index}")).collect()
    }

    fn rect_of(board: &Board, action: Action) -> [f32; 4] {
        board.buttons().into_iter().find(|button| button.action == action).unwrap().rect
    }

    fn centre(board: &Board, action: Action) -> [f32; 2] {
        let [left, bottom, right, top] = rect_of(board, action);
        [(left + right) / 2.0, (bottom + top) / 2.0]
    }

    /// A right controller's tip approaching from 10 cm in front, pressing `depth` into the face, holding, then withdrawing.
    fn poke(board: &mut Board, [x, y]: [f32; 2], depth: f32) -> Vec<Press> {
        let mut presses = Vec::new();
        let mut z = 0.1;
        let mut steps = Vec::new();
        while z > -depth {
            steps.push(z);
            z -= 0.004;
        }
        steps.extend([-depth; 20]);
        steps.extend(steps.clone().into_iter().rev());
        for (step, z) in steps.into_iter().enumerate() {
            let jitter = if step % 2 == 0 { 0.002 } else { -0.002 };
            presses.extend(board.touch(Some([x + jitter, y - jitter, z])));
        }
        presses
    }

    fn poke_on(board: &mut Board, action: Action, depth: f32) -> Vec<Press> {
        let at = centre(board, action);
        poke(board, at, depth)
    }

    #[test]
    fn touching_each_button_fires_its_action_exactly_once() {
        let mut board = Board::new(names(3), 0);
        for button in board.buttons() {
            let Action::Press(expected) = button.action else { unreachable!("one page has no arrows") };
            for depth in [0.0, 0.01, 0.03, 0.2] {
                assert_eq!(poke_on(&mut board, button.action, depth), [expected], "{expected:?} pressed {depth} m deep");
            }
        }
    }

    /// The right controller held with its pointing axis into the board, its tip `z` in front of `[x, y]` on the board's face.
    fn right_controller(board: &Pose, [x, y]: [f32; 2], z: f32) -> Pose {
        Pose { r: board.r, t: crate::placement::sub(board.apply([x, y, z]), board.rotate(TIP)) }
    }

    #[test]
    fn a_simulated_right_controller_presses_each_button_on_the_wrist_once_and_passing_near_presses_nothing() {
        let (yaw, pitch) = (0.6f32, -0.4f32);
        let left = Pose { r: [[yaw.cos(), pitch.sin() * yaw.sin(), pitch.cos() * yaw.sin()], [0.0, pitch.cos(), -pitch.sin()], [-yaw.sin(), pitch.sin() * yaw.cos(), pitch.cos() * yaw.cos()]], t: [-0.2, 1.1, -0.35] };
        let head = Pose { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0, 1.6, 0.0] };
        let pose = below_wrist(&left, &head, HEIGHT);
        let mut board = Board::new(names(3 * NAMES), 0);
        let tip = |right: &Pose| local_tip(&pose, right);
        for z in [0.05, 0.015] {
            for step in 0..=100 {
                for y in [-0.08, 0.0, 0.08] {
                    let right = right_controller(&pose, [-WIDTH + step as f32 * WIDTH / 50.0, y], z);
                    assert_eq!(board.touch(Some(tip(&right))), None);
                }
            }
        }
        for button in board.buttons() {
            let [left, bottom, right, top] = button.rect;
            let centre = [(left + right) / 2.0, (bottom + top) / 2.0];
            let mut presses = Vec::new();
            for step in (0..=60).chain((0..=60).rev()) {
                presses.extend(board.touch(Some(tip(&right_controller(&pose, centre, 0.1 - step as f32 * 0.002)))));
            }
            let expected = match button.action {
                Action::Press(press) => vec![press],
                Action::Page(_) => vec![],
            };
            assert_eq!(presses, expected, "{:?}", button.action);
        }
        assert_eq!(board.page, 0, "one press forward and one back");
    }

    #[test]
    fn passing_near_without_touching_fires_nothing() {
        let mut board = Board::new(names(12), 0);
        for z in [0.05, 0.02, 0.012] {
            for step in 0..=200 {
                let x = -WIDTH + step as f32 * WIDTH / 100.0;
                for y in [-0.06, 0.0, 0.07] {
                    assert_eq!(board.touch(Some([x, y, z])), None, "({x}, {y}, {z})");
                }
            }
        }
        assert_eq!(board.page, 0);
    }

    #[test]
    fn a_touch_from_behind_or_beside_the_board_fires_nothing() {
        let mut board = Board::new(names(3), 0);
        let [x, y] = centre(&board, Action::Press(Press::Dismiss));
        for z in [-0.2, -0.05, -0.02, 0.0] {
            assert_eq!(board.touch(Some([x, y, z])), None);
        }
        board.touch(Some([x, y, 0.1]));
        assert_eq!(board.touch(Some([WIDTH, y, 0.0])), None, "beside the board");
        assert_eq!(board.touch(Some([x, y, 0.0])), None, "slid onto it while already in contact");
    }

    #[test]
    fn sliding_across_buttons_while_touching_fires_only_the_first() {
        let mut board = Board::new(names(3), 0);
        let [x, top] = centre(&board, Action::Press(Press::Dismiss));
        let [_, bottom] = centre(&board, Action::Press(Press::Appearance(2)));
        board.touch(Some([x, top, 0.1]));
        let mut presses = Vec::new();
        for step in 0..=50 {
            presses.extend(board.touch(Some([x, top + (bottom - top) * step as f32 / 50.0, 0.0])));
        }
        assert_eq!(presses, [Press::Dismiss]);
    }

    #[test]
    fn a_tip_trembling_at_the_face_presses_once() {
        let mut board = Board::new(names(3), 0);
        let [x, y] = centre(&board, Action::Press(Press::Dismiss));
        board.touch(Some([x, y, 0.1]));
        let presses: Vec<Press> = (0..40).filter_map(|step| board.touch(Some([x, y, if step % 2 == 0 { 0.005 } else { 0.018 }]))).collect();
        assert_eq!(presses, [Press::Dismiss]);
    }

    #[test]
    fn a_right_controller_reappearing_at_the_face_presses_nothing() {
        let mut board = Board::new(names(3), 0);
        let [x, y] = centre(&board, Action::Press(Press::Dismiss));
        board.touch(Some([x, y, 0.1]));
        assert_eq!(board.touch(None), None);
        assert_eq!(board.touch(Some([x, y, 0.0])), None);
    }

    #[test]
    fn summoning_with_the_hand_already_on_the_board_needs_a_fresh_touch() {
        let mut board = Board::new(names(3), 0);
        let [x, y] = centre(&board, Action::Press(Press::Dismiss));
        board.touch(Some([x, y, 0.1]));
        board.reset();
        assert_eq!(board.touch(Some([x, y, 0.0])), None);
        assert_eq!(poke(&mut board, [x, y], 0.0), [Press::Dismiss]);
    }

    #[test]
    fn the_arrows_page_through_every_appearance_once_per_touch() {
        let count = 2 * NAMES + 2;
        let mut board = Board::new(names(count), 0);
        let mut seen = Vec::new();
        for _ in 0..board.pages() {
            for slot in 0..NAMES {
                let action = board.buttons().into_iter().filter(|button| matches!(button.action, Action::Press(Press::Appearance(_)))).nth(slot).map(|button| button.action);
                if let Some(action) = action {
                    seen.extend(poke_on(&mut board, action, 0.0));
                }
            }
            assert!(poke_on(&mut board, Action::Page(1), 0.0).is_empty());
        }
        assert_eq!(seen, (0..count).map(Press::Appearance).collect::<Vec<_>>());
        assert_eq!(board.page, 0, "paging wraps around");
        poke_on(&mut board, Action::Page(-1), 0.0);
        assert_eq!(board.page, 2);
    }

    #[test]
    fn a_single_page_shows_no_arrows() {
        let board = Board::new(names(NAMES), 0);
        assert!(board.buttons().iter().all(|button| !matches!(button.action, Action::Page(_))));
    }

    #[test]
    fn summoning_shows_the_page_of_the_active_appearance() {
        let board = Board::new(names(3 * NAMES), NAMES + 1);
        assert_eq!(board.page, 1);
    }

    #[test]
    fn the_image_draws_each_button_with_its_label_and_the_active_one_highlighted() {
        let mut board = Board::new(vec!["A".into(), "Bee".into(), "x".repeat(80)], 1);
        let image = board.take_image().unwrap();
        assert_eq!(image.len(), (PIXELS[0] * PIXELS[1] * 4) as usize);
        assert!(board.take_image().is_none(), "unchanged");
        let pixel = |x: f32, y: f32| {
            let (column, line) = (((x + WIDTH / 2.0) * PIXELS_PER_METRE) as u32, ((HEIGHT / 2.0 - y) * PIXELS_PER_METRE) as u32);
            let at = ((line * PIXELS[0] + column) * 4) as usize;
            <[u8; 4]>::try_from(&image[at..at + 4]).unwrap()
        };
        let corner = |action| {
            let [left, _, _, top] = board.buttons().into_iter().find(|button| button.action == action).unwrap().rect;
            pixel(left + 0.001, top - 0.001)
        };
        assert_eq!(corner(Action::Press(Press::Dismiss)), DISMISS);
        assert_eq!(corner(Action::Press(Press::Appearance(0))), BUTTON);
        assert_eq!(corner(Action::Press(Press::Appearance(1))), ACTIVE);
        assert_eq!(corner(Action::Press(Press::Mute)), BUTTON);
        assert_eq!(corner(Action::Press(Press::Reset)), BUTTON);
        assert_eq!(pixel(0.0, -HEIGHT / 2.0 + 0.001)[3], BACKGROUND[3]);
        for index in 0..3 {
            let [left, bottom, right, top] = rect_of(&board, Action::Press(Press::Appearance(index)));
            let inked = (0..100).flat_map(|i| (0..20).map(move |j| (left + (right - left) * i as f32 / 100.0, bottom + (top - bottom) * j as f32 / 20.0))).filter(|&(x, y)| pixel(x, y) == INK).count();
            assert!(inked > 0, "appearance {index} carries a label");
        }
        board.pending = Some(2);
        board.mark();
        let image = board.take_image().unwrap();
        let [left, _, _, top] = rect_of(&board, Action::Press(Press::Appearance(2)));
        let at = ((((HEIGHT / 2.0 - top + 0.001) * PIXELS_PER_METRE) as u32 * PIXELS[0] + ((left + 0.001 + WIDTH / 2.0) * PIXELS_PER_METRE) as u32) * 4) as usize;
        assert_eq!(image[at..at + 4], PENDING);
        let inked = |image: &[u8], [left, top, right, bottom]: [u32; 4]| (top..bottom).flat_map(|y| (left..right).map(move |x| ((y * PIXELS[0] + x) * 4) as usize)).filter(|&at| image[at..at + 4] == INK).count();
        let mute = pixels(rect_of(&board, Action::Press(Press::Mute)));
        let unmuted = inked(&image, mute);
        board.muted = true;
        board.mark();
        let image = board.take_image().unwrap();
        let [left, _, _, top] = rect_of(&board, Action::Press(Press::Mute));
        let at = ((((HEIGHT / 2.0 - top + 0.001) * PIXELS_PER_METRE) as u32 * PIXELS[0] + ((left + 0.001 + WIDTH / 2.0) * PIXELS_PER_METRE) as u32) * 4) as usize;
        assert_eq!(image[at..at + 4], MUTED);
        assert_ne!(inked(&image, mute), unmuted, "the label says Unmute mic");
    }

    #[test]
    fn voice_id_shows_only_with_a_voiceprint_and_each_voice_button_presses_once() {
        let mut board = Board::new(names(3), 0);
        let has = |board: &Board, press| board.buttons().iter().any(|button| button.action == Action::Press(press));
        assert!(!has(&board, Press::VoiceId) && has(&board, Press::Learn));
        board.voice = VoiceId { stored: true, off: false, learning: None };
        for press in [Press::VoiceId, Press::Learn] {
            assert_eq!(poke_on(&mut board, Action::Press(press), 0.0), [press]);
        }
        let rect = |press| rect_of(&board, Action::Press(press));
        assert!(rect(Press::Mute)[1] > rect(Press::VoiceId)[3] && rect(Press::VoiceId)[1] > rect(Press::Learn)[3] && rect(Press::Learn)[1] > rect(Press::Appearance(0))[3]);
    }

    #[test]
    fn the_voice_buttons_say_what_a_press_will_do() {
        let ink = |voice: VoiceId, press| {
            let mut board = Board::new(names(3), 0);
            board.voice = voice;
            let image = board.take_image().unwrap();
            let [left, top, right, bottom] = pixels(rect_of(&board, Action::Press(press)));
            let fill = <[u8; 4]>::try_from(&image[((top * PIXELS[0] + left) * 4) as usize..][..4]).unwrap();
            let inked: Vec<bool> = (top..bottom).flat_map(|y| (left..right).map(move |x| ((y * PIXELS[0] + x) * 4) as usize)).map(|at| image[at..at + 4] == INK).collect();
            (fill, inked)
        };
        let none = VoiceId::default();
        let stored = VoiceId { stored: true, ..none };
        let off = VoiceId { off: true, ..stored };
        let learning = VoiceId { learning: Some(0.42), ..none };
        assert_eq!(ink(stored, Press::VoiceId).0, ACTIVE);
        assert_eq!(ink(off, Press::VoiceId).0, BUTTON);
        assert_ne!(ink(stored, Press::VoiceId).1, ink(off, Press::VoiceId).1, "Voice ID on, Voice ID off");
        assert_eq!(ink(learning, Press::Learn).0, PENDING);
        let labels = [ink(none, Press::Learn).1, ink(stored, Press::Learn).1, ink(learning, Press::Learn).1, ink(VoiceId { learning: Some(0.5), ..none }, Press::Learn).1];
        for (index, label) in labels.iter().enumerate() {
            assert!(label.iter().any(|&inked| inked));
            assert!(labels[index + 1..].iter().all(|other| other != label), "Learn my voice, Forget my voice, and the progress each differ");
        }
        let mut board = Board::new(names(3), 0);
        board.voice = learning;
        let [left, _, right, _] = pixels(rect_of(&board, Action::Press(Press::Learn)));
        assert!((right - left - 2 * PADDING) / (8 * GLYPH_SCALE) >= "Stop learning (100%)".len() as u32, "the longest label fits");
    }

    #[test]
    fn the_microphone_and_reset_share_a_row_without_overlapping() {
        let board = Board::new(names(3), 0);
        let rect = |press| board.buttons().into_iter().find(|button| button.action == Action::Press(press)).unwrap().rect;
        let ([_, mute_bottom, mute_right, mute_top], [reset_left, reset_bottom, _, reset_top]) = (rect(Press::Mute), rect(Press::Reset));
        assert!(mute_right < reset_left);
        assert_eq!((mute_bottom, mute_top), (reset_bottom, reset_top));
        assert!(rect(Press::Dismiss)[1] > mute_top && mute_bottom > rect(Press::Appearance(0))[3]);
    }
}
