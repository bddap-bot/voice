use std::collections::HashMap;
use std::path::Path;

use crate::vrm::expressions::{Controller, Control, Kind};

pub struct Settings {
    file: std::path::PathBuf,
    avatars: HashMap<String, HashMap<String, f32>>,
}

impl Settings {
    pub fn read(state: &Path) -> Settings {
        let file = state.join("menus.json");
        let avatars = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!("{}: {error}; starting every avatar's menu afresh", file.display());
                HashMap::new()
            }),
            Err(_) => HashMap::new(),
        };
        Settings { file, avatars }
    }

    pub fn of(&self, avatar: &str) -> HashMap<String, f32> {
        self.avatars.get(avatar).cloned().unwrap_or_default()
    }

    pub fn keep(&mut self, avatar: &str, controls: &[Control], controller: &Controller) -> Result<(), String> {
        self.avatars.insert(avatar.to_owned(), shown(controls, controller));
        let bytes = serde_json::to_vec_pretty(&self.avatars).map_err(|error| error.to_string())?;
        let pending = self.file.with_extension("json.new");
        std::fs::write(&pending, bytes).and_then(|()| std::fs::rename(&pending, &self.file)).map_err(|error| format!("{}: {error}", self.file.display()))
    }
}

pub fn shown(controls: &[Control], controller: &Controller) -> HashMap<String, f32> {
    let mut out = HashMap::new();
    for control in controls {
        match control.kind {
            Kind::Submenu => out.extend(shown(&control.controls, controller)),
            Kind::Button => {}
            _ => {
                for name in std::iter::once(&control.parameter).chain(&control.axes).filter(|name| !name.is_empty()) {
                    out.insert(name.clone(), controller.get(name));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vrm::expressions::Expressions;

    #[test]
    fn a_setting_survives_a_new_reader_and_stays_with_its_avatar() {
        let directory = std::env::temp_dir().join(format!("voice-vr-settings-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let _ = std::fs::remove_file(directory.join("menus.json"));
        let expressions = Expressions::parse(&serde_json::json!({
            "menu": [{"name": "Hat", "type": "toggle", "parameter": "Hat"}, {"name": "Wave", "type": "button", "parameter": "Wave"}],
            "parameters": [{"name": "Hat", "type": "bool"}, {"name": "Wave", "type": "bool"}],
            "layers": [], "clips": [], "targets": []
        }))
        .unwrap();
        let mut settings = Settings::read(&directory);
        let mut controller = Controller::new(&expressions, &settings.of("a"));
        controller.set("Hat", 1.0);
        controller.set("Wave", 1.0);
        settings.keep("a", &expressions.menu, &controller).unwrap();
        let again = Settings::read(&directory);
        assert_eq!(again.of("a"), HashMap::from([("Hat".to_owned(), 1.0)]));
        assert!(again.of("b").is_empty());
        assert_eq!(Controller::new(&expressions, &again.of("a")).get("Hat"), 1.0);
    }
}
