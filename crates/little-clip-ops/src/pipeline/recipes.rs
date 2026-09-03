use super::catalog::{find_template, instantiate_template};
use super::types::{Connection, NodeGraph, Recipe};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

pub fn get_recipes_dir() -> PathBuf {
    if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(config_home)
            .join("little-screen-clip")
            .join("recipes")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".config")
            .join("little-screen-clip")
            .join("recipes")
    } else {
        PathBuf::from("/tmp/little-screen-clip/recipes")
    }
}

pub fn ensure_recipes_dir_and_defaults() -> Result<PathBuf, std::io::Error> {
    let dir = get_recipes_dir();
    if !dir.exists() {
        fs::create_dir_all(&dir)?;
        init_default_recipes(&dir)?;
    } else {
        // If directory exists but is empty, initialize default recipes
        let is_empty = fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().map_or(false, |ext| ext == "yaml" || ext == "yml"))
            .count()
            == 0;
        if is_empty {
            init_default_recipes(&dir)?;
        }
    }
    Ok(dir)
}

pub fn load_all_recipes() -> Vec<Recipe> {
    let dir = match ensure_recipes_dir_and_defaults() {
        Ok(d) => d,
        Err(e) => {
            warn!("Failed to ensure recipes dir: {}", e);
            return vec![make_pad_and_caption_recipe()];
        }
    };

    let mut recipes = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "yaml" || ext == "yml") {
                if let Ok(content) = fs::read_to_string(&path) {
                    match serde_yaml::from_str::<Recipe>(&content) {
                        Ok(recipe) => recipes.push(recipe),
                        Err(e) => warn!("Failed to parse recipe {}: {}", path.display(), e),
                    }
                }
            }
        }
    }

    if recipes.is_empty() {
        recipes.push(make_pad_and_caption_recipe());
    }

    // Sort alphabetically by name
    recipes.sort_by(|a, b| a.name.cmp(&b.name));
    recipes
}

pub fn save_recipe(recipe: &Recipe) -> Result<PathBuf, std::io::Error> {
    let dir = ensure_recipes_dir_and_defaults()?;
    let safe_name: String = recipe
        .name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    let filename = format!("{}.yaml", safe_name.to_lowercase());
    let path = dir.join(filename);

    let yaml = serde_yaml::to_string(recipe)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    fs::write(&path, yaml)?;
    info!("Saved recipe {} to {}", recipe.name, path.display());
    Ok(path)
}

pub fn find_recipe_by_name(name: &str) -> Option<Recipe> {
    let recipes = load_all_recipes();
    recipes.into_iter().find(|r| {
        r.name.eq_ignore_ascii_case(name)
            || r.name
                .to_lowercase()
                .replace(' ', "_")
                .contains(&name.to_lowercase().replace(' ', "_"))
    })
}

fn init_default_recipes(dir: &Path) -> Result<(), std::io::Error> {
    let defaults = vec![
        make_pad_and_caption_recipe(),
        make_normalize_audio_recipe(),
        make_clean_and_slugify_recipe(),
    ];

    for r in defaults {
        let safe_name: String = r
            .name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect();
        let path = dir.join(format!("{}.yaml", safe_name.to_lowercase()));
        if !path.exists() {
            let yaml = serde_yaml::to_string(&r)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
            fs::write(path, yaml)?;
        }
    }
    Ok(())
}

