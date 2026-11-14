use std::fs;

fn default_features(manifest: &str) -> Result<Vec<String>, String> {
    let mut in_features = false;
    let mut in_default = false;
    let mut features = Vec::new();

    for line in manifest.lines() {
        let line = line.trim();
        if line == "[features]" {
            in_features = true;
            continue;
        }
        if in_features && line.starts_with('[') {
            if in_default {
                return Err("default feature list is not closed".to_owned());
            }
            break;
        }
        if in_features && line == "default = [" {
            in_default = true;
            continue;
        }
        if in_default {
            if line == "]" {
                if features.is_empty() {
                    return Err("default feature list is empty".to_owned());
                }
                return Ok(features);
            }
            let feature = line
                .strip_suffix(',')
                .unwrap_or(line)
                .strip_prefix('"')
                .and_then(|line| line.strip_suffix('"'))
                .filter(|feature| !feature.is_empty())
                .ok_or_else(|| format!("invalid default feature entry: {line}"))?;
            features.push(feature.to_owned());
        }
    }

    if !in_features {
        return Err("manifest has no [features] table".to_owned());
    }
    if !in_default {
        return Err("manifest has no default feature list".to_owned());
    }
    Err("default feature list is not closed".to_owned())
}

fn documented_default_features(readme: &str) -> Result<Vec<String>, String> {
    let feature_list = readme
        .split_once("All on by default: ")
        .and_then(|(_, rest)| rest.split_once(". With a module compiled out"))
        .map(|(list, _)| list)
        .ok_or_else(|| "README must document the default feature list".to_owned())?;

    let features: Vec<_> = feature_list
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect();
    if features.is_empty() {
        return Err("README default feature list is empty".to_owned());
    }
    Ok(features)
}

#[test]
fn readme_default_features_match_manifest() {
    let manifest = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("Cargo.toml must be readable");
    let readme = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"))
        .expect("README.md must be readable");

    let manifest_features = default_features(&manifest).expect("Cargo.toml default features");
    let readme_features = documented_default_features(&readme).expect("README default features");

    assert_eq!(
        manifest_features, readme_features,
        "README default features must match Cargo.toml"
    );
}
