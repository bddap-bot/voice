use std::time::{Duration, Instant};

use font8x8::legacy::BASIC_LEGACY;

use std::collections::HashMap;

use crate::calls::State;
use crate::conversation::Entry;
use crate::placement::{Anchor, Vec3};
use crate::voice::VoiceId;
use crate::vrm::expressions::{Control, Kind};
use crate::vrm::Image;

pub const WIDTH: f32 = 0.2;
/// How long the appearance library stays after a reveal.
pub const LIBRARY_SHOWN: Duration = Duration::from_secs(60);
const PIXELS_PER_METRE: f32 = 2000.0;
const ROW: f32 = 0.022;
const GAP: f32 = 0.004;
const ROWS: usize = 5;
const PAD_ROWS: usize = 4;
const MENU_GLYPH_SCALE: u32 = 1;
const ROW_PIXELS: u32 = (ROW * PIXELS_PER_METRE) as u32;
const COLUMNS: usize = 6;
const CELL: f32 = (WIDTH - (COLUMNS + 1) as f32 * GAP) / COLUMNS as f32;
/// The highlight showing around a preview.
const BORDER: u32 = 4;
pub const PREVIEW: u32 = (CELL * PIXELS_PER_METRE) as u32 - 2 * BORDER;
const GLYPH_SCALE: u32 = 2;
const PADDING: u32 = 8;
/// The log's text is drawn at the font's own size.
const LINE: u32 = 10;
/// The log's height in lines; it shows the newest that fit.
pub const LOG_LINES: usize = 12;
const STRIPE: u32 = 4;
const REQUEST_LINES: usize = 2;
const REPLY_LINES: usize = 3;

/// Touch depths along the board's normal, positive toward the viewer.
const CONTACT: f32 = 0.01;
const RELEASE: f32 = 0.02;
const NEAR: f32 = 0.1;
pub const MARKER_WIDTH: f32 = 0.008;
pub const MARKER_PIXELS: u32 = 32;

const BACKGROUND: [u8; 4] = [18, 18, 22, 210];
const BUTTON: [u8; 4] = [58, 58, 68, 255];
const ACTIVE: [u8; 4] = [38, 104, 196, 255];
const PENDING: [u8; 4] = [150, 110, 30, 255];
const DISMISS: [u8; 4] = [150, 42, 42, 255];
const MUTED: [u8; 4] = [140, 47, 57, 255];
const INK: [u8; 4] = [240, 240, 240, 255];
const HOVER: [u8; 4] = [255, 214, 90, 255];
const REPLY_INK: [u8; 4] = [170, 200, 240, 255];
const HEARD: [u8; 4] = [64, 168, 96, 255];
const SPOKE_INK: [u8; 4] = [190, 190, 200, 255];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Dismiss,
    Mute,
    Reset,
    VoiceId,
    Learn,
    Log,
    SetDown,
    Appearance(usize),
    Menu,
    Back,
    Control(usize),
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

/// The marker is the tip held at the face once through it, shown only over the board.
pub struct Touch {
    /// Where the tip crossed the face from in front, on or beside a button.
    pub crossing: Option<[f32; 2]>,
    pub press: Option<Press>,
    pub marker: Option<Vec3>,
}

/// A fingertip pressing a face: a press is the tip crossing the face from in front; it fires once, and the next needs the tip drawn back.
#[derive(Default)]
pub struct Contact {
    armed: bool,
}

impl Contact {
    pub fn reset(&mut self) {
        self.armed = false;
    }

    /// Where the tip, in the face's frame (x right, y up, z toward the viewer), crossed the face's plane, anywhere on it.
    pub fn crossing(&mut self, tip: Option<Vec3>) -> Option<[f32; 2]> {
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
        Some([x, y])
    }
}

/// The tip held at a face of `[width, height]` once through it, while over the face and near or through it.
pub fn marker_at([width, height]: [f32; 2], tip: Option<Vec3>) -> Option<Vec3> {
    tip.filter(|&[x, y, z]| x.abs() <= width / 2.0 && y.abs() <= height / 2.0 && z <= NEAR).map(|[x, y, z]| [x, y, z.max(0.0)])
}

struct Menu {
    controls: Vec<Control>,
    icons: Vec<Image>,
    path: Vec<usize>,
    open: bool,
    values: HashMap<String, f32>,
}

impl Menu {
    fn page(&self) -> &[Control] {
        self.path.iter().fold(&self.controls, |controls, &at| &controls[at].controls)
    }
}

pub struct Board {
    previews: Vec<Option<Vec<u8>>>,
    menu: Option<Menu>,
    /// When the appearance library was last revealed, while it shows.
    library: Option<Instant>,
    shows_log: bool,
    pub log: Vec<Entry>,
    pub active: usize,
    pub pending: Option<usize>,
    pub muted: bool,
    pub voice: VoiceId,
    pub anchor: Anchor,
    contact: Contact,
    hover: Option<Press>,
    dirty: bool,
}

impl Board {
    pub fn new(appearances: usize, active: usize) -> Board {
        Board { previews: vec![None; appearances], menu: None, library: None, shows_log: true, log: Vec::new(), active, pending: None, muted: false, voice: VoiceId::default(), anchor: Anchor::Riding, contact: Contact::default(), hover: None, dirty: true }
    }

    /// As summoned: a hand already at the board must withdraw before it presses, and the appearance library is hidden.
    pub fn reset(&mut self) {
        self.contact.reset();
        self.hover = None;
        self.library = None;
        if let Some(menu) = &mut self.menu {
            menu.open = false;
            menu.path.clear();
        }
        self.dirty = true;
    }

    /// Shows the appearance library below the buttons, growing the board downward, for `LIBRARY_SHOWN` from `now`.
    pub fn reveal(&mut self, now: Instant) {
        self.library = Some(now);
        if let Some(menu) = &mut self.menu {
            menu.open = false;
        }
        self.dirty = true;
    }

    pub fn wear(&mut self, menu: Option<(Vec<Control>, Vec<Image>)>, values: HashMap<String, f32>) {
        self.menu = menu.filter(|(controls, _)| !controls.is_empty()).map(|(controls, icons)| Menu { controls, icons, path: Vec::new(), open: false, values });
        self.dirty = true;
    }

