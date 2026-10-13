use image::imageops::FilterType;

use crate::board::{marker_at, Canvas, Contact};
use crate::hub::Display;
use crate::placement::Vec3;

pub const WIDTH: f32 = 0.3;
const PIXELS_PER_METRE: f32 = 2000.0;
const MAX_HEIGHT: f32 = 0.3;
const PADDING: u32 = 12;
const LEADING: u32 = 6;
const SCALE: u32 = 2;
const HEADING_SCALE: u32 = 3;

const BACKGROUND: [u8; 4] = [18, 18, 22, 210];
const INK: [u8; 4] = [240, 240, 240, 255];
const LINK: [u8; 4] = [142, 171, 255, 255];
const CODE: [u8; 4] = [214, 200, 150, 255];
const TRACK: [u8; 4] = [52, 52, 60, 255];
const THUMB: [u8; 4] = [150, 150, 160, 255];
/// The position bar's width, in the right padding.
const BAR: u32 = 4;

const ON_THE_PAGE: &str = "Shown on the page";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Text,
    Heading,
    Code,
    Link,
}

impl Kind {
    fn scale(self) -> u32 {
        if self == Kind::Heading { HEADING_SCALE } else { SCALE }
    }
    fn ink(self) -> [u8; 4] {
        match self {
            Kind::Link => LINK,
            Kind::Code => CODE,
            _ => INK,
        }
    }
    fn height(self) -> u32 {
        8 * self.scale() + LEADING
    }
}

#[derive(Debug, PartialEq)]
struct Line {
    kind: Kind,
    text: String,
}

/// The font's ASCII for what a reply's text commonly carries beyond it.
fn ascii(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '‘' | '’' => '\'',
            '“' | '”' => '"',
            '–' | '—' | '•' | '−' => '-',
            '…' => '.',
            '\t' => ' ',
            c if c.is_ascii() && !c.is_ascii_control() => c,
            _ => '?',
        })
        .collect()
}

/// Emphasis and code marks dropped, links and images reduced to their text.
fn inline(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some(label) = rest.strip_prefix("![").or_else(|| rest.strip_prefix('[')) {
            if let Some((shown, after)) = label.split_once("](") {
                if let Some((_, after)) = after.split_once(')') {
                    out.push_str(shown);
                    rest = after;
                    continue;
                }
            }
        }
        if !matches!(c, '*' | '`') || rest.starts_with("* ") && out.ends_with(' ') {
            out.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn columns(kind: Kind, width: u32) -> usize {
    ((width - 2 * PADDING) / (8 * kind.scale())) as usize
}

/// Words onto lines `columns` wide; a word longer than a line is broken.
fn wrap(text: &str, columns: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ').filter(|word| !word.is_empty()) {
        let word: Vec<char> = word.chars().collect();
        for piece in word.chunks(columns) {
            let piece: String = piece.iter().collect();
            if !line.is_empty() && line.len() + 1 + piece.len() > columns {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&piece);
        }
    }
    lines.push(line);
    lines
}

fn lines(display: &Display, width: u32) -> Vec<Line> {
    if display.html {
        return vec![Line { kind: Kind::Link, text: ON_THE_PAGE.into() }];
    }
    let mut out: Vec<Line> = Vec::new();
    let mut push = |kind: Kind, text: &str| {
        let text = ascii(text);
        let wrapped = if kind == Kind::Code {
            let characters: Vec<char> = text.chars().collect();
            characters.chunks(columns(kind, width)).map(String::from_iter).chain(characters.is_empty().then(String::new)).collect()
        } else {
            wrap(&text, columns(kind, width))
        };
        out.extend(wrapped.into_iter().map(|text| Line { kind, text }));
    };
    let mut code = false;
    for raw in display.markdown.as_deref().unwrap_or_default().lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with("```") {
            code = !code;
        } else if code {
            push(Kind::Code, raw.trim_end());
        } else if trimmed.starts_with('#') {
            push(Kind::Heading, &inline(trimmed.trim_start_matches('#').trim()));
        } else if trimmed.starts_with('|') && trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
        } else if let Some(item) = ["- ", "* ", "+ "].iter().find_map(|bullet| trimmed.strip_prefix(bullet)) {
            push(Kind::Text, &format!("- {}", inline(item)));
        } else {
            push(Kind::Text, &inline(trimmed));
        }
    }
    if let Some(link) = &display.link {
        push(Kind::Link, link);
    }
    while out.first().is_some_and(|line| line.text.is_empty()) {
        out.remove(0);
    }
    out.dedup_by(|line, previous| line.text.is_empty() && previous.text.is_empty());
    while out.last().is_some_and(|line| line.text.is_empty()) {
        out.pop();
    }
    out
}

