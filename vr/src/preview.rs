use std::rc::Rc;
use std::sync::mpsc;
use std::time::Instant;

use glam::{Mat4, Vec3};

use crate::board::PREVIEW;
use crate::relay::{Avatar, Relay};
use crate::render::{eye_projection, Renderer, PAGE_HEIGHT};
use crate::vrm::{Fit, Model};
use crate::vulkan::Gpu;

/// Rendered at twice the preview's size and averaged down, for smooth edges.
const SUPERSAMPLE: u32 = 2;
/// The figure stands one unit tall; the square window frames it from the top of the head to the thighs.
const WINDOW: f32 = 0.6;
const CENTRE: f32 = 0.72;
const EYE: Vec3 = Vec3::new(0.0, 0.0, 3.0);

/// Renders every appearance once, in the catalog's order, on a device of its own so the frame loop never waits for it.
pub fn spawn(relay: Relay, avatars: Vec<Avatar>) -> mpsc::Receiver<(usize, Vec<u8>)> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        if let Err(error) = render_all(relay, &avatars, &sender) {
            eprintln!("previews: {error}");
        }
    });
    receiver
}

fn render_all(mut relay: Relay, avatars: &[Avatar], sender: &mpsc::Sender<(usize, Vec<u8>)>) -> Result<(), String> {
    let started = Instant::now();
    let mut renderer = renderer(Rc::new(Gpu::new(None)?))?;
    for (index, avatar) in avatars.iter().enumerate() {
        match relay.puppet(avatar).and_then(|bytes| Model::parse(&bytes)).and_then(|model| render(&mut renderer, &model)) {
            Ok(image) => {
                if sender.send((index, image)).is_err() {
                    return Ok(());
                }
            }
            Err(error) => eprintln!("preview {}: {error}", avatar.id),
        }
    }
    eprintln!("{} previews rendered in {:.1} s", avatars.len(), started.elapsed().as_secs_f64());
    Ok(())
}

fn renderer(gpu: Rc<Gpu>) -> Result<Renderer, String> {
    Renderer::new(gpu, [PREVIEW * SUPERSAMPLE; 2], 1.0 / PAGE_HEIGHT)
}

/// The appearance in its standing pose, facing the viewer: `PREVIEW` pixels square, premultiplied RGBA, top row first.
fn render(renderer: &mut Renderer, model: &Model) -> Result<Vec<u8>, String> {
    let mut skinned = model.skinned()?;
    let fit = Fit::new(model, &skinned, 1.0);
    let pose = model.pose(&model.standing());
    let changed = skinned.morph(&pose.weights);
    let worlds = model.worlds(&pose);
    let palette = skinned.palette(&worlds, Mat4::from_translation(Vec3::new(0.0, -CENTRE, 0.0)) * fit.placement(model, &worlds, 0.0));
    let mut appearance = renderer.appearance(model, &skinned)?;
    appearance.update(&palette, &skinned.vertices, &changed);
    renderer.render(&appearance, [eye_projection(EYE, WINDOW / 2.0, WINDOW / 2.0), None])?;
    Ok(downsample(&renderer.read()?, renderer.eye[0] * 2))
}

