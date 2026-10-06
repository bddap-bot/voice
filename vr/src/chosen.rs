use std::path::Path;

use serde::Deserialize;

use crate::relay::Avatar;

#[derive(Deserialize)]
struct Choice {
    id: String,
}

/// The appearance the overlay boots with, `{"id": "<catalog id>"}` in `puppet.json` beside the cached puppets; a library pick lives only in the running host.
pub fn read(assets: &Path, avatars: &[Avatar]) -> Result<usize, String> {
    let file = assets.join("puppet.json");
    let bytes = std::fs::read(&file).map_err(|error| format!("{}: {error}", file.display()))?;
    let choice: Choice = serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", file.display()))?;
    avatars.iter().position(|avatar| avatar.id == choice.id).ok_or_else(|| format!("{}: appearance {} is not in the catalog", file.display(), choice.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;

    fn avatars() -> Vec<Avatar> {
        ["1", "2"].map(|id| Avatar { id: id.to_owned(), content_hash: "00".to_owned() }).to_vec()
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!("voice-vr-chosen-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn a_pick_leaves_the_boot_file_byte_identical() {
        let directory = scratch("pick");
        let file = directory.join("puppet.json");
        std::fs::write(&file, "{\"id\": \"1\"}\n").unwrap();
        let before = (std::fs::read(&file).unwrap(), std::fs::metadata(&file).unwrap().modified().unwrap());
        let mut board = Board::new(2, read(&directory, &avatars()).unwrap());
        board.active = 1;
        assert_eq!(board.active, 1);
        assert_eq!((std::fs::read(&file).unwrap(), std::fs::metadata(&file).unwrap().modified().unwrap()), before);
        assert_eq!(read(&directory, &avatars()), Ok(0));
    }

    #[test]
    fn a_missing_file_or_unknown_id_is_an_error() {
        let directory = scratch("errors");
        let _ = std::fs::remove_file(directory.join("puppet.json"));
        assert!(read(&directory, &avatars()).is_err());
        std::fs::write(directory.join("puppet.json"), r#"{"id": "3"}"#).unwrap();
        assert!(read(&directory, &avatars()).unwrap_err().contains("appearance 3 is not in the catalog"));
    }
}
