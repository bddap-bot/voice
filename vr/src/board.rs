use font8x8::legacy::BASIC_LEGACY;

use crate::placement::Vec3;
use crate::voice::VoiceId;

pub const WIDTH: f32 = 0.2;
const PIXELS_PER_METRE: f32 = 2000.0;
const ROW: f32 = 0.022;
const GAP: f32 = 0.004;
/// Dismiss, the microphone and reset, Voice ID, and Learn my voice.
const ROWS: usize = 4;
const COLUMNS: usize = 6;
const CELL: f32 = (WIDTH - (COLUMNS + 1) as f32 * GAP) / COLUMNS as f32;
/// The highlight showing around a preview.
const BORDER: u32 = 4;
pub const PREVIEW: u32 = (CELL * PIXELS_PER_METRE) as u32 - 2 * BORDER;
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

struct Button {
    press: Press,
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
    previews: Vec<Option<Vec<u8>>>,
    height: f32,
    pub active: usize,
    pub pending: Option<usize>,
    pub muted: bool,
    pub voice: VoiceId,
    armed: bool,
    dirty: bool,
}

impl Board {
    pub fn new(appearances: usize, active: usize) -> Board {
        let lines = appearances.div_ceil(COLUMNS);
        let height = ROWS as f32 * ROW + lines as f32 * CELL + (ROWS + lines + 1) as f32 * GAP;
        Board { previews: vec![None; appearances], height, active, pending: None, muted: false, voice: VoiceId::default(), armed: false, dirty: true }
    }

    /// As summoned: a hand already at the board must withdraw before it presses.
    pub fn reset(&mut self) {
        self.armed = false;
        self.dirty = true;
    }

    /// The appearance's preview, `PREVIEW` pixels square, premultiplied RGBA, top row first.
    pub fn preview(&mut self, index: usize, rgba: Vec<u8>) {
        assert_eq!(rgba.len(), (PREVIEW * PREVIEW * 4) as usize, "one RGBA texel per preview pixel");
        self.previews[index] = Some(rgba);
        self.dirty = true;
    }

    pub fn height(&self) -> f32 {
        self.height
    }

    pub fn pixels(&self) -> [u32; 2] {
        [(WIDTH * PIXELS_PER_METRE).round() as u32, (self.height() * PIXELS_PER_METRE).round() as u32]
    }

    fn row(&self, index: usize, left: f32, right: f32) -> [f32; 4] {
        let top = self.height() / 2.0 - GAP - index as f32 * (ROW + GAP);
        [left, top - ROW, right, top]
    }

    fn cell(&self, index: usize) -> [f32; 4] {
        let top = self.height() / 2.0 - GAP - ROWS as f32 * (ROW + GAP) - (index / COLUMNS) as f32 * (CELL + GAP);
        let left = -WIDTH / 2.0 + GAP + (index % COLUMNS) as f32 * (CELL + GAP);
        [left, top - CELL, left + CELL, top]
    }

    fn buttons(&self) -> Vec<Button> {
        let (left, right) = (-WIDTH / 2.0 + GAP, WIDTH / 2.0 - GAP);
        let half = (right - left - GAP) / 2.0;
        let mut buttons = vec![
            Button { press: Press::Dismiss, rect: self.row(0, left, right) },
            Button { press: Press::Mute, rect: self.row(1, left, left + half) },
            Button { press: Press::Reset, rect: self.row(1, right - half, right) },
            Button { press: Press::Learn, rect: self.row(3, left, right) },
        ];
        if self.voice.stored {
            buttons.push(Button { press: Press::VoiceId, rect: self.row(2, left, right) });
        }
        buttons.extend((0..self.previews.len()).map(|index| Button { press: Press::Appearance(index), rect: self.cell(index) }));
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
        Some(self.buttons().into_iter().find(|button| button.contains(x, y))?.press)
    }

    pub fn mark(&mut self) {
        self.dirty = true;
    }

    /// The board's premultiplied RGBA image, top row first, when it changed since the last call.
    pub fn take_image(&mut self) -> Option<Vec<u8>> {
        std::mem::take(&mut self.dirty).then(|| self.image())
    }

