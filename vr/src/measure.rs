use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::relay::{Catalog, Relay, Token};
use crate::render::{Renderer, PAGE_HEIGHT};
use crate::vulkan::Gpu;
use crate::{dressed, fetch_aside, preview, HEIGHT, QUAD};

/// Picks each appearance of the catalog the host started with, or of `--catalog <file>`, as the host's library does: fetched aside, then dressed on this thread as the frame loop dresses it.
/// Prints one JSON line per pick: whether it was shown, the seconds from the pick to shown, how long the frame loop was held, and the error of one not shown;
/// for an appearance the catalog serves an exporter file for, whether the relay fetched that file with the catalog's hash.
/// With `--previews` it runs the library's preview thread instead and prints when each image arrived.
pub fn run(token: &Token, state: &Path, args: &[String]) -> Result<(), String> {
    let usage = "usage: voice-vr load [--previews] [--catalog <file>] [<catalog id>...]";
    let previews = args.first().is_some_and(|flag| flag == "--previews");
    let args = &args[previews as usize..];
    let cache = state.join("assets");
    let (startup, ids): (Catalog, &[String]) = match args {
        [flag, file, ids @ ..] if flag == "--catalog" => (serde_json::from_slice(&std::fs::read(file).map_err(|error| format!("{file}: {error}"))?).map_err(|error| format!("{file}: {error}"))?, ids),
        [flag, ..] if flag.starts_with("--") => return Err(usage.into()),
        ids => (Relay::connect(token, &cache)?.catalog()?, ids),
    };
    let wanted = |id: &str| ids.is_empty() || ids.iter().any(|wanted| wanted == id);
    // Made first, as the host does: were the preview thread's device the process's only one, the loader would unload the driver before that thread's exit runs the driver's thread-local destructors.
    let gpu = Rc::new(Gpu::new(None)?);
    if previews {
        let started = Instant::now();
        let mut shown = std::collections::HashSet::new();
        for (id, _) in preview::spawn(token.clone(), cache.clone()) {
            if wanted(&id) {
                println!("{}", serde_json::json!({"id": id, "shown": true, "seconds": started.elapsed().as_secs_f64()}));
                shown.insert(id);
            }
        }
        for avatar in startup.avatars.iter().filter(|avatar| wanted(&avatar.id) && !shown.contains(&avatar.id)) {
            println!("{}", serde_json::json!({"id": avatar.id, "shown": false, "error": "no preview"}));
        }
        return Ok(());
    }
    let mut renderer = Renderer::for_window(gpu, QUAD, HEIGHT / PAGE_HEIGHT)?;
    for avatar in startup.avatars.iter().filter(|avatar| wanted(&avatar.id)) {
        let started = Instant::now();
        let (aside, aside_cache) = (token.clone(), cache.clone());
        let fetched = fetch_aside(move || Relay::connect(&aside, &aside_cache), &avatar.id);
        let mut waiting = Duration::ZERO;
        let arrived = loop {
            let polled = Instant::now();
            match fetched.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                arrived => break arrived.unwrap_or_else(|_| Err("its fetch ended without an answer".into())),
            }
            waiting += polled.elapsed();
            std::thread::sleep(Duration::from_millis(2));
        };
        let dressing = Instant::now();
        let shown = arrived.and_then(|fetched| dressed(fetched.model, fetched.clips, &avatar.id, &mut renderer, &Default::default()));
        let blocked = dressing.elapsed() + waiting;
        let mut pick = serde_json::json!({"id": avatar.id, "shown": shown.is_ok(), "seconds": started.elapsed().as_secs_f64(), "blocked": blocked.as_secs_f64(), "error": shown.err()});
        if let Some(exporter) = &avatar.exporter {
            let fetched = Relay::connect(token, &cache).and_then(|mut relay| relay.exporter(avatar)).and_then(|bytes| {
                let hash = crate::relay::sha256(&bytes);
                if hash == exporter.content_hash { Ok(()) } else { Err(format!("the exporter file arrived as {hash}, not the catalog's {}", exporter.content_hash)) }
            });
            pick["exporter"] = serde_json::json!({"fetched": fetched.is_ok(), "error": fetched.err()});
        }
        println!("{pick}");
    }
    Ok(())
}
