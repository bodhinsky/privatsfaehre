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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetsFile {
    watch_mode: WatchTargets,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WatchTargets {
    min_recall: std::collections::BTreeMap<detect::EntityType, f64>,
}

/// Watch-mode recall targets from `accuracy-targets.toml`.
pub fn targets(path: &Path) -> Result<Vec<(detect::EntityType, f64)>, Box<dyn std::error::Error>> {
    let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let t: TargetsFile = toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(t.watch_mode.min_recall.into_iter().collect())
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

    #[test]
    fn repo_targets_file_parses() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../accuracy-targets.toml");
        assert_eq!(targets(&path).unwrap(), [(detect::EntityType::Health, 0.98)]);
    }
}
