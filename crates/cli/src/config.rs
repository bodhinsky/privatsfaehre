use detect::{Engine, EngineConfig};
use serde::Deserialize;
use std::path::Path;

/// `privatsphaere.toml`:
///
/// ```toml
/// [detect]
/// default_threshold = 0.5
/// allowlist = ["support@example.com"]
/// denylist = [{ text = "Projekt Falke", entity = "ORGANIZATION" }]
/// [detect.thresholds]
/// DATE = 0.8
/// ```
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    detect: EngineConfig,
}

pub fn engine(path: Option<&Path>) -> Result<Engine, Box<dyn std::error::Error>> {
    let cfg = match path {
        Some(p) => {
            let s = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            toml::from_str::<FileConfig>(&s).map_err(|e| format!("{}: {e}", p.display()))?.detect
        }
        None => EngineConfig::default(),
    };
    Ok(Engine::new(cfg)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example() {
        let s = r#"
            [detect]
            default_threshold = 0.6
            allowlist = ["support@example.com"]
            denylist = [{ text = "Projekt Falke", entity = "ORGANIZATION" }]
            [detect.thresholds]
            DATE = 0.8
        "#;
        let c: FileConfig = toml::from_str(s).unwrap();
        assert_eq!(c.detect.thresholds[&detect::EntityType::Date], 0.8);
        assert!(Engine::new(c.detect).is_ok());
        let bad = "[detect.thresholds]\nHEALTH = 0.9";
        assert!(Engine::new(toml::from_str::<FileConfig>(bad).unwrap().detect).is_err());
    }
}
