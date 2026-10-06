use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::relay::Avatar;

/// The overlay's own appearance, `{"id": "<catalog id>"}` in `puppet.json` beside the cached puppets; the page's selection never reaches it.
pub struct Chosen {
    file: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Choice {
    id: String,
}

impl Chosen {
    pub fn beside(assets: &Path) -> Chosen {
        Chosen { file: assets.join("puppet.json") }
    }

    /// The chosen appearance's index in `avatars`.
    pub fn read(&self, avatars: &[Avatar]) -> Result<usize, String> {
        let bytes = std::fs::read(&self.file).map_err(|error| format!("{}: {error}", self.file.display()))?;
        let choice: Choice = serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", self.file.display()))?;
        avatars.iter().position(|avatar| avatar.id == choice.id).ok_or_else(|| format!("{}: appearance {} is not in the catalog", self.file.display(), choice.id))
    }

    pub fn write(&self, id: &str) -> Result<(), String> {
        let staged = self.file.with_extension("json.new");
        std::fs::write(&staged, format!("{}\n", serde_json::json!(Choice { id: id.to_owned() })))
            .and_then(|()| std::fs::rename(&staged, &self.file))
            .map_err(|error| format!("{}: {error}", self.file.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avatars() -> Vec<Avatar> {
        ["1", "2"].map(|id| Avatar { id: id.to_owned(), content_hash: "00".to_owned() }).to_vec()
    }

    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("voice-vr-chosen-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn a_written_choice_reads_back() {
        let chosen = Chosen::beside(&scratch("round-trip"));
        chosen.write("2").unwrap();
        assert_eq!(chosen.read(&avatars()), Ok(1));
        chosen.write("1").unwrap();
        assert_eq!(chosen.read(&avatars()), Ok(0));
    }

    #[test]
    fn a_missing_file_or_unknown_id_is_an_error() {
        let directory = scratch("errors");
        let chosen = Chosen::beside(&directory);
        let _ = std::fs::remove_file(directory.join("puppet.json"));
        assert!(chosen.read(&avatars()).is_err());
        std::fs::write(directory.join("puppet.json"), r#"{"id": "3"}"#).unwrap();
        assert!(chosen.read(&avatars()).unwrap_err().contains("appearance 3 is not in the catalog"));
    }
}