/// The left eye's image averaged over `SUPERSAMPLE`-square blocks.
fn downsample(pixels: &[u8], row_length: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((PREVIEW * PREVIEW * 4) as usize);
    for y in 0..PREVIEW {
        for x in 0..PREVIEW {
            let mut sum = [0u32; 4];
            for (dy, dx) in (0..SUPERSAMPLE).flat_map(|dy| (0..SUPERSAMPLE).map(move |dx| (dy, dx))) {
                let at = (((y * SUPERSAMPLE + dy) * row_length + x * SUPERSAMPLE + dx) * 4) as usize;
                for (channel, total) in sum.iter_mut().enumerate() {
                    *total += pixels[at + channel] as u32;
                }
            }
            out.extend(sum.map(|total| ((total + SUPERSAMPLE * SUPERSAMPLE / 2) / (SUPERSAMPLE * SUPERSAMPLE)) as u8));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vrm::tests::plain;

    fn texel(image: &[u8], x: u32, y: u32) -> [u8; 4] {
        let at = ((y * PREVIEW + x) * 4) as usize;
        image[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn a_preview_frames_the_figure_from_just_above_its_head() {
        let mut renderer = renderer(Rc::new(Gpu::new(None).unwrap())).unwrap();
        for (color, expected) in [([1.0, 0.0, 0.0, 1.0], [255, 0, 0]), ([0.0, 0.0, 1.0, 1.0], [0, 0, 255])] {
            let image = render(&mut renderer, &plain(color)).unwrap();
            assert_eq!(image.len(), (PREVIEW * PREVIEW * 4) as usize);
            let head = ((CENTRE + WINDOW / 2.0 - 1.0) / WINDOW * PREVIEW as f32).ceil() as u32;
            for y in 0..PREVIEW {
                for x in 0..PREVIEW {
                    let texel = texel(&image, x, y);
                    match y {
                        y if y + 1 < head => assert_eq!(texel, [0; 4], "clear above the head at ({x}, {y})"),
                        y if y > head => assert!(texel[3] == 255 && (0..3).all(|channel| (texel[channel] > 200) == (expected[channel] == 255) && (texel[channel] == 0) == (expected[channel] == 0)), "the figure at ({x}, {y}): {texel:?}"),
                        _ => {}
                    }
                }
            }
        }
    }

    #[test]
    fn a_simulated_right_controller_touching_the_blue_preview_picks_blue() {
        use crate::board::{Board, Press};
        use crate::placement::{below_wrist, local_tip, Pose, TIP};
        let mut previews = renderer(Rc::new(Gpu::new(None).unwrap())).unwrap();
        let models = [plain([1.0, 0.0, 0.0, 1.0]), plain([0.0, 0.0, 1.0, 1.0]), plain([0.0, 1.0, 0.0, 1.0])];
        let mut board = Board::new(models.len(), 0);
        board.reveal();
        for (index, model) in models.iter().enumerate() {
            board.preview(index, render(&mut previews, model).unwrap());
        }
        let image = board.take_image().unwrap();
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let (left, head) = (Pose { r: identity, t: [-0.2, 1.1, -0.35] }, Pose { r: identity, t: [0.0, 1.6, 0.0] });
        let pose = below_wrist(&left, &head, board.height());
        let [width, height] = board.pixels();
        let found = (0..height).flat_map(|y| (0..width).map(move |x| (x, y))).find(|&(x, y)| {
            let at = ((y * width + x) * 4) as usize;
            image[at + 2] > 200 && image[at] == 0 && image[at + 1] == 0
        });
        let (x, y) = found.expect("the blue preview is on the board");
        let tall = board.height();
        let on_face = |z: f32| [(x as f32 + 0.5) / width as f32 * crate::board::WIDTH - crate::board::WIDTH / 2.0, tall / 2.0 - (y as f32 + 0.5) / height as f32 * tall, z];
        let mut presses = Vec::new();
        for step in (0..=60).chain((0..=60).rev()) {
            let tip = pose.apply(on_face(0.1 - step as f32 * 0.002));
            let right = Pose { r: pose.r, t: crate::placement::sub(tip, pose.rotate(TIP)) };
            presses.extend(board.touch(Some(local_tip(&pose, &right))));
        }
        assert_eq!(presses, [Press::Appearance(1)]);
    }

    #[test]
    fn downsampling_averages_each_block_of_the_left_eye() {
        let size = PREVIEW * SUPERSAMPLE;
        let row_length = size * 2;
        let pixels: Vec<u8> = (0..size).flat_map(|y| (0..row_length).flat_map(move |x| if x >= size { [9; 4] } else if (x + y) % 2 == 0 { [200, 100, 0, 255] } else { [0, 0, 0, 0] })).collect();
        let image = downsample(&pixels, row_length);
        assert_eq!(image.len(), (PREVIEW * PREVIEW * 4) as usize);
        assert!(image.chunks(4).all(|texel| texel == [100, 50, 0, 128]), "half covered, and the right eye ignored");
    }
}