    fn image(&self) -> Vec<u8> {
        let [width, height] = self.pixels();
        let mut image = Canvas { pixels: vec![0; (width * height * 4) as usize], width };
        image.fill([0, 0, width, height], BACKGROUND);
        for button in self.buttons() {
            let area = self.area(button.rect);
            let (fill, label) = match button.press {
                Press::Dismiss => (DISMISS, "Dismiss".to_owned()),
                Press::Mute if self.muted => (MUTED, "Unmute mic".to_owned()),
                Press::Mute => (BUTTON, "Mute mic".to_owned()),
                Press::Reset => (BUTTON, "Reset".to_owned()),
                Press::VoiceId if self.voice.off => (BUTTON, "Voice ID off".to_owned()),
                Press::VoiceId => (ACTIVE, "Voice ID on".to_owned()),
                Press::Learn => match self.voice.learning {
                    Some(progress) => (PENDING, format!("Stop learning ({}%)", (progress * 100.0).round())),
                    None if self.voice.stored => (BUTTON, "Forget my voice".to_owned()),
                    None => (BUTTON, "Learn my voice".to_owned()),
                },
                Press::Appearance(index) => {
                    let fill = if self.pending == Some(index) {
                        PENDING
                    } else if index == self.active {
                        ACTIVE
                    } else {
                        BUTTON
                    };
                    image.fill(area, fill);
                    let [left, top, right, bottom] = area;
                    let corner = [left + (right - left - PREVIEW) / 2, top + (bottom - top - PREVIEW) / 2];
                    image.fill([corner[0], corner[1], corner[0] + PREVIEW, corner[1] + PREVIEW], BUTTON);
                    if let Some(preview) = &self.previews[index] {
                        image.over(corner, PREVIEW, preview);
                    }
                    continue;
                }
            };
            image.fill(area, fill);
            image.label(area, &label);
        }
        image.pixels
    }

    /// Metres from the board's centre to left, top, right, bottom pixels.
    fn area(&self, [left, bottom, right, top]: [f32; 4]) -> [u32; 4] {
        let height = self.height();
        let x = |x: f32| ((x + WIDTH / 2.0) * PIXELS_PER_METRE).round() as u32;
        let y = |y: f32| ((height / 2.0 - y) * PIXELS_PER_METRE).round() as u32;
        [x(left), y(top), x(right), y(bottom)]
    }
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

