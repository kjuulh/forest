use std::path::Path;

use sha2::Digest;

fn bytes_hash(bytes: &[u8]) -> String {
    format!("{:x}", sha2::Sha256::digest(bytes))[..12].to_string()
}

fn content_hash(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => bytes_hash(&bytes),
        Err(_) => "dev".to_string(),
    }
}

fn asset_href(path: &str) -> String {
    static HASHES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::LazyLock::new(Default::default);
    let mut hashes = HASHES.lock().unwrap_or_else(|e| e.into_inner());
    let hash = hashes
        .entry(path.to_string())
        .or_insert_with(|| content_hash(&Path::new("static").join(path)));
    format!("/static/{path}?v={hash}")
}

#[derive(Clone)]
pub struct TemplateEngine {
    assets: forage_ui::chrome::Assets,
    build: forage_ui::chrome::BuildStamp,
}

impl TemplateEngine {
    pub fn unhashed() -> Self {
        let build = crate::build_info::BuildInfo::from_env();
        Self {
            assets: forage_ui::chrome::Assets::default(),
            build: forage_ui::chrome::BuildStamp {
                commit: build.commit,
                time: build.build_time,
            },
        }
    }

    pub fn new() -> anyhow::Result<Self> {
        let mut engine = Self::unhashed();
        engine.assets = forage_ui::chrome::Assets {
            css_hash: content_hash(Path::new("static/css/style.css")),
            ui_js_hash: content_hash(Path::new("static/js/ui.js")),
            components_js_hash: content_hash(Path::new(
                "static/js/components/forage-components.js",
            )),
            sedx_js_hash: bytes_hash(sedx::JS.as_bytes()),
        };
        Ok(engine)
    }

    pub fn assets(&self) -> &forage_ui::chrome::Assets {
        &self.assets
    }

    pub fn build_stamp(&self) -> &forage_ui::chrome::BuildStamp {
        &self.build
    }

    pub fn asset_href(&self, path: &str) -> String {
        asset_href(path)
    }
}