    pub fn toggle_menu(&mut self) {
        if let Some(menu) = &mut self.menu {
            menu.open = !menu.open;
            menu.path.clear();
            self.library = None;
            self.dirty = true;
        }
    }

    pub fn back(&mut self) {
        if let Some(menu) = &mut self.menu {
            menu.path.pop();
            self.dirty = true;
        }
    }

    pub fn control(&mut self, index: usize) -> Option<Control> {
        let menu = self.menu.as_mut()?;
        let control = menu.page().get(index)?.clone();
        if control.kind == Kind::Submenu {
            menu.path.push(index);
        }
        self.dirty = true;
        Some(control)
    }

    pub fn place_on(&self, index: usize, [x, y]: [f32; 2]) -> [f32; 2] {
        let [left, bottom, right, top] = self.menu_rects().into_iter().find(|&(press, _)| press == Press::Control(index)).map_or([0.0, 0.0, 1.0, 1.0], |(_, rect)| rect);
        [((x - left) / (right - left)).clamp(0.0, 1.0), ((y - bottom) / (top - bottom)).clamp(0.0, 1.0)]
    }

    pub fn set_values(&mut self, values: HashMap<String, f32>) {
        if let Some(menu) = &mut self.menu {
            if menu.values != values {
                menu.values = values;
                self.dirty = true;
            }
        }
    }

    fn rows(&self) -> usize {
        ROWS + self.menu.is_some() as usize
    }

    fn menu_open(&self) -> Option<&Menu> {
        self.menu.as_ref().filter(|menu| menu.open)
    }

    fn menu_rects(&self) -> Vec<(Press, [f32; 4])> {
        let Some(menu) = self.menu_open() else { return Vec::new() };
        let (left, right) = (-WIDTH / 2.0 + GAP, WIDTH / 2.0 - GAP);
        let half = (right - left - GAP) / 2.0;
        let mut top = self.height() / 2.0 - GAP - self.rows() as f32 * (ROW + GAP) - self.log_height();
        let mut out = Vec::new();
        let mut row = |height: f32, columns: &[(Press, f32, f32)], out: &mut Vec<(Press, [f32; 4])>| {
            for &(press, from, to) in columns {
                out.push((press, [from, top - height, to, top]));
            }
            top -= height + GAP;
        };
        if !menu.path.is_empty() {
            row(ROW, &[(Press::Back, left, right)], &mut out);
        }
        let mut pending: Option<usize> = None;
        for (index, control) in menu.page().iter().enumerate() {
            match control.kind {
                Kind::Radial => row(ROW, &[(Press::Control(index), left, right)], &mut out),
                Kind::TwoAxis | Kind::FourAxis => row(PAD_ROWS as f32 * ROW + (PAD_ROWS - 1) as f32 * GAP, &[(Press::Control(index), left, right)], &mut out),
                _ => match pending.take() {
                    Some(first) => row(ROW, &[(Press::Control(first), left, left + half), (Press::Control(index), right - half, right)], &mut out),
                    None => pending = Some(index),
                },
            }
        }
        if let Some(last) = pending {
            row(ROW, &[(Press::Control(last), left, left + half)], &mut out);
        }
        out
    }

    fn menu_height(&self) -> f32 {
        let Some(menu) = self.menu_open() else { return 0.0 };
        let pad = PAD_ROWS as f32 * ROW + (PAD_ROWS - 1) as f32 * GAP;
        let paired = menu.page().iter().filter(|control| !matches!(control.kind, Kind::Radial | Kind::TwoAxis | Kind::FourAxis)).count();
        let heights = (!menu.path.is_empty()) as usize as f32 * (ROW + GAP)
            + menu.page().iter().map(|control| match control.kind {
                Kind::Radial => ROW + GAP,
                Kind::TwoAxis | Kind::FourAxis => pad + GAP,
                _ => 0.0,
            }).sum::<f32>()
            + paired.div_ceil(2) as f32 * (ROW + GAP);
        heights
    }

    /// Hides the appearance library once `LIBRARY_SHOWN` has passed since its reveal.
    pub fn tick(&mut self, now: Instant) {
        if self.library.is_some_and(|revealed| now.duration_since(revealed) >= LIBRARY_SHOWN) {
            self.library = None;
            self.dirty = true;
        }
    }

    pub fn library(&self) -> bool {
        self.library.is_some()
    }

    /// Shows or hides the log below the buttons.
    pub fn toggle_log(&mut self) {
        self.shows_log = !self.shows_log;
        self.dirty = true;
    }

    /// The newest lines of the log that fit, oldest first, each with its ink and stripe: what was heard, what was spoken, and each hub call marked by its state.
    fn log_lines(&self) -> Vec<(String, [u8; 4], [u8; 4])> {
        if !self.shows_log {
            return Vec::new();
        }
        let fits = ((WIDTH * PIXELS_PER_METRE) as u32 - 2 * PADDING - 2 * STRIPE) as usize / 8;
        let mut lines = Vec::new();
        for entry in &self.log {
            match entry {
                Entry::Heard(text) => lines.extend(wrap(text, fits, usize::MAX).into_iter().map(|line| (line, INK, HEARD))),
                Entry::Spoke(text) => lines.extend(wrap(text, fits, usize::MAX).into_iter().map(|line| (line, SPOKE_INK, BUTTON))),
                Entry::Note(text) => lines.extend(wrap(text, fits, usize::MAX).into_iter().map(|line| (line, INK, DISMISS))),
                Entry::Call(call) => {
                    let mark = match call.state {
                        State::Waiting => PENDING,
                        State::Replied => ACTIVE,
                        State::Failed => DISMISS,
                    };
                    let request = wrap(&call.request, fits, REQUEST_LINES);
                    let request = if request.is_empty() && call.replies.is_empty() { vec![String::new()] } else { request };
                    lines.extend(request.into_iter().map(|line| (line, INK, mark)));
                    lines.extend(wrap(&call.replies.join(" "), fits, REPLY_LINES).into_iter().map(|line| (line, REPLY_INK, mark)));
                }
            }
        }
        lines.split_off(lines.len().saturating_sub(LOG_LINES))
    }

