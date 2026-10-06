use image::imageops::FilterType;

use crate::board::Canvas;
use crate::hub::Display;

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

pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA, top row first.
    pub rgba: Vec<u8>,
}

impl Picture {
    pub fn size(&self) -> [f32; 2] {
        [WIDTH, self.height as f32 / PIXELS_PER_METRE]
    }
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

/// The page's display card for one reply: its text, its link, then its picture; at most `MAX_HEIGHT` tall, text cut short to leave the picture half.
pub fn draw(display: &Display) -> Picture {
    let width = (WIDTH * PIXELS_PER_METRE) as u32;
    let inside = (MAX_HEIGHT * PIXELS_PER_METRE) as u32 - 2 * PADDING;
    let image = display.image.as_deref();
    let mut lines = lines(display, width);
    let budget = if image.is_some() { inside / 2 } else { inside };
    let mut used = 0;
    let fits = lines.iter().take_while(|line| {
        used += line.kind.height();
        used <= budget
    }).count();
    if fits < lines.len() {
        lines.truncate(fits);
        if let Some(last) = lines.last_mut() {
            let keep = columns(last.kind, width).saturating_sub(2);
            last.text = last.text.chars().take(keep).collect::<String>().trim_end().to_owned() + "..";
        }
    }
    let text: u32 = lines.iter().map(|line| line.kind.height()).sum();
    let picture = image.and_then(|encoded| match fitted(encoded, [width - 2 * PADDING, inside - text]) {
        Ok(fitted) => Some(fitted),
        Err(error) => {
            eprintln!("display image not shown: {error}");
            None
        }
    });
    let height = 2 * PADDING + text + picture.as_ref().map_or(0, |(_, [_, height])| *height);
    let mut canvas = Canvas::new(width, height, BACKGROUND);
    let mut y = PADDING;
    for line in &lines {
        canvas.text([PADDING, y], &line.text.chars().collect::<Vec<_>>(), line.kind.scale(), line.kind.ink());
        y += line.kind.height();
    }
    if let Some((rgba, size)) = &picture {
        canvas.over([(width - size[0]) / 2, y], *size, rgba);
    }
    Picture { width, height, rgba: canvas.pixels }
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

    fn at(picture: &Picture, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * picture.width + x) * 4) as usize;
        picture.rgba[at..at + 4].try_into().unwrap()
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
        let picture = draw(&Display { markdown: None, link: Some("https://example.com".into()), image: None });
        assert_eq!(picture.height, 2 * PADDING + Kind::Link.height());
        let inked: Vec<[u8; 4]> = (0..picture.width).flat_map(|x| (0..picture.height).map(move |y| (x, y))).map(|(x, y)| at(&picture, x, y)).filter(|texel| *texel != at(&picture, 0, 0)).collect();
        assert!(!inked.is_empty() && inked.iter().all(|texel| *texel == LINK));
    }

    #[test]
    fn a_picture_fills_the_width_below_the_text_and_long_text_leaves_it_half() {
        let red = [200, 0, 0, 255];
        let picture = draw(&Display { markdown: Some("Chart".into()), link: None, image: Some(png(40, 20, red)) });
        let width = picture.width - 2 * PADDING;
        assert_eq!(picture.height, 2 * PADDING + Kind::Text.height() + width / 2);
        assert_eq!(at(&picture, picture.width / 2, PADDING + Kind::Text.height() + width / 4), red);
        let tall = draw(&Display { markdown: Some("words ".repeat(2000)), link: None, image: Some(png(10, 40, red)) });
        assert!(tall.size()[1] <= MAX_HEIGHT);
        assert_eq!(at(&tall, tall.width / 2, tall.height - PADDING - 4), red);
        assert_eq!(at(&tall, tall.width / 2, tall.height / 2 + 8), red, "text stops at half");
    }

    #[test]
    fn an_unreadable_image_leaves_the_text() {
        let picture = draw(&Display { markdown: Some("Hi".into()), link: None, image: Some(b"not an image".to_vec()) });
        assert_eq!(picture.height, 2 * PADDING + Kind::Text.height());
    }
}
