use detect::Engine;
use serde::Deserialize;
use std::path::Path;

/// Engine from `--config` (see [`session::config`]).
pub fn engine(path: Option<&Path>) -> Result<Engine, Box<dyn std::error::Error>> {
    session::config::engine(path)
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
    fn repo_targets_file_parses() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../accuracy-targets.toml");
        assert_eq!(targets(&path).unwrap(), [(detect::EntityType::Health, 0.98)]);
    }
}