/// The image's premultiplied RGBA scaled to fit `[width, height]`, aspect kept.
fn fitted(encoded: &[u8], [width, height]: [u32; 2]) -> Result<(Vec<u8>, [u32; 2]), String> {
    let image = image::load_from_memory(encoded).map_err(|error| error.to_string())?;
    let scale = (width as f32 / image.width() as f32).min(height as f32 / image.height() as f32);
    let size = [((image.width() as f32 * scale) as u32).max(1), ((image.height() as f32 * scale) as u32).max(1)];
    let mut rgba = image.resize_exact(size[0], size[1], FilterType::Triangle).to_rgba8().into_raw();
    for texel in rgba.chunks_exact_mut(4) {
        let alpha = texel[3] as u32;
        for channel in &mut texel[..3] {
            *channel = (*channel as u32 * alpha / 255) as u8;
        }
    }
    Ok((rgba, size))
}

/// The page's display card for one reply: its text, its link, then its picture; at most `MAX_HEIGHT` tall, the picture kept to its lower half.
/// Text longer than one view reads a view at a time: a fingertip press on the lower half shows the next, on the upper half the one before.
pub struct Card {
    lines: Vec<Line>,
    /// Each view's first line; a view runs to the next one's.
    views: Vec<usize>,
    view: usize,
    text: u32,
    picture: Option<(Vec<u8>, [u32; 2])>,
    contact: Contact,
    dirty: bool,
}

/// What a fingertip did at the card this frame.
pub struct Touch {
    pub turned: bool,
    pub marker: Option<Vec3>,
}

impl Card {
    pub fn new(display: &Display) -> Card {
        let width = (WIDTH * PIXELS_PER_METRE) as u32;
        let inside = (MAX_HEIGHT * PIXELS_PER_METRE) as u32 - 2 * PADDING;
        let image = display.image.as_deref();
        let lines = lines(display, width);
        let budget = if image.is_some() { inside / 2 } else { inside };
        let mut views = vec![0];
        let mut used = 0;
        for (index, line) in lines.iter().enumerate() {
            used += line.kind.height();
            if used > budget && index > *views.last().unwrap() {
                views.push(index);
                used = line.kind.height();
            }
        }
        let mut card = Card { lines, views, view: 0, text: 0, picture: None, contact: Contact::default(), dirty: true };
        card.text = (0..card.views.len()).map(|view| card.shown_in(view).iter().map(|line| line.kind.height()).sum()).max().unwrap_or(0);
        card.picture = image.and_then(|encoded| match fitted(encoded, [width - 2 * PADDING, inside - card.text]) {
            Ok(fitted) => Some(fitted),
            Err(error) => {
                tracing::warn!("display image not shown: {error}");
                None
            }
        });
        card
    }

    fn shown_in(&self, view: usize) -> &[Line] {
        let end = self.views.get(view + 1).copied().unwrap_or(self.lines.len());
        &self.lines[self.views[view]..end]
    }

    fn height(&self) -> u32 {
        2 * PADDING + self.text + self.picture.as_ref().map_or(0, |(_, [_, height])| *height)
    }

    /// The card's extent in metres, the same for every view.
    pub fn size(&self) -> [f32; 2] {
        [WIDTH, self.height() as f32 / PIXELS_PER_METRE]
    }

    pub fn views(&self) -> usize {
        self.views.len()
    }

    /// Shows the view `by` away from this one, if there is one.
    pub fn turn(&mut self, by: isize) -> bool {
        let Some(view) = self.view.checked_add_signed(by).filter(|&view| view < self.views.len()) else { return false };
        self.view = view;
        self.dirty = true;
        true
    }

    /// As shown afresh: a hand already at the card must withdraw before it turns a view.
    pub fn reset(&mut self) {
        self.contact.reset();
        self.dirty = true;
    }

    /// The right controller's touch point in the card's frame (x right, y up, z toward the viewer), or None while untracked.
    pub fn touch(&mut self, tip: Option<Vec3>) -> Touch {
        let [width, height] = self.size();
        let turned = match self.contact.crossing(tip) {
            Some([x, y]) if x.abs() <= width / 2.0 && y.abs() <= height / 2.0 => self.turn(if y < 0.0 { 1 } else { -1 }),
            _ => false,
        };
        Touch { turned, marker: marker_at([width, height], tip) }
    }