/// The canonical Section 7.1 blueprint recipe:
/// Left Hand (Picture) + Right Hand (Text) -> Pad with frame -> Add caption -> Set in Left Hand
pub fn make_pad_and_caption_recipe() -> Recipe {
    let t_lh = find_template("source_lh").unwrap();
    let t_rh = find_template("source_rh").unwrap();
    let t_pad = find_template("img_pad_frame").unwrap();
    let t_caption = find_template("img_add_title").unwrap();
    let t_sink = find_template("sink_lh").unwrap();

    let node_lh = instantiate_template(t_lh, [60.0, 100.0]);
    let node_rh = instantiate_template(t_rh, [60.0, 300.0]);
    let node_pad = instantiate_template(t_pad, [320.0, 100.0]);
    let node_caption = instantiate_template(t_caption, [580.0, 180.0]);
    let node_sink = instantiate_template(t_sink, [860.0, 180.0]);

    let connections = vec![
        // LH Out -> Pad Image In
        Connection {
            from_node: node_lh.id,
            from_pin: "Out".to_string(),
            to_node: node_pad.id,
            to_pin: "Image In".to_string(),
        },
        // Pad Image Out -> Caption Image In
        Connection {
            from_node: node_pad.id,
            from_pin: "Image Out".to_string(),
            to_node: node_caption.id,
            to_pin: "Image In".to_string(),
        },
        // RH Out -> Caption Caption
        Connection {
            from_node: node_rh.id,
            from_pin: "Out".to_string(),
            to_node: node_caption.id,
            to_pin: "Caption".to_string(),
        },
        // Caption Image Out -> Sink In
        Connection {
            from_node: node_caption.id,
            from_pin: "Image Out".to_string(),
            to_node: node_sink.id,
            to_pin: "In".to_string(),
        },
    ];

    Recipe {
        name: "Pad and Caption".to_string(),
        description: "Add stylish border padding and embed Right Hand text caption at bottom (ImageMagick)"
            .to_string(),
        tags: vec![
            "image".to_string(),
            "frame".to_string(),
            "caption".to_string(),
            "dual-wield".to_string(),
        ],
        graph: NodeGraph {
            nodes: vec![node_lh, node_rh, node_pad, node_caption, node_sink],
            connections,
            pan: [0.0, 0.0],
            zoom: 1.0,
        },
    }
}

pub fn make_normalize_audio_recipe() -> Recipe {
    let t_lh = find_template("source_lh").unwrap();
    let t_norm = find_template("audio_normalize").unwrap();
    let t_sink = find_template("sink_lh").unwrap();

    let node_lh = instantiate_template(t_lh, [80.0, 140.0]);
    let node_norm = instantiate_template(t_norm, [360.0, 140.0]);
    let node_sink = instantiate_template(t_sink, [640.0, 140.0]);

    let connections = vec![
        Connection {
            from_node: node_lh.id,
            from_pin: "Out".to_string(),
            to_node: node_norm.id,
            to_pin: "Audio In".to_string(),
        },
        Connection {
            from_node: node_norm.id,
            from_pin: "Audio Out".to_string(),
            to_node: node_sink.id,
            to_pin: "In".to_string(),
        },
    ];

    Recipe {
        name: "Normalize Audio (14 LUFS)".to_string(),
        description: "Broadcast-standard loudness normalization for audio in Left Hand (FFmpeg)"
            .to_string(),
        tags: vec![
            "audio".to_string(),
            "normalize".to_string(),
            "loudness".to_string(),
            "lufs".to_string(),
        ],
        graph: NodeGraph {
            nodes: vec![node_lh, node_norm, node_sink],
            connections,
            pan: [0.0, 0.0],
            zoom: 1.0,
        },
    }
}

pub fn make_clean_and_slugify_recipe() -> Recipe {
    let t_lh = find_template("source_lh").unwrap();
    let t_regex = find_template("text_regex_replace").unwrap();
    let t_slug = find_template("text_slugify").unwrap();
    let t_sink = find_template("sink_lh").unwrap();

    let node_lh = instantiate_template(t_lh, [80.0, 140.0]);
    let mut node_regex = instantiate_template(t_regex, [320.0, 140.0]);
    node_regex.props.insert("pattern".to_string(), r"[-_\s]+".to_string());
    node_regex.props.insert("replacement".to_string(), " ".to_string());


    let node_slug = instantiate_template(t_slug, [580.0, 140.0]);
    let node_sink = instantiate_template(t_sink, [820.0, 140.0]);

    let connections = vec![
        Connection {
            from_node: node_lh.id,
            from_pin: "Out".to_string(),
            to_node: node_regex.id,
            to_pin: "In".to_string(),
        },
        Connection {
            from_node: node_regex.id,
            from_pin: "Out".to_string(),
            to_node: node_slug.id,
            to_pin: "In".to_string(),
        },
        Connection {
            from_node: node_slug.id,
            from_pin: "Out".to_string(),
            to_node: node_sink.id,
            to_pin: "In".to_string(),
        },
    ];

    Recipe {
        name: "Clean and Slugify".to_string(),
        description: "Normalize separators and transform Left Hand string into URL-safe kebab slug"
            .to_string(),
        tags: vec![
            "text".to_string(),
            "slug".to_string(),
            "kebab".to_string(),
            "url".to_string(),
        ],
        graph: NodeGraph {
            nodes: vec![node_lh, node_regex, node_slug, node_sink],
            connections,
            pan: [0.0, 0.0],
            zoom: 1.0,
        },
    }
}
