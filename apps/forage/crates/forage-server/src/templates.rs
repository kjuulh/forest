use std::path::Path;

use anyhow::Context;
use minijinja::Environment;
use sha2::Digest;

#[derive(Clone)]
pub struct TemplateEngine {
    env: Environment<'static>,
    assets: forage_ui::chrome::Assets,
    build: forage_ui::chrome::BuildStamp,
}

impl TemplateEngine {
    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            anyhow::bail!("templates directory not found: {}", path.display());
        }

        let mut env = Environment::new();
        env.set_loader(minijinja::path_loader(path));
        env.set_formatter(|out, state, value| {
            if value.is_none() || value.is_undefined() {
                return Ok(());
            }
            minijinja::escape_formatter(out, state, value)
        });
        env.add_filter("timeago", |v: String| -> String {
            forage_ui::format::timeago(&v)
        });
        env.add_filter("timeuntil", |v: String| -> String {
            forage_ui::format::timeuntil(&v)
        });
        env.add_filter("datetime", |v: String| -> String {
            forage_ui::format::datetime(&v)
        });
        env.add_filter("urlencode", |v: String| -> String {
            forage_ui::format::urlencode(&v)
        });
        env.add_filter("short_sha", |v: String| -> String {
            crate::manifest_view::ManifestView::short_sha(&v)
        });
        env.add_filter("human_size", |v: u64| -> String {
            crate::manifest_view::ManifestView::human_size(v)
        });
        env.add_filter("anchor", |v: String| -> String {
            forage_core::integrations::slack_release::anchor_id(&v)
        });
        env.add_filter("prettify_url", |v: String| -> String {
            forage_core::platform::prettify_url(&v)
        });

        env.add_global("css_hash", "dev");
        env.add_global("ui_js_hash", "dev");
        env.add_global("components_js_hash", "dev");

        let build = crate::build_info::BuildInfo::from_env();
        env.add_global("build_commit", build.commit.clone());
        env.add_global("build_time", build.build_time.clone());

        Ok(Self {
            env,
            assets: forage_ui::chrome::Assets::default(),
            build: forage_ui::chrome::BuildStamp {
                commit: build.commit,
                time: build.build_time,
            },
        })
    }

    pub fn new() -> anyhow::Result<Self> {
        let mut engine = Self::from_path(Path::new("templates"))?;
        engine.compute_asset_hashes();
        Ok(engine)
    }

    fn compute_asset_hashes(&mut self) {
        fn asset_hash(path: &str) -> String {
            match std::fs::read(path) {
                Ok(bytes) => {
                    let digest = sha2::Sha256::digest(&bytes);
                    format!("{digest:x}")[..12].to_string()
                }
                Err(_) => "dev".to_string(),
            }
        }

        self.assets = forage_ui::chrome::Assets {
            css_hash: asset_hash("static/css/style.css"),
            ui_js_hash: asset_hash("static/js/ui.js"),
            components_js_hash: asset_hash("static/js/components/forage-components.js"),
        };
        self.env
            .add_global("css_hash", self.assets.css_hash.clone());
        self.env
            .add_global("ui_js_hash", self.assets.ui_js_hash.clone());
        self.env
            .add_global("components_js_hash", self.assets.components_js_hash.clone());
    }

    pub fn assets(&self) -> &forage_ui::chrome::Assets {
        &self.assets
    }

    pub fn build_stamp(&self) -> &forage_ui::chrome::BuildStamp {
        &self.build
    }

    pub fn asset_href(&self, path: &str) -> String {
        static HASHES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
            std::sync::LazyLock::new(Default::default);
        let mut hashes = HASHES.lock().unwrap_or_else(|e| e.into_inner());
        let hash = hashes.entry(path.to_string()).or_insert_with(|| {
            match std::fs::read(Path::new("static").join(path)) {
                Ok(bytes) => format!("{:x}", sha2::Sha256::digest(&bytes))[..12].to_string(),
                Err(_) => "dev".to_string(),
            }
        });
        format!("/static/{path}?v={hash}")
    }

    #[cfg(test)]
    pub fn render_str(&self, source: &str, ctx: minijinja::Value) -> anyhow::Result<String> {
        Ok(self.env.render_str(source, ctx)?)
    }

    pub fn render(&self, template: &str, ctx: minijinja::Value) -> anyhow::Result<String> {
        let tmpl = self
            .env
            .get_template(template)
            .with_context(|| format!("template not found: {template}"))?;
        tmpl.render(ctx)
            .with_context(|| format!("failed to render template: {template}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_renders_as_nothing_and_everything_else_is_still_escaped() {
        let engine = TemplateEngine::from_path(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates"),
        )
        .expect("load templates");
        let html = engine
            .env
            .render_named_str(
                "test.html.jinja",
                r#"<input value="{{ unset }}">{{ missing }}|{{ text }}|{{ zero }}|{{ no }}"#,
                minijinja::context! { unset => None::<String>, text => "<b>", zero => 0, no => false },
            )
            .unwrap();
        assert_eq!(html, r#"<input value="">|&lt;b&gt;|0|false"#);
    }

    #[test]
    fn every_page_template_parses() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("templates");
        let engine = TemplateEngine::from_path(&root).expect("load templates");

        let pages = root.join("pages");
        let mut checked = 0;
        let mut failures = Vec::new();
        for entry in std::fs::read_dir(&pages).expect("read pages dir") {
            let path = entry.expect("dir entry").path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.ends_with(".jinja") {
                continue;
            }
            let rel = format!("pages/{name}");
            if let Err(e) = engine.env.get_template(&rel) {
                failures.push(format!("{rel}: {e}"));
            }
            checked += 1;
        }

        assert!(checked > 0, "found no page templates under {pages:?}");
        assert!(failures.is_empty(), "templates failed to parse:\n{failures:#?}");
    }
}


#[cfg(test)]
mod timeline_prefetch_tests {
    use super::*;

    fn engine() -> TemplateEngine {
        TemplateEngine::from_path(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates"))
            .expect("template engine")
    }

    fn releases_page(org: &str) -> String {
        engine()
            .render(
                "pages/releases.html.jinja",
                minijinja::context! {
                    title => "t", description => "d",
                    user => minijinja::context! { username => "u" },
                    csrf_token => "c", current_org => org,
                    orgs => Vec::<minijinja::Value>::new(),
                    org_name => org,
                    env_options => Vec::<minijinja::Value>::new(),
                    active_tab => "releases",
                },
            )
            .expect("render")
    }

    #[test]
    fn the_page_starts_the_timeline_request() {
        let html = releases_page("understory");
        assert!(
            html.contains("__forestTimeline"),
            "the releases page no longer primes the timeline; \
             <release-timeline> will wait for the bundle before it asks for data"
        );
    }

    #[test]
    fn the_url_cannot_break_out_of_its_attribute() {
        let html = releases_page("evil\"><script>alert(1)</script>");
        assert!(
            !html.contains("<script>alert(1)</script>"),
            "an org name escaped its attribute:\n{html}"
        );
    }
}