    fn log_height(&self) -> f32 {
        if !self.shows_log {
            return 0.0;
        }
        (LOG_LINES as u32 * LINE + 2 * PADDING) as f32 / PIXELS_PER_METRE + GAP
    }

    /// The appearance's preview, `PREVIEW` pixels square, premultiplied RGBA, top row first.
    pub fn preview(&mut self, index: usize, rgba: Vec<u8>) {
        assert_eq!(rgba.len(), (PREVIEW * PREVIEW * 4) as usize, "one RGBA texel per preview pixel");
        self.previews[index] = Some(rgba);
        self.dirty = true;
    }

    pub fn height(&self) -> f32 {
        let lines = if self.library() { self.previews.len().div_ceil(COLUMNS) } else { 0 };
        self.rows() as f32 * ROW + self.log_height() + lines as f32 * CELL + (self.rows() + lines + 1) as f32 * GAP + self.menu_height()
    }

    pub fn pixels(&self) -> [u32; 2] {
        [(WIDTH * PIXELS_PER_METRE).round() as u32, (self.height() * PIXELS_PER_METRE).round() as u32]
    }

    fn row(&self, index: usize, left: f32, right: f32) -> [f32; 4] {
        let top = self.height() / 2.0 - GAP - index as f32 * (ROW + GAP);
        [left, top - ROW, right, top]
    }

    fn cell(&self, index: usize) -> [f32; 4] {
        let top = self.height() / 2.0 - GAP - self.rows() as f32 * (ROW + GAP) - self.log_height() - (index / COLUMNS) as f32 * (CELL + GAP);
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
            Button { press: Press::Log, rect: self.row(4, left, left + half) },
            Button { press: Press::SetDown, rect: self.row(4, right - half, right) },
        ];
        if self.voice.stored {
            buttons.push(Button { press: Press::VoiceId, rect: self.row(2, left, right) });
        }
        if self.menu.is_some() {
            buttons.push(Button { press: Press::Menu, rect: self.row(ROWS, left, right) });
        }
        buttons.extend(self.menu_rects().into_iter().map(|(press, rect)| Button { press, rect }));
        let shown = if self.library() { self.previews.len() } else { 0 };
        buttons.extend((0..shown).map(|index| Button { press: Press::Appearance(index), rect: self.cell(index) }));
        buttons
    }

    /// The right controller's touch point in the board's frame (x right, y up, z toward the viewer), or None while untracked.
    /// Crossing the face's plane beside the board disarms it too, so sliding on from the edge presses nothing.
    pub fn touch(&mut self, tip: Option<Vec3>) -> Touch {
        let crossing = self.contact.crossing(tip);
        let press = crossing.and_then(|[x, y]| self.under(x, y));
        let marker = marker_at([WIDTH, self.height()], tip);
        let hover = marker.filter(|_| self.contact.armed).and_then(|[x, y, _]| self.under(x, y));
        if hover != self.hover {
            self.hover = hover;
            self.dirty = true;
        }
        Touch { crossing, press, marker }
    }

    fn under(&self, x: f32, y: f32) -> Option<Press> {
        Some(self.buttons().into_iter().find(|button| button.contains(x, y))?.press)
    }

    pub fn mark(&mut self) {
        self.dirty = true;
    }

    /// The board's premultiplied RGBA image, top row first, when it changed since the last call.
    pub fn take_image(&mut self) -> Option<Canvas> {
        std::mem::take(&mut self.dirty).then(|| self.image())
    }

    fn image(&self) -> Canvas {
        let [width, height] = self.pixels();
        let mut image = Canvas::new(width, height, BACKGROUND);
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
                Press::Log => (if self.shows_log { ACTIVE } else { BUTTON }, "Log".to_owned()),
                Press::SetDown => match self.anchor {
                    Anchor::Riding => (BUTTON, "Set down".to_owned()),
                    Anchor::Settling(_) => (PENDING, "Set down".to_owned()),
                    Anchor::Stuck(_) | Anchor::Grounded(_) => (ACTIVE, "Pick up".to_owned()),
                },
                Press::Menu => (if self.menu_open().is_some() { ACTIVE } else { BUTTON }, "Menu".to_owned()),
                Press::Back => {
                    image.fill(area, BUTTON);
                    image.small_label(area, "<");
                    continue;
                }
                Press::Control(index) => {
                    let menu = self.menu_open().expect("a control shows only with the menu open");
                    let control = &menu.page()[index];
                    let [left, top, right, bottom] = area;
                    let icon = control.icon.map(|icon| &menu.icons[icon]).filter(|icon| icon.height <= ROW_PIXELS && 2 * icon.width <= right - left);
                    let inset = icon.map_or(0, |icon| icon.width + (ROW_PIXELS - icon.height) / 2);
                    self.draw_control(&mut image, area, inset, control, &menu.values);
                    if let Some(icon) = icon {
                        let margin = (ROW_PIXELS - icon.height) / 2;
                        image.over([left + margin, top + margin.min(bottom - top - icon.height)], [icon.width, icon.height], &icon.rgba);
                    }
                    continue;
                }
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
                        image.over(corner, [PREVIEW, PREVIEW], preview);
                    }
                    continue;
                }
            };
            image.fill(area, fill);
            image.label(area, &label);
        }
        let [left, _, _, bottom] = self.area(self.row(self.rows() - 1, -WIDTH / 2.0 + GAP, WIDTH / 2.0 - GAP));
        let first = bottom + (GAP * PIXELS_PER_METRE) as u32 + PADDING;
        for (index, (line, ink, mark)) in self.log_lines().into_iter().enumerate() {
            let y = first + index as u32 * LINE;
            image.fill([left, y, left + STRIPE, y + LINE], mark);
            image.text([left + 2 * STRIPE, y + (LINE - 8) / 2], &line.chars().collect::<Vec<_>>(), 1, ink);
        }
        if let Some(button) = self.buttons().into_iter().find(|button| Some(button.press) == self.hover) {
            image.outline(self.area(button.rect), BORDER, HOVER);
        }
        image
    }

    fn draw_control(&self, image: &mut Canvas, area: [u32; 4], inset: u32, control: &Control, values: &HashMap<String, f32>) {
        let value = |name: &str| values.get(name).copied().unwrap_or(0.0);
        let [left, top, right, bottom] = area;
        let labelled = [left + inset, top, right, bottom];
        match control.kind {
            Kind::Toggle | Kind::Button => {
                let on = !control.parameter.is_empty() && value(&control.parameter) == control.value;
                image.fill(area, if on { ACTIVE } else { BUTTON });
                image.small_label(labelled, &control.name);
            }
            Kind::Submenu => {
                image.fill(area, BUTTON);
                image.small_label(labelled, &format!("{} >", control.name));
            }
            Kind::Radial => {
                image.fill(area, BUTTON);
                let fill = value(control.axes.first().map_or("", String::as_str)).clamp(0.0, 1.0);
                image.fill([left, top, left + ((right - left) as f32 * fill).round() as u32, bottom], ACTIVE);
                image.small_label(labelled, &control.name);
            }
            Kind::TwoAxis | Kind::FourAxis => {
                image.fill(area, BUTTON);
                let [x, y] = match control.kind {
                    Kind::TwoAxis => [(value(control.axes.first().map_or("", String::as_str)) + 1.0) / 2.0, (value(control.axes.get(1).map_or("", String::as_str)) + 1.0) / 2.0],
                    _ => {
                        let axis = |at: usize| value(control.axes.get(at).map_or("", String::as_str));
                        [(1.0 + axis(1) - axis(3)) / 2.0, (1.0 + axis(0) - axis(2)) / 2.0]
                    }
                };
                let (cx, cy) = ((right - left) / 2 + left, (bottom - top) / 2 + top);
                image.fill([cx, top, cx + 1, bottom], BACKGROUND);
                image.fill([left, cy, right, cy + 1], BACKGROUND);
                let dot = [left + ((right - left) as f32 * x.clamp(0.0, 1.0)) as u32, bottom - ((bottom - top) as f32 * y.clamp(0.0, 1.0)) as u32];
                image.fill([dot[0].saturating_sub(BORDER).max(left), dot[1].saturating_sub(BORDER).max(top), (dot[0] + BORDER).min(right), (dot[1] + BORDER).min(bottom)], ACTIVE);
                image.small_label([left + inset, top, right, top + ROW_PIXELS], &control.name);
            }
        }
    }

    /// Metres from the board's centre to left, top, right, bottom pixels.
    fn area(&self, [left, bottom, right, top]: [f32; 4]) -> [u32; 4] {
        let height = self.height();
        let x = |x: f32| ((x + WIDTH / 2.0) * PIXELS_PER_METRE).round() as u32;
        let y = |y: f32| ((height / 2.0 - y) * PIXELS_PER_METRE).round() as u32;
        [x(left), y(top), x(right), y(bottom)]
    }
}