    /// The shown view's premultiplied RGBA image, top row first, when it changed since the last call.
    pub fn take_image(&mut self) -> Option<Canvas> {
        std::mem::take(&mut self.dirty).then(|| self.image())
    }

    fn image(&self) -> Canvas {
        let width = (WIDTH * PIXELS_PER_METRE) as u32;
        let mut canvas = Canvas::new(width, self.height(), BACKGROUND);
        let mut y = PADDING;
        for line in self.shown_in(self.view) {
            canvas.text([PADDING, y], &line.text.chars().collect::<Vec<_>>(), line.kind.scale(), line.kind.ink());
            y += line.kind.height();
        }
        if self.views.len() > 1 {
            let [left, right] = [width - (PADDING + BAR) / 2, width - (PADDING - BAR) / 2];
            let along = |line: usize| PADDING + (line as u64 * self.text as u64 / self.lines.len() as u64) as u32;
            let end = self.views.get(self.view + 1).copied().unwrap_or(self.lines.len());
            canvas.fill([left, PADDING, right, PADDING + self.text], TRACK);
            canvas.fill([left, along(self.views[self.view]), right, along(end)], THUMB);
        }
        if let Some((rgba, size)) = &self.picture {
            canvas.over([(width - size[0]) / 2, PADDING + self.text], *size, rgba);
        }
        canvas
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(markdown: &str) -> Vec<(Kind, String)> {
        lines(&Display { markdown: Some(markdown.into()), ..Display::default() }, (WIDTH * PIXELS_PER_METRE) as u32).into_iter().map(|line| (line.kind, line.text)).collect()
    }

    fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut encoded = Vec::new();
        image::RgbaImage::from_pixel(width, height, image::Rgba(rgba)).write_to(&mut std::io::Cursor::new(&mut encoded), image::ImageFormat::Png).unwrap();
        encoded
    }

    fn draw(display: &Display) -> Canvas {
        Card::new(display).image()
    }