    /// A premultiplied square image drawn over the canvas with its top left corner at `at`.
    fn over(&mut self, [x0, y0]: [u32; 2], size: u32, rgba: &[u8]) {
        for y in 0..size {
            for x in 0..size {
                let from = ((y * size + x) * 4) as usize;
                let to = (((y0 + y) * self.width + x0 + x) * 4) as usize;
                let keep = 255 - rgba[from + 3] as u32;
                for channel in 0..4 {
                    self.pixels[to + channel] = (rgba[from + channel] as u32 + self.pixels[to + channel] as u32 * keep / 255).min(255) as u8;
                }
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

    fn rect_of(board: &Board, press: Press) -> [f32; 4] {
        board.buttons().into_iter().find(|button| button.press == press).unwrap().rect
    }

    fn centre(board: &Board, press: Press) -> [f32; 2] {
        let [left, bottom, right, top] = rect_of(board, press);
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

    fn poke_on(board: &mut Board, press: Press, depth: f32) -> Vec<Press> {
        let at = centre(board, press);
        poke(board, at, depth)
    }

    fn pixel(board: &Board, image: &[u8], x: f32, y: f32) -> [u8; 4] {
        let [column, line, _, _] = board.area([x, y, x, y]);
        let at = ((line * board.pixels()[0] + column) * 4) as usize;
        image[at..at + 4].try_into().unwrap()
    }

    fn inked(board: &Board, image: &[u8], [left, top, right, bottom]: [u32; 4]) -> Vec<bool> {
        let width = board.pixels()[0];
        (top..bottom).flat_map(|y| (left..right).map(move |x| ((y * width + x) * 4) as usize)).map(|at| image[at..at + 4] == INK).collect()
    }

    #[test]
    fn touching_each_button_fires_its_action_exactly_once() {
        let mut board = Board::new(3, 0);
        for button in board.buttons() {
            for depth in [0.0, 0.01, 0.03, 0.2] {
                assert_eq!(poke_on(&mut board, button.press, depth), [button.press], "{:?} pressed {depth} m deep", button.press);
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
        let mut board = Board::new(36, 0);
        let pose = below_wrist(&left, &head, board.height());
        let tip = |right: &Pose| local_tip(&pose, right);
        for z in [0.05, 0.015] {
            for step in 0..=100 {
                for y in [-0.12, -0.08, 0.0, 0.08, 0.12] {
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
            assert_eq!(presses, [button.press]);
        }
    }

    #[test]
    fn passing_near_without_touching_fires_nothing() {
        let mut board = Board::new(12, 0);
        for z in [0.05, 0.02, 0.012] {
            for step in 0..=200 {
                let x = -WIDTH + step as f32 * WIDTH / 100.0;
                for y in [-0.06, 0.0, 0.07] {
                    assert_eq!(board.touch(Some([x, y, z])), None, "({x}, {y}, {z})");
                }
            }
        }
    }

    #[test]
    fn a_touch_from_behind_or_beside_the_board_fires_nothing() {
        let mut board = Board::new(3, 0);
        let [x, y] = centre(&board, Press::Dismiss);
        for z in [-0.2, -0.05, -0.02, 0.0] {
            assert_eq!(board.touch(Some([x, y, z])), None);
        }
        board.touch(Some([x, y, 0.1]));
        assert_eq!(board.touch(Some([WIDTH, y, 0.0])), None, "beside the board");
        assert_eq!(board.touch(Some([x, y, 0.0])), None, "slid onto it while already in contact");
    }

    #[test]
    fn sliding_across_buttons_while_touching_fires_only_the_first() {
        let mut board = Board::new(3, 0);
        let [x, top] = centre(&board, Press::Dismiss);
        let [_, bottom] = centre(&board, Press::Appearance(0));
        board.touch(Some([x, top, 0.1]));
        let mut presses = Vec::new();
        for step in 0..=50 {
            presses.extend(board.touch(Some([x, top + (bottom - top) * step as f32 / 50.0, 0.0])));
        }
        assert_eq!(presses, [Press::Dismiss]);
    }

    #[test]
    fn a_tip_trembling_at_the_face_presses_once() {
        let mut board = Board::new(3, 0);
        let [x, y] = centre(&board, Press::Dismiss);
        board.touch(Some([x, y, 0.1]));
        let presses: Vec<Press> = (0..40).filter_map(|step| board.touch(Some([x, y, if step % 2 == 0 { 0.005 } else { 0.018 }]))).collect();
        assert_eq!(presses, [Press::Dismiss]);
    }

    #[test]
    fn a_right_controller_reappearing_at_the_face_presses_nothing() {
        let mut board = Board::new(3, 0);
        let [x, y] = centre(&board, Press::Dismiss);
        board.touch(Some([x, y, 0.1]));
        assert_eq!(board.touch(None), None);
        assert_eq!(board.touch(Some([x, y, 0.0])), None);
    }

    #[test]
    fn summoning_with_the_hand_already_on_the_board_needs_a_fresh_touch() {
        let mut board = Board::new(3, 0);
        let [x, y] = centre(&board, Press::Dismiss);
        board.touch(Some([x, y, 0.1]));
        board.reset();
        assert_eq!(board.touch(Some([x, y, 0.0])), None);
        assert_eq!(poke(&mut board, [x, y], 0.0), [Press::Dismiss]);
    }

    #[test]
    fn every_appearance_has_its_own_cell_on_the_board_at_once() {
        for count in [1, COLUMNS - 1, COLUMNS, COLUMNS + 1, 36, 37] {
            let mut board = Board::new(count, 0);
            let half = [WIDTH / 2.0, board.height() / 2.0];
            let cells: Vec<[f32; 4]> = (0..count).map(|index| rect_of(&board, Press::Appearance(index))).collect();
            for (index, &[left, bottom, right, top]) in cells.iter().enumerate() {
                assert!(left >= -half[0] && right <= half[0] && bottom >= -half[1] && top <= half[1], "cell {index} of {count} lies on the board");
                assert!((right - left - (top - bottom)).abs() < 1e-6, "square");
                assert!(top < rect_of(&board, Press::Learn)[1], "below the buttons");
                for other in &cells[index + 1..] {
                    assert!(other[0] > right || other[2] < left || other[1] > top || other[3] < bottom, "cells {index} and another of {count} overlap");
                }
            }
            let presses: Vec<Press> = (0..count).flat_map(|index| poke_on(&mut board, Press::Appearance(index), 0.0)).collect();
            assert_eq!(presses, (0..count).map(Press::Appearance).collect::<Vec<_>>());
        }
    }

    #[test]
    fn each_cell_shows_its_preview_framed_in_the_cell_highlight() {
        let mut board = Board::new(3, 1);
        let opaque: Vec<u8> = (0..PREVIEW * PREVIEW).flat_map(|_| [200, 10, 90, 255]).collect();
        let half: Vec<u8> = (0..PREVIEW * PREVIEW).flat_map(|texel| if texel % PREVIEW < PREVIEW / 2 { [0, 0, 0, 0] } else { [100, 0, 0, 128] }).collect();
        board.preview(0, opaque);
        board.preview(1, half);
        board.pending = Some(2);
        let image = board.take_image().unwrap();
        let at = |index, u: f32, v: f32| {
            let [left, bottom, right, top] = rect_of(&board, Press::Appearance(index));
            pixel(&board, &image, left + (right - left) * u, top - (top - bottom) * v)
        };
        assert_eq!(at(0, 0.5, 0.5), [200, 10, 90, 255], "the preview");
        assert_eq!(at(0, 0.02, 0.5), BUTTON, "framed");
        assert_eq!(at(1, 0.02, 0.5), ACTIVE, "the shown appearance's frame");
        assert_eq!(at(1, 0.3, 0.5), BUTTON, "a transparent texel shows the cell");
        assert_eq!(at(1, 0.7, 0.5), [0, 1, 2, 3].map(|channel| ([100, 0, 0, 128][channel] + BUTTON[channel] as u32 * 127 / 255) as u8), "a translucent one blends over it");
        assert_eq!(at(2, 0.02, 0.5), PENDING, "the picked appearance's frame");
        assert_eq!(at(2, 0.5, 0.5), BUTTON, "loading, with no preview yet");
    }

    #[test]
    fn the_image_draws_each_button_with_its_label() {
        let mut board = Board::new(3, 1);
        let image = board.take_image().unwrap();
        let [width, height] = board.pixels();
        assert_eq!(image.len(), (width * height * 4) as usize);
        assert!(board.take_image().is_none(), "unchanged");
        let corner = |board: &Board, image: &[u8], press| {
            let [left, _, _, top] = rect_of(board, press);
            pixel(board, image, left + 0.001, top - 0.001)
        };
        assert_eq!(corner(&board, &image, Press::Dismiss), DISMISS);
        assert_eq!(corner(&board, &image, Press::Mute), BUTTON);
        assert_eq!(corner(&board, &image, Press::Reset), BUTTON);
        assert_eq!(pixel(&board, &image, 0.0, -board.height() / 2.0 + 0.001)[3], BACKGROUND[3]);
        for press in [Press::Dismiss, Press::Mute, Press::Reset, Press::Learn] {
            assert!(inked(&board, &image, board.area(rect_of(&board, press))).iter().any(|&inked| inked), "{press:?} carries a label");
        }
        let mute = board.area(rect_of(&board, Press::Mute));
        let unmuted = inked(&board, &image, mute);
        board.muted = true;
        board.mark();
        let image = board.take_image().unwrap();
        assert_eq!(corner(&board, &image, Press::Mute), MUTED);
        assert_ne!(inked(&board, &image, mute), unmuted, "the label says Unmute mic");
    }

    #[test]
    fn voice_id_shows_only_with_a_voiceprint_and_each_voice_button_presses_once() {
        let mut board = Board::new(3, 0);
        let has = |board: &Board, press| board.buttons().iter().any(|button| button.press == press);
        assert!(!has(&board, Press::VoiceId) && has(&board, Press::Learn));
        board.voice = VoiceId { stored: true, off: false, learning: None };
        for press in [Press::VoiceId, Press::Learn] {
            assert_eq!(poke_on(&mut board, press, 0.0), [press]);
        }
        let rect = |press| rect_of(&board, press);
        assert!(rect(Press::Mute)[1] > rect(Press::VoiceId)[3] && rect(Press::VoiceId)[1] > rect(Press::Learn)[3] && rect(Press::Learn)[1] > rect(Press::Appearance(0))[3]);
    }

    #[test]
    fn the_voice_buttons_say_what_a_press_will_do() {
        let ink = |voice: VoiceId, press| {
            let mut board = Board::new(3, 0);
            board.voice = voice;
            let image = board.take_image().unwrap();
            let area = board.area(rect_of(&board, press));
            let fill = <[u8; 4]>::try_from(&image[((area[1] * board.pixels()[0] + area[0]) * 4) as usize..][..4]).unwrap();
            (fill, inked(&board, &image, area))
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
        let mut board = Board::new(3, 0);
        board.voice = learning;
        let [left, _, right, _] = board.area(rect_of(&board, Press::Learn));
        assert!((right - left - 2 * PADDING) / (8 * GLYPH_SCALE) >= "Stop learning (100%)".len() as u32, "the longest label fits");
    }

    #[test]
    fn the_microphone_and_reset_share_a_row_without_overlapping() {
        let board = Board::new(3, 0);
        let rect = |press| rect_of(&board, press);
        let ([_, mute_bottom, mute_right, mute_top], [reset_left, reset_bottom, _, reset_top]) = (rect(Press::Mute), rect(Press::Reset));
        assert!(mute_right < reset_left);
        assert_eq!((mute_bottom, mute_top), (reset_bottom, reset_top));
        assert!(rect(Press::Dismiss)[1] > mute_top && mute_bottom > rect(Press::Appearance(0))[3]);
    }
}