pub fn marker_image() -> Canvas {
    let centre = MARKER_PIXELS as f32 / 2.0;
    let mut image = Canvas { pixels: vec![0; (MARKER_PIXELS * MARKER_PIXELS * 4) as usize], width: MARKER_PIXELS, height: MARKER_PIXELS };
    for y in 0..MARKER_PIXELS {
        for x in 0..MARKER_PIXELS {
            let distance = (x as f32 + 0.5 - centre).hypot(y as f32 + 0.5 - centre);
            if distance <= centre * 0.6 {
                image.fill([x, y, x + 1, y + 1], INK);
            } else if distance <= centre {
                image.fill([x, y, x + 1, y + 1], BACKGROUND);
            }
        }
    }
    image
}

pub struct Canvas {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Canvas {
    pub fn new(width: u32, height: u32, color: [u8; 4]) -> Canvas {
        let mut canvas = Canvas { pixels: vec![0; (width * height * 4) as usize], width, height };
        canvas.fill([0, 0, width, height], color);
        canvas
    }

    pub fn fill(&mut self, [left, top, right, bottom]: [u32; 4], color: [u8; 4]) {
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

    fn outline(&mut self, [left, top, right, bottom]: [u32; 4], width: u32, color: [u8; 4]) {
        self.fill([left, top, right, top + width], color);
        self.fill([left, bottom - width, right, bottom], color);
        self.fill([left, top, left + width, bottom], color);
        self.fill([right - width, top, right, bottom], color);
    }

    /// A premultiplied image drawn over the canvas with its top left corner at `at`.
    pub fn over(&mut self, [x0, y0]: [u32; 2], [width, height]: [u32; 2], rgba: &[u8]) {
        for y in 0..height {
            for x in 0..width {
                let from = ((y * width + x) * 4) as usize;
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
        self.text([x0, y0], &characters, GLYPH_SCALE, INK);
    }

    fn small_label(&mut self, [left, top, right, bottom]: [u32; 4], text: &str) {
        let glyph = 8 * MENU_GLYPH_SCALE;
        let fits = ((right - left).saturating_sub(2 * PADDING) / glyph) as usize;
        let mut characters: Vec<char> = text.trim().chars().map(|c| if c.is_ascii() && !c.is_ascii_control() { c } else { '?' }).collect();
        if characters.len() > fits {
            characters.truncate(fits.saturating_sub(2));
            characters.extend(['.', '.']);
        }
        let x0 = left + (right - left - characters.len() as u32 * glyph) / 2;
        let y0 = top + (bottom - top).saturating_sub(glyph) / 2;
        self.text([x0, y0], &characters, MENU_GLYPH_SCALE, INK);
    }

    /// ASCII glyphs `scale` canvas pixels per font pixel, the first glyph's top left corner at `at`.
    pub fn text(&mut self, [x0, y0]: [u32; 2], characters: &[char], scale: u32, ink: [u8; 4]) {
        for (index, &character) in characters.iter().enumerate() {
            let Some(bits) = BASIC_LEGACY.get(character as usize) else { continue };
            for (line, bits) in bits.iter().enumerate() {
                for column in 0..8 {
                    if bits >> column & 1 == 1 {
                        let x = x0 + (index as u32 * 8 + column) * scale;
                        let y = y0 + line as u32 * scale;
                        self.fill([x, y, x + scale, y + scale], ink);
                    }
                }
            }
        }
    }
}

/// Words wrapped to lines of at most `width` characters, the last of `most` cut short with "..".
fn wrap(text: &str, width: usize, most: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        while !word.is_empty() {
            let line = match lines.last_mut() {
                Some(line) if line.chars().count() + 1 + word.len().min(width) <= width => {
                    line.push(' ');
                    line
                }
                _ => {
                    lines.push(String::new());
                    lines.last_mut().unwrap()
                }
            };
            let take = word.len().min(width - line.chars().count());
            line.extend(word.drain(..take));
        }
    }
    if lines.len() > most {
        lines.truncate(most);
        let last = lines.last_mut().unwrap();
        let mut characters: Vec<char> = last.chars().collect();
        characters.truncate(width - 2);
        while characters.last() == Some(&' ') {
            characters.pop();
        }
        *last = characters.into_iter().chain(['.', '.']).collect();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::{length, local_tip, marker, sub, under_controller, Pose, TIP};

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
            presses.extend(board.touch(Some([x + jitter, y - jitter, z])).press);
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

    const BACKGROUND_PREMULTIPLIED: [u8; 4] = [(18 * 210 / 255) as u8, (18 * 210 / 255) as u8, (22 * 210 / 255) as u8, 210];

    fn corner_of(board: &Board, image: &[u8], press: Press) -> [u8; 4] {
        let [left, _, _, top] = rect_of(board, press);
        pixel(board, image, left + 0.001, top - 0.001)
    }

    fn inked(board: &Board, image: &[u8], [left, top, right, bottom]: [u32; 4]) -> Vec<bool> {
        let width = board.pixels()[0];
        (top..bottom).flat_map(|y| (left..right).map(move |x| ((y * width + x) * 4) as usize)).map(|at| image[at..at + 4] == INK).collect()
    }

    #[test]
    fn touching_each_button_fires_its_action_exactly_once() {
        let mut board = Board::new(3, 0);
        board.reveal(Instant::now());
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
    fn a_simulated_right_controller_presses_each_button_on_the_turned_over_board_once_and_passing_near_presses_nothing() {
        let (yaw, pitch) = (0.6f32, -0.4f32);
        let left = Pose { r: [[yaw.cos(), pitch.sin() * yaw.sin(), pitch.cos() * yaw.sin()], [0.0, pitch.cos(), -pitch.sin()], [-yaw.sin(), pitch.sin() * yaw.cos(), pitch.cos() * yaw.cos()]], t: [-0.2, 1.1, -0.35] };
        let over = left.then(&Pose { r: [[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] });
        let mut board = Board::new(36, 0);
        board.reveal(Instant::now());
        let pose = over.then(&under_controller(board.height()));
        let tip = |right: &Pose| local_tip(&pose, right, TIP);
        for z in [0.05, 0.015] {
            for step in 0..=100 {
                for y in [-0.12, -0.08, 0.0, 0.08, 0.12] {
                    let right = right_controller(&pose, [-WIDTH + step as f32 * WIDTH / 50.0, y], z);
                    assert_eq!(board.touch(Some(tip(&right))).press, None);
                }
            }
        }
        for button in board.buttons() {
            let [left, bottom, right, top] = button.rect;
            let centre = [(left + right) / 2.0, (bottom + top) / 2.0];
            let mut presses = Vec::new();
            for step in (0..=60).chain((0..=60).rev()) {
                presses.extend(board.touch(Some(tip(&right_controller(&pose, centre, 0.1 - step as f32 * 0.002)))).press);
            }
            assert_eq!(presses, [button.press]);
        }
    }

    #[test]
    fn with_a_simulated_right_controller_the_marker_sits_at_the_hit_test_point_and_each_touch_presses_once() {
        let left = Pose { r: [[0.8, 0.0, 0.6], [0.0, 1.0, 0.0], [-0.6, 0.0, 0.8]], t: [-0.2, 1.1, -0.35] };
        const PRESSING: Vec3 = [0.004, -0.021, -0.012];
        let mut board = Board::new(12, 0);
        board.reveal(Instant::now());
        let pose = left.then(&under_controller(board.height()));
        for button in board.buttons() {
            let [left, bottom, right, top] = button.rect;
            let centre = [(left + right) / 2.0, (bottom + top) / 2.0];
            let mut presses = Vec::new();
            for step in (0..=70).chain((0..=70).rev()) {
                let tilted = pose.then(&Pose { r: [[0.6, 0.0, 0.8], [0.0, 1.0, 0.0], [-0.8, 0.0, 0.6]], t: [0.0; 3] });
                let controller = Pose { r: tilted.r, t: sub(pose.apply([centre[0], centre[1], 0.09 - step as f32 * 0.002]), tilted.rotate(PRESSING)) };
                let tip = local_tip(&pose, &controller, PRESSING);
                let touch = board.touch(Some(tip));
                let (press, at) = (touch.press, touch.marker.expect("shown over the board"));
                assert!(length(sub(at, [tip[0], tip[1], tip[2].max(0.0)])) < 1e-5, "{at:?} for {tip:?}");
                let shown = controller.then(&marker(&pose, &controller, at));
                assert!(length(sub(shown.t, pose.apply(at))) < 1e-5, "drawn where the board says");
                assert!((0..3).all(|axis| length(sub(shown.axis(axis), pose.axis(axis))) < 1e-5), "facing as the board does");
                if tip[2] >= 0.0 {
                    assert!(length(sub(shown.t, controller.apply(PRESSING))) < 1e-5, "at the controller's touch point");
                }
                if let Some(press) = press {
                    assert!(button.contains(at[0], at[1]), "{press:?} pressed under the marker");
                    presses.push(press);
                }
            }
            assert_eq!(presses, [button.press]);
        }
    }

    #[test]
    fn a_point_presses_a_button_exactly_where_that_button_is_drawn() {
        let mut board = Board::new(12, 0);
        board.reveal(Instant::now());
        let image = board.take_image().unwrap().pixels;
        let [width, height] = board.pixels();
        let edge = 1.0 / PIXELS_PER_METRE;
        for line in 0..height {
            for column in 0..width {
                let x = (column as f32 + 0.5) / PIXELS_PER_METRE - WIDTH / 2.0;
                let y = board.height() / 2.0 - (line as f32 + 0.5) / PIXELS_PER_METRE;
                let near_an_edge = board.buttons().iter().any(|button| button.rect.iter().enumerate().any(|(side, &bound)| ((if side % 2 == 0 { x } else { y }) - bound).abs() < edge));
                if near_an_edge {
                    continue;
                }
                let drawn = image[((line * width + column) * 4 + 3) as usize] != BACKGROUND[3];
                assert_eq!(board.under(x, y).is_some(), drawn, "({x}, {y})");
            }
        }
    }

    #[test]
    fn hover_follows_the_tip_across_button_bounds_and_clears_on_the_press() {
        let mut board = Board::new(3, 0);
        let [_, y] = centre(&board, Press::Mute);
        board.touch(Some([0.0, y, 0.1]));
        let mut seen = vec![board.hover];
        for step in 0..=100 {
            board.touch(Some([-WIDTH / 2.0 + step as f32 * WIDTH / 100.0, y, 0.03]));
            if seen.last() != Some(&board.hover) {
                seen.push(board.hover);
            }
        }
        assert_eq!(seen, [None, Some(Press::Mute), None, Some(Press::Reset), None]);
        let [x, y] = centre(&board, Press::Reset);
        board.touch(Some([x, y, 0.03]));
        board.take_image();
        assert_eq!(board.touch(Some([x, y, 0.0])).press, Some(Press::Reset));
        assert_eq!(board.hover, None, "pressed; the next press needs the tip drawn back");
        board.touch(Some([x, y, 0.03]));
        assert_eq!(board.hover, Some(Press::Reset));
        let image = board.take_image().expect("redrawn for the hover").pixels;
        let [left, _, _, top] = rect_of(&board, Press::Reset);
        assert_eq!(pixel(&board, &image, left + 0.0005, top - 0.0005), HOVER);
        let [left, _, _, top] = rect_of(&board, Press::Mute);
        assert_eq!(pixel(&board, &image, left + 0.0005, top - 0.0005), BUTTON);
        board.touch(Some([x, y, 0.031]));
        assert!(board.take_image().is_none(), "moving within a button redraws nothing");
    }

    #[test]
    fn the_marker_shows_only_over_the_board_and_near_or_through_its_face() {
        let mut board = Board::new(3, 0);
        let [x, y] = centre(&board, Press::Dismiss);
        for tip in [None, Some([x, y, 0.2]), Some([WIDTH, y, 0.0]), Some([x, board.height(), 0.0])] {
            assert_eq!(board.touch(tip).marker, None, "{tip:?}");
        }
        for z in [-0.05, -0.3] {
            assert_eq!(board.touch(Some([x, y, z])).marker, Some([x, y, 0.0]), "held at the face once through it");
        }
        let image = marker_image().pixels;
        let at = |x: u32, y: u32| <[u8; 4]>::try_from(&image[((y * MARKER_PIXELS + x) * 4) as usize..][..4]).unwrap();
        let middle = MARKER_PIXELS / 2;
        assert_eq!((at(middle, middle), at(1, middle)[3], at(0, 0)[3]), (INK, BACKGROUND[3], 0), "a light dot ringed dark on a clear square");
    }

    #[test]
    fn passing_near_without_touching_fires_nothing() {
        let mut board = Board::new(12, 0);
        for z in [0.05, 0.02, 0.012] {
            for step in 0..=200 {
                let x = -WIDTH + step as f32 * WIDTH / 100.0;
                for y in [-0.06, 0.0, 0.07] {
                    assert_eq!(board.touch(Some([x, y, z])).press, None, "({x}, {y}, {z})");
                }
            }
        }
    }

    #[test]
    fn a_touch_from_behind_or_beside_the_board_fires_nothing() {
        let mut board = Board::new(3, 0);
        let [x, y] = centre(&board, Press::Dismiss);
        for z in [-0.2, -0.05, -0.02, 0.0] {
            assert_eq!(board.touch(Some([x, y, z])).press, None);
        }
        board.touch(Some([x, y, 0.1]));
        let beside = board.touch(Some([WIDTH, y, 0.0]));
        assert_eq!((beside.crossing, beside.press), (Some([WIDTH, y]), None), "a crossing beside the board, reported for the log");
        assert_eq!(board.touch(Some([x, y, 0.0])).press, None, "slid onto it while already in contact");
    }

    #[test]
    fn sliding_across_buttons_while_touching_fires_only_the_first() {
        let mut board = Board::new(3, 0);
        board.reveal(Instant::now());
        let [x, top] = centre(&board, Press::Dismiss);
        let [_, bottom] = centre(&board, Press::Appearance(0));
        board.touch(Some([x, top, 0.1]));
        let mut presses = Vec::new();
        for step in 0..=50 {
            presses.extend(board.touch(Some([x, top + (bottom - top) * step as f32 / 50.0, 0.0])).press);
        }
        assert_eq!(presses, [Press::Dismiss]);
    }

    #[test]
    fn a_tip_trembling_at_the_face_presses_once() {
        let mut board = Board::new(3, 0);
        let [x, y] = centre(&board, Press::Dismiss);
        board.touch(Some([x, y, 0.1]));
        let presses: Vec<Press> = (0..40).filter_map(|step| board.touch(Some([x, y, if step % 2 == 0 { 0.005 } else { 0.018 }])).press).collect();
        assert_eq!(presses, [Press::Dismiss]);
    }

    #[test]
    fn a_right_controller_reappearing_at_the_face_presses_nothing() {
        let mut board = Board::new(3, 0);
        let [x, y] = centre(&board, Press::Dismiss);
        board.touch(Some([x, y, 0.1]));
        assert_eq!(board.touch(None).press, None);
        assert_eq!(board.touch(Some([x, y, 0.0])).press, None);
    }

    #[test]
    fn summoning_with_the_hand_already_on_the_board_needs_a_fresh_touch() {
        let mut board = Board::new(3, 0);
        let [x, y] = centre(&board, Press::Dismiss);
        board.touch(Some([x, y, 0.1]));
        board.reset();
        assert_eq!(board.touch(Some([x, y, 0.0])).press, None);
        assert_eq!(poke(&mut board, [x, y], 0.0), [Press::Dismiss]);
    }

    #[test]
    fn every_appearance_has_its_own_cell_on_the_board_at_once() {
        for count in [1, COLUMNS - 1, COLUMNS, COLUMNS + 1, 36, 37] {
            let mut board = Board::new(count, 0);
            board.reveal(Instant::now());
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
        board.reveal(Instant::now());
        let opaque: Vec<u8> = (0..PREVIEW * PREVIEW).flat_map(|_| [200, 10, 90, 255]).collect();
        let half: Vec<u8> = (0..PREVIEW * PREVIEW).flat_map(|texel| if texel % PREVIEW < PREVIEW / 2 { [0, 0, 0, 0] } else { [100, 0, 0, 128] }).collect();
        board.preview(0, opaque);
        board.preview(1, half);
        board.pending = Some(2);
        let image = board.take_image().unwrap().pixels;
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
        let image = board.take_image().unwrap().pixels;
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
        let image = board.take_image().unwrap().pixels;
        assert_eq!(corner(&board, &image, Press::Mute), MUTED);
        assert_ne!(inked(&board, &image, mute), unmuted, "the label says Unmute mic");
    }

    #[test]
    fn voice_id_shows_only_with_a_voiceprint_and_each_voice_button_presses_once() {
        let mut board = Board::new(3, 0);
        board.reveal(Instant::now());
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
            let image = board.take_image().unwrap().pixels;
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
        let mut board = Board::new(3, 0);
        board.reveal(Instant::now());
        let rect = |press| rect_of(&board, press);
        let ([_, mute_bottom, mute_right, mute_top], [reset_left, reset_bottom, _, reset_top]) = (rect(Press::Mute), rect(Press::Reset));
        assert!(mute_right < reset_left);
        assert_eq!((mute_bottom, mute_top), (reset_bottom, reset_top));
        assert!(rect(Press::Dismiss)[1] > mute_top && mute_bottom > rect(Press::Appearance(0))[3]);
    }

    #[test]
    fn set_down_shares_the_log_row_presses_once_and_shows_its_state() {
        let mut board = Board::new(3, 0);
        let ([_, log_bottom, log_right, log_top], [set_left, set_bottom, _, set_top]) = (rect_of(&board, Press::Log), rect_of(&board, Press::SetDown));
        assert!(log_right < set_left);
        assert_eq!((log_bottom, log_top), (set_bottom, set_top));
        assert_eq!(poke_on(&mut board, Press::SetDown, 0.0), [Press::SetDown]);
        board.touch(None);
        let mut shown = |anchor| {
            board.anchor = anchor;
            board.mark();
            let image = board.take_image().unwrap().pixels;
            (corner_of(&board, &image, Press::SetDown), inked(&board, &image, board.area(rect_of(&board, Press::SetDown))))
        };
        let now = std::time::Instant::now();
        let (riding, settling, stuck) = (shown(Anchor::Riding), shown(Anchor::Settling(now)), shown(Anchor::Stuck(crate::placement::Pose { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] })));
        assert_eq!([riding.0, settling.0, stuck.0], [BUTTON, PENDING, ACTIVE]);
        assert!(riding.1.iter().any(|&inked| inked));
        assert_ne!(riding.1, stuck.1, "Set down, then Pick up");
    }

    fn call(request: &str, replies: &[&str], state: State) -> Entry {
        Entry::Call(crate::calls::Call { id: request.into(), at: 0, request: request.into(), state, replies: replies.iter().map(|reply| reply.to_string()).collect() })
    }

    #[test]
    fn the_log_shows_below_the_buttons_and_above_the_library_newest_lines_last_marked_by_kind_and_state() {
        let mut board = Board::new(6, 0);
        board.reveal(Instant::now());
        board.log = vec![
            Entry::Heard("What is queued right now on every lane of the build queue, and which are stalled?".into()),
            call("What is queued right now on every lane of the build queue, and which are stalled?", &[], State::Waiting),
            Entry::Spoke("Checking.".into()),
            call("Is the printer busy?", &["It is idle.", "The bed is clear."], State::Replied),
            call("Lost one", &[], State::Failed),
            Entry::Heard("And the deck".into()),
        ];
        let lines = board.log_lines();
        assert_eq!(lines.iter().map(|(line, _, _)| line.as_str()).collect::<Vec<_>>(), [
            "What is queued right now on every lane of the",
            "build queue, and which are stalled?",
            "What is queued right now on every lane of the",
            "build queue, and which are stalled?",
            "Checking.",
            "Is the printer busy?",
            "It is idle. The bed is clear.",
            "Lost one",
            "And the deck",
        ]);
        let marks: Vec<_> = lines.iter().map(|&(_, ink, mark)| (ink, mark)).collect();
        assert_eq!(marks, [(INK, HEARD), (INK, HEARD), (INK, PENDING), (INK, PENDING), (SPOKE_INK, BUTTON), (INK, ACTIVE), (REPLY_INK, ACTIVE), (INK, DISMISS), (INK, HEARD)]);
        let image = board.take_image().unwrap().pixels;
        let [left, _, _, bottom] = board.area(rect_of(&board, Press::Log));
        assert_eq!(corner_of(&board, &image, Press::Log), ACTIVE);
        let first = bottom + (GAP * PIXELS_PER_METRE) as u32 + PADDING;
        let stripe = |line: u32| <[u8; 4]>::try_from(&image[(((first + line * LINE + LINE / 2) * board.pixels()[0] + left + 1) * 4) as usize..][..4]).unwrap();
        assert_eq!([stripe(0), stripe(2), stripe(4), stripe(6), stripe(7), stripe(8), stripe(9)], [HEARD, PENDING, BUTTON, ACTIVE, DISMISS, HEARD, BACKGROUND_PREMULTIPLIED]);
        let width = board.pixels()[0];
        let row_inked = |line: u32, ink: [u8; 4]| (first + line * LINE..first + (line + 1) * LINE).any(|y| (left..width).any(|x| image[((y * width + x) * 4) as usize..][..4] == ink));
        assert!(row_inked(0, INK) && row_inked(4, SPOKE_INK) && row_inked(6, REPLY_INK) && !row_inked(9, INK));
        let [_, cell_top, _, _] = board.area(rect_of(&board, Press::Appearance(0)));
        assert!(first + LOG_LINES as u32 * LINE <= cell_top, "the log ends above the library");

        let shown = board.height();
        board.log.push(Entry::Heard("one two three four five six seven eight nine ten ".repeat(20)));
        let lines = board.log_lines();
        assert_eq!(lines.len(), LOG_LINES);
        assert!(lines.iter().all(|&(_, _, mark)| mark == HEARD) && lines.last().unwrap().0.ends_with("ten"), "a long utterance shows its newest words");
        assert_eq!(board.height(), shown, "the log keeps its height as it fills");

        let cell = rect_of(&board, Press::Appearance(0));
        assert_eq!(poke_on(&mut board, Press::Log, 0.0), [Press::Log]);
        board.touch(None);
        board.toggle_log();
        assert!(board.log_lines().is_empty() && board.height() < shown && rect_of(&board, Press::Appearance(0))[3] > cell[3], "hidden, the board shrinks and the library moves up");
        board.reset();
        assert!(board.log_lines().is_empty(), "a summon keeps it hidden");
        board.toggle_log();
        board.reset();
        assert_eq!(board.log_lines().len(), LOG_LINES, "and shown");
    }

    #[test]
    fn wrapping_breaks_between_words_splits_long_words_and_cuts_the_last_line() {
        assert_eq!(wrap("  one two  three ", 7, 3), ["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4, 3), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("aa bb cc dd ee", 5, 2), ["aa bb", "cc.."]);
        assert!(wrap("", 5, 2).is_empty());
    }

    #[test]
    fn the_library_is_hidden_until_revealed_and_hides_again_on_summon() {
        let mut board = Board::new(36, 0);
        let appearances = |board: &Board| board.buttons().iter().filter(|button| matches!(button.press, Press::Appearance(_))).count();
        let hidden = board.height();
        assert_eq!(appearances(&board), 0);
        assert_eq!(board.take_image().unwrap().pixels.len(), (board.pixels()[0] * board.pixels()[1] * 4) as usize);
        board.reveal(Instant::now());
        assert_eq!(appearances(&board), 36);
        assert!(board.height() > hidden && board.take_image().is_some(), "the board grows to hold it");
        assert!(board.take_image().is_none());
        board.reset();
        assert_eq!((appearances(&board), board.height()), (0, hidden));
    }

    #[test]
    fn the_library_hides_itself_a_minute_after_each_reveal() {
        let mut board = Board::new(36, 0);
        let hidden = board.height();
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        board.reveal(start);
        board.take_image();
        board.tick(at(59));
        assert!(board.library() && board.take_image().is_none(), "still shown at 59 s");
        board.tick(at(60));
        assert!(!board.library() && board.height() == hidden && board.take_image().is_some(), "hidden and redrawn at 60 s");
        board.reveal(at(100));
        board.tick(at(150));
        board.reveal(at(150));
        board.tick(at(209));
        assert!(board.library(), "a second reveal restarts the minute");
        board.tick(at(210));
        assert!(!board.library());
    }

    fn menu() -> Vec<Control> {
        serde_json::from_value(serde_json::json!([
            {"name": "Hat", "type": "toggle", "parameter": "Hat"},
            {"name": "Colour", "type": "radial", "axes": ["Hue"]},
            {"name": "Outfits", "type": "submenu", "controls": [{"name": "Coat", "type": "toggle", "parameter": "Coat"}, {"name": "Pose", "type": "twoAxis", "axes": ["X", "Y"]}]}
        ]))
        .unwrap()
    }

    #[test]
    fn the_menu_shows_only_for_an_avatar_with_one_and_each_control_presses_once() {
        let mut board = Board::new(3, 0);
        let plain = board.height();
        assert!(board.buttons().iter().all(|button| button.press != Press::Menu));
        board.wear(Some((menu(), Vec::new())), HashMap::from([("Hat".to_owned(), 1.0)]));
        assert_eq!(poke_on(&mut board, Press::Menu, 0.01), [Press::Menu]);
        board.toggle_menu();
        assert!(board.height() > plain + 3.0 * ROW, "the menu's page grows the board");
        for index in 0..3 {
            assert_eq!(poke_on(&mut board, Press::Control(index), 0.01), [Press::Control(index)]);
        }
        board.reveal(Instant::now());
        assert!(board.buttons().iter().all(|button| !matches!(button.press, Press::Control(_))), "the library takes the menu's place");
    }

    #[test]
    fn a_submenu_opens_in_place_and_back_returns() {
        let mut board = Board::new(1, 0);
        board.wear(Some((menu(), Vec::new())), HashMap::new());
        board.toggle_menu();
        assert_eq!(board.control(2).map(|control| control.name), Some("Outfits".to_owned()));
        assert_eq!(board.control(0).map(|control| control.name), Some("Coat".to_owned()));
        assert!(board.buttons().iter().any(|button| button.press == Press::Back));
        board.back();
        assert_eq!(board.control(0).map(|control| control.name), Some("Hat".to_owned()));
    }

    #[test]
    fn a_radial_reads_where_along_it_the_tip_crossed() {
        let mut board = Board::new(1, 0);
        board.wear(Some((menu(), Vec::new())), HashMap::new());
        board.toggle_menu();
        let [left, bottom, right, top] = rect_of(&board, Press::Control(1));
        let [x, _] = board.place_on(1, [left + 0.25 * (right - left), (bottom + top) / 2.0]);
        assert!((x - 0.25).abs() < 1e-4, "{x}");
        let image = board.take_image().unwrap();
        assert_eq!(pixel(&board, &image.pixels, left + 0.001, (bottom + top) / 2.0), BUTTON, "an unset radial shows no fill");
    }
}