    fn at(picture: &Canvas, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * picture.width + x) * 4) as usize;
        picture.pixels[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn markdown_reads_as_the_card_does_without_its_marks() {
        let card = "## Queue — today\n\n\n**Four** jobs are `queued`, see [the dashboard](https://example.com/d).\n- one\n* two\n\n| job | state |\n|---|:-:|\n| 1 | done |\n```\nfn main() {\n    x\n```";
        assert_eq!(texts(card), [
            (Kind::Heading, "Queue - today".into()),
            (Kind::Text, String::new()),
            (Kind::Text, "Four jobs are queued, see the".into()),
            (Kind::Text, "dashboard.".into()),
            (Kind::Text, "- one".into()),
            (Kind::Text, "- two".into()),
            (Kind::Text, String::new()),
            (Kind::Text, "| job | state |".into()),
            (Kind::Text, "| 1 | done |".into()),
            (Kind::Code, "fn main() {".into()),
            (Kind::Code, "    x".into()),
        ]);
        assert_eq!(texts("2 * 3 and snake_case"), [(Kind::Text, "2 * 3 and snake_case".into())]);
    }

    #[test]
    fn lines_fit_the_panel_and_a_long_word_is_broken() {
        let columns = columns(Kind::Text, (WIDTH * PIXELS_PER_METRE) as u32);
        let long = "x".repeat(columns + 5);
        let wrapped = texts(&format!("a {long}"));
        assert_eq!(wrapped.iter().map(|(_, text)| text.len()).collect::<Vec<_>>(), [1, columns, 5]);
        assert!(texts(&"word ".repeat(200)).iter().all(|(_, text)| text.len() <= columns));
    }

    #[test]
    fn a_link_is_drawn_in_link_ink_below_the_text() {
        let picture = draw(&Display { link: Some("https://example.com".into()), ..Display::default() });
        assert_eq!(picture.height, 2 * PADDING + Kind::Link.height());
        let inked: Vec<[u8; 4]> = (0..picture.width).flat_map(|x| (0..picture.height).map(move |y| (x, y))).map(|(x, y)| at(&picture, x, y)).filter(|texel| *texel != at(&picture, 0, 0)).collect();
        assert!(!inked.is_empty() && inked.iter().all(|texel| *texel == LINK));
    }

    #[test]
    fn a_picture_fills_the_width_below_the_text_and_long_text_leaves_it_half() {
        let red = [200, 0, 0, 255];
        let picture = draw(&Display { markdown: Some("Chart".into()), image: Some(png(40, 20, red)), ..Display::default() });
        let width = picture.width - 2 * PADDING;
        assert_eq!(picture.height, 2 * PADDING + Kind::Text.height() + width / 2);
        assert_eq!(at(&picture, picture.width / 2, PADDING + Kind::Text.height() + width / 4), red);
        let tall = Card::new(&Display { markdown: Some("words ".repeat(2000)), image: Some(png(10, 40, red)), ..Display::default() });
        assert!(tall.size()[1] <= MAX_HEIGHT);
        let tall = tall.image();
        assert_eq!(at(&tall, tall.width / 2, tall.height - PADDING - 4), red);
        assert_eq!(at(&tall, tall.width / 2, tall.height / 2 + 8), red, "text stops at half");
    }

    #[test]
    fn an_unreadable_image_leaves_the_text() {
        let picture = draw(&Display { markdown: Some("Hi".into()), image: Some(b"not an image".to_vec()), ..Display::default() });
        assert_eq!(picture.height, 2 * PADDING + Kind::Text.height());
    }

    #[test]
    fn an_html_display_says_where_it_is_shown_instead_of_drawing_blank() {
        let html = Display { html: true, ..Display::default() };
        assert_eq!(lines(&html, (WIDTH * PIXELS_PER_METRE) as u32), [Line { kind: Kind::Link, text: ON_THE_PAGE.into() }]);
        let picture = draw(&html);
        assert_eq!(picture.height, 2 * PADDING + Kind::Link.height());
        assert!((0..picture.width).any(|x| (0..picture.height).any(|y| at(&picture, x, y) == LINK)));
    }

    fn press(card: &mut Card, y: f32) -> bool {
        card.touch(Some([0.0, y, 0.05]));
        let turned = card.touch(Some([0.0, y, 0.0])).turned;
        card.touch(None);
        turned
    }

    #[test]
    fn a_card_longer_than_one_view_reads_to_its_last_line_by_pressing_its_lower_half() {
        let markdown: String = (1..=120).map(|n| format!("Line {n} of the draft.\n\n")).collect();
        let mut card = Card::new(&Display { markdown: Some(markdown), ..Display::default() });
        assert!(card.views() > 2);
        let size = card.size();
        assert!(size[1] <= MAX_HEIGHT);
        let mut read: Vec<String> = Vec::new();
        let mut views = 0;
        loop {
            let image = card.take_image().expect("each view is drawn");
            assert_eq!([image.width, image.height], [(size[0] * PIXELS_PER_METRE) as u32, (size[1] * PIXELS_PER_METRE).round() as u32], "every view the same size");
            read.extend(card.shown_in(card.view).iter().map(|line| line.text.clone()).filter(|text| !text.is_empty()));
            views += 1;
            if !press(&mut card, -size[1] / 4.0) {
                break;
            }
        }
        assert_eq!(views, card.views());
        assert_eq!(read, (1..=120).map(|n| format!("Line {n} of the draft.")).collect::<Vec<_>>(), "every line once, none cut");
        let last = card.image();
        let row = PADDING + card.shown_in(card.view).iter().map(|line| line.kind.height()).sum::<u32>() - Kind::Text.height() + 4;
        assert!((PADDING..last.width / 2).any(|x| at(&last, x, row) == INK), "the last line is inked");
        assert!(!press(&mut card, -size[1] / 4.0), "the last view stays");
        assert!(press(&mut card, size[1] / 4.0), "the upper half goes back");
        assert_eq!(card.view, card.views() - 2);
    }

    #[test]
    fn a_press_beside_the_card_or_without_drawing_back_turns_nothing_and_one_view_has_no_bar() {
        let markdown: String = (1..=120).map(|n| format!("Line {n}\n")).collect();
        let mut card = Card::new(&Display { markdown: Some(markdown), ..Display::default() });
        let [width, height] = card.size();
        assert!(!press(&mut card, -height), "below the card");
        assert!(!card.touch(Some([width, -height / 4.0, 0.05])).turned && !card.touch(Some([width, -height / 4.0, 0.0])).turned, "beside the card");
        card.touch(Some([0.0, -height / 4.0, 0.05]));
        assert!(card.touch(Some([0.0, -height / 4.0, 0.0])).turned);
        assert!(!card.touch(Some([0.0, -height / 4.0, 0.0])).turned, "held at the face turns once");
        let full = card.image();
        assert!((0..full.height).any(|y| at(&full, full.width - PADDING / 2, y) == THUMB));
        let short = draw(&Display { markdown: Some("Hi".into()), ..Display::default() });
        assert!((0..short.height).all(|y| at(&short, short.width - PADDING / 2, y) == at(&short, 0, 0)));
    }
}
