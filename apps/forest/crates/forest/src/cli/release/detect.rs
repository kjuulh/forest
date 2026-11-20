use crate::services::project::ProjectParserState;
use crate::state::State;
use crate::user_state::UserStateLoaderState;

use super::annotate::git_output;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Author {
    pub username: Option<String>,
    pub email: Option<String>,
    pub github_login: Option<String>,
    pub github_user_id: Option<String>,
    pub display_name: Option<String>,
}

impl Author {
    fn named(&self) -> bool {
        self.username.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Candidate {
    pub origin: &'static str,
    pub author: Author,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Detected {
    pub author: Author,
    pub origin: Option<&'static str>,
}

pub(super) const META_PREFIX: &str = "forest.author";

impl Detected {
    pub fn metadata(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut put = |key: &str, value: &Option<String>| {
            if let Some(value) = value {
                out.push((format!("{META_PREFIX}.{key}"), value.clone()));
            }
        };

        put("origin", &self.origin.map(str::to_string));
        put("github_login", &self.author.github_login);
        put("github_user_id", &self.author.github_user_id);
        put("name", &self.author.display_name);
        put("email", &self.author.email);
        out
    }
}

pub(super) fn choose(candidates: &[Candidate]) -> Detected {
    match candidates.iter().find(|c| c.author.named()) {
        Some(c) => Detected {
            author: c.author.clone(),
            origin: Some(c.origin),
        },
        None => Detected::default(),
    }
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn str_at(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|v| v.as_str()).and_then(non_empty)
}

fn json_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => non_empty(s),
        _ => None,
    }
}

pub(super) fn author_from_event(event: &serde_json::Value) -> Option<Author> {
    if let Some(author) = event.pointer("/head_commit/author") {
        let login = str_at(author, "username");
        let name = str_at(author, "name");
        let candidate = Author {
            username: login.clone().or_else(|| name.clone()),
            email: str_at(author, "email"),
            github_login: login,
            github_user_id: None,
            display_name: name,
        };
        if candidate.named() {
            return Some(candidate);
        }
    }

    if let Some(login) = event
        .pointer("/pull_request/user/login")
        .and_then(|v| v.as_str())
        .and_then(non_empty)
    {
        let id = event
            .pointer("/pull_request/user/id")
            .and_then(json_id)
            .filter(|_| true);

        return Some(Author {
            username: Some(login.clone()),
            email: None,
            github_login: Some(login),
            github_user_id: id,
            display_name: None,
        });
    }

    None
}

async fn author_from_event_file(path: &str) -> Option<Author> {
    let raw = match tokio::fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(e) => {
            tracing::debug!("GITHUB_EVENT_PATH ({path}) could not be read: {e}");
            return None;
        }
    };

    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(event) => author_from_event(&event),
        Err(e) => {
            tracing::debug!("GITHUB_EVENT_PATH ({path}) is not valid JSON: {e}");
            None
        }
    }
}

async fn github_event_author() -> Option<Author> {
    let path = env_non_empty("GITHUB_EVENT_PATH")?;
    author_from_event_file(&path).await
}

pub(super) fn assemble(
    event: Option<Author>,
    actor: Option<String>,
    actor_id: Option<String>,
    commit: Author,
    auth: Author,
    config: Author,
) -> Vec<Candidate> {
    let mut event = event.unwrap_or_default();

    if event.github_user_id.is_none()
        && let (Some(login), Some(actor)) = (&event.github_login, &actor)
        && login.eq_ignore_ascii_case(actor)
    {
        event.github_user_id = actor_id.clone();
    }

    vec![
        Candidate {
            origin: "github-event",
            author: event,
        },
        Candidate {
            origin: "github-actor",
            author: Author {
                username: actor.clone(),
                email: None,
                github_login: actor,
                github_user_id: actor_id,
                display_name: None,
            },
        },
        Candidate {
            origin: "git-commit",
            author: commit,
        },
        Candidate {
            origin: "forest-auth",
            author: auth,
        },
        Candidate {
            origin: "git-config",
            author: config,
        },
    ]
}

async fn candidates(state: &State) -> Vec<Candidate> {
    let (event, commit_name, commit_email, config_name, config_email) = tokio::join!(
        github_event_author(),
        git_output(&["log", "-1", "--format=%an"]),
        git_output(&["log", "-1", "--format=%ae"]),
        git_output(&["config", "user.name"]),
        git_output(&["config", "user.email"]),
    );

    let actor = env_non_empty("GITHUB_ACTOR").or_else(|| env_non_empty("GITHUB_TRIGGERING_ACTOR"));
    let actor_id =
        env_non_empty("GITHUB_ACTOR_ID").or_else(|| env_non_empty("GITHUB_TRIGGERING_ACTOR_ID"));

    let auth = match state.user_state().get_state().await {
        Ok(Some(user)) => Author {
            username: non_empty(&user.username),
            email: user.emails.into_iter().find_map(|e| non_empty(&e)),
            ..Author::default()
        },
        Ok(None) => Author::default(),
        Err(e) => {
            tracing::debug!("could not read auth state for author detection: {e:#}");
            Author::default()
        }
    };

    assemble(
        event,
        actor,
        actor_id,
        Author {
            username: commit_name.clone(),
            email: commit_email,
            display_name: commit_name,
            ..Author::default()
        },
        auth,
        Author {
            username: config_name,
            email: config_email,
            ..Author::default()
        },
    )
}

pub(crate) async fn project(state: &State) -> (Option<String>, Option<String>) {
    match state.project_parser().get_project().await {
        Ok(project) => (project.organisation.clone(), Some(project.name.clone())),
        Err(e) => {
            tracing::debug!("no project file to read organisation/project from: {e:#}");
            (None, None)
        }
    }
}

pub(super) async fn resolve(
    state: &State,
    username: Option<String>,
    email: Option<String>,
    detect: bool,
) -> Attribution {
    if !detect || username.is_some() || email.is_some() {
        return Attribution {
            username,
            email,
            metadata: Vec::new(),
        };
    }

    let detected = choose(&candidates(state).await);

    match detected.origin {
        Some(origin) => tracing::info!(
            "detected release author from {origin}: {} <{}>",
            detected.author.username.as_deref().unwrap_or("?"),
            detected.author.email.as_deref().unwrap_or("?"),
        ),
        None => tracing::warn!(
            "--detect found nothing in the environment to attribute this release to; \
             the server will fall back to the authenticated token's owner"
        ),
    }

    Attribution {
        username: detected.author.username.clone(),
        email: detected.author.email.clone(),
        metadata: detected.metadata(),
    }
}

pub(super) struct Attribution {
    pub username: Option<String>,
    pub email: Option<String>,
    pub metadata: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn author(username: Option<&str>, email: Option<&str>) -> Author {
        Author {
            username: username.map(str::to_string),
            email: email.map(str::to_string),
            ..Author::default()
        }
    }

    fn gh(login: &str, email: Option<&str>, id: Option<&str>) -> Author {
        Author {
            username: Some(login.to_string()),
            email: email.map(str::to_string),
            github_login: Some(login.to_string()),
            github_user_id: id.map(str::to_string),
            display_name: None,
        }
    }

    #[test]
    fn a_ci_push_is_attributed_to_the_commit_author_not_the_token_owner() {
        let chosen = choose(&assemble(
            Some(author(Some("dentych"), Some("dennis@understory.io"))),
            Some("dentych".to_string()),
            Some("2256372".to_string()),
            author(Some("Dennis Tychsen"), Some("dennis@understory.io")),
            author(Some("kjuulh"), Some("kasper@understory.io")),
            author(None, None),
        ));

        assert_eq!(chosen.author.username.as_deref(), Some("dentych"));
        assert_eq!(chosen.author.email.as_deref(), Some("dennis@understory.io"));
        assert_eq!(chosen.origin, Some("github-event"));
    }

    #[test]
    fn a_commit_the_token_owner_wrote_still_says_the_token_owner() {
        let chosen = choose(&assemble(
            Some(author(Some("kjuulh"), Some("kasper@understory.io"))),
            Some("kjuulh".to_string()),
            Some("26280046".to_string()),
            author(Some("Kasper Juul Hermansen"), Some("kasper@understory.io")),
            author(Some("kjuulh"), Some("kasper@understory.io")),
            author(None, None),
        ));

        assert_eq!(chosen.author.username.as_deref(), Some("kjuulh"));
    }

    #[test]
    fn outside_ci_the_commit_author_still_beats_the_signed_in_user() {
        let chosen = choose(&assemble(
            None,
            None,
            None,
            author(Some("Dennis Tychsen"), Some("dennis@understory.io")),
            author(Some("kjuulh"), Some("kasper@understory.io")),
            author(Some("Kasper Juul Hermansen"), Some("kasper@understory.io")),
        ));

        assert_eq!(chosen.author.username.as_deref(), Some("Dennis Tychsen"));
        assert_eq!(chosen.origin, Some("git-commit"));
    }

    #[test]
    fn with_no_commit_the_signed_in_user_answers() {
        let chosen = choose(&assemble(
            None,
            None,
            None,
            author(None, None),
            author(Some("kjuulh"), Some("kasper@understory.io")),
            author(None, None),
        ));

        assert_eq!(chosen.author.username.as_deref(), Some("kjuulh"));
        assert_eq!(chosen.origin, Some("forest-auth"));
    }

    #[test]
    fn the_actor_id_is_lent_to_the_author_when_they_are_the_same_person() {
        let candidates = assemble(
            Some(gh("dentych", Some("dennis@understory.io"), None)),
            Some("dentych".to_string()),
            Some("2256372".to_string()),
            author(None, None),
            author(None, None),
            author(None, None),
        );

        let chosen = choose(&candidates);
        assert_eq!(chosen.origin, Some("github-event"));
        assert_eq!(chosen.author.github_user_id.as_deref(), Some("2256372"));
    }

    #[test]
    fn the_actor_id_is_withheld_when_the_actor_did_not_write_the_commit() {
        let candidates = assemble(
            Some(gh("dentych", Some("dennis@understory.io"), None)),
            Some("kjuulh".to_string()),
            Some("26280046".to_string()),
            author(None, None),
            author(None, None),
            author(None, None),
        );

        let chosen = choose(&candidates);
        assert_eq!(chosen.author.github_login.as_deref(), Some("dentych"));
        assert_eq!(chosen.author.github_user_id, None);
    }

    #[test]
    fn the_actor_match_ignores_case() {
        let candidates = assemble(
            Some(gh("DenTych", None, None)),
            Some("dentych".to_string()),
            Some("2256372".to_string()),
            author(None, None),
            author(None, None),
            author(None, None),
        );

        assert_eq!(
            choose(&candidates).author.github_user_id.as_deref(),
            Some("2256372")
        );
    }

    #[test]
    fn the_sniffed_signals_are_kept_as_metadata() {
        let detected = choose(&assemble(
            Some(Author {
                username: Some("dentych".into()),
                email: Some("dennis@understory.io".into()),
                github_login: Some("dentych".into()),
                github_user_id: None,
                display_name: Some("Dennis Tychsen".into()),
            }),
            Some("dentych".to_string()),
            Some("2256372".to_string()),
            author(None, None),
            author(None, None),
            author(None, None),
        ));

        let meta: std::collections::HashMap<String, String> =
            detected.metadata().into_iter().collect();

        assert_eq!(meta["forest.author.origin"], "github-event");
        assert_eq!(meta["forest.author.github_login"], "dentych");
        assert_eq!(meta["forest.author.github_user_id"], "2256372");
        assert_eq!(meta["forest.author.name"], "Dennis Tychsen");
        assert_eq!(meta["forest.author.email"], "dennis@understory.io");
    }

    #[test]
    fn detecting_nobody_writes_no_metadata() {
        assert!(Detected::default().metadata().is_empty());
    }

    #[test]
    fn a_pull_request_event_carries_the_openers_id() {
        let event = serde_json::json!({
            "pull_request": { "user": { "login": "dentych", "id": 2256372 } }
        });

        let author = author_from_event(&event).expect("names an author");

        assert_eq!(author.github_login.as_deref(), Some("dentych"));
        assert_eq!(author.github_user_id.as_deref(), Some("2256372"));
    }

    fn candidate(origin: &'static str, username: Option<&str>, email: Option<&str>) -> Candidate {
        Candidate {
            origin,
            author: author(username, email),
        }
    }

    #[test]
    fn the_first_named_candidate_wins() {
        let chosen = choose(&[
            candidate("github-event", None, None),
            candidate("github-actor", Some("dentych"), None),
            candidate(
                "git-commit",
                Some("Dennis Tychsen"),
                Some("dennis@understory.io"),
            ),
        ]);

        assert_eq!(chosen.author.username.as_deref(), Some("dentych"));
        assert_eq!(chosen.origin, Some("github-actor"));
    }

    #[test]
    fn an_email_is_never_borrowed_from_another_candidate() {
        let chosen = choose(&[
            candidate("github-actor", Some("dentych"), None),
            candidate(
                "git-commit",
                Some("Someone Else"),
                Some("someone@understory.io"),
            ),
        ]);

        assert_eq!(chosen.author.username.as_deref(), Some("dentych"));
        assert_eq!(chosen.author.email, None);
    }

    #[test]
    fn nothing_in_the_environment_detects_nobody() {
        let chosen = choose(&[
            candidate("github-event", None, None),
            candidate("github-actor", None, None),
        ]);

        assert_eq!(chosen, Detected::default());
    }

    #[test]
    fn an_email_alone_does_not_name_anybody() {
        let chosen = choose(&[candidate("git-commit", None, Some("dennis@understory.io"))]);

        assert_eq!(chosen, Detected::default());
    }

    #[test]
    fn push_event_yields_the_commit_author_login() {
        let event = serde_json::json!({
            "head_commit": {
                "id": "8f4f8da0e1485df1fccf3def1b91d78b5d99ce4a",
                "message": "DATA-655 give dennis a 16 GB box (#21)",
                "author": {
                    "name": "Dennis Tychsen",
                    "email": "dennis@understory.io",
                    "username": "dentych"
                }
            },
            "pusher": { "name": "dentych", "email": "dennis@understory.io" },
            "sender": { "login": "dentych" }
        });

        let author = author_from_event(&event).expect("push event names an author");

        assert_eq!(author.username.as_deref(), Some("dentych"));
        assert_eq!(author.email.as_deref(), Some("dennis@understory.io"));
    }

    #[test]
    fn push_event_falls_back_to_the_display_name() {
        let event = serde_json::json!({
            "head_commit": {
                "author": { "name": "Dennis Tychsen", "email": "dennis@understory.io" }
            }
        });

        let author = author_from_event(&event).expect("push event names an author");

        assert_eq!(author.username.as_deref(), Some("Dennis Tychsen"));
        assert_eq!(author.email.as_deref(), Some("dennis@understory.io"));
    }

    #[test]
    fn pull_request_event_yields_the_opener() {
        let event = serde_json::json!({
            "pull_request": { "user": { "login": "dentych" } }
        });

        let author = author_from_event(&event).expect("pull_request event names an author");

        assert_eq!(author.username.as_deref(), Some("dentych"));
        assert_eq!(author.email, None);
    }

    #[test]
    fn an_event_with_nobody_on_it_detects_nobody() {
        let event = serde_json::json!({ "schedule": "0 3 * * *" });

        assert_eq!(author_from_event(&event), None);
    }

    #[test]
    fn blank_fields_are_not_an_author() {
        let event = serde_json::json!({
            "head_commit": { "author": { "name": "", "email": "", "username": "" } }
        });

        assert_eq!(author_from_event(&event), None);
    }

    #[tokio::test]
    async fn an_event_file_is_read_from_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("event.json");
        std::fs::write(
            &path,
            r#"{"head_commit":{"author":{"username":"dentych","email":"dennis@understory.io"}}}"#,
        )
        .expect("write event");

        let author = author_from_event_file(&path.to_string_lossy())
            .await
            .expect("event file names an author");

        assert_eq!(author.username.as_deref(), Some("dentych"));
        assert_eq!(author.email.as_deref(), Some("dennis@understory.io"));
    }

    #[tokio::test]
    async fn a_missing_or_broken_event_file_is_not_fatal() {
        assert_eq!(
            author_from_event_file("/nonexistent/event.json").await,
            None
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("truncated.json");
        std::fs::write(&path, r#"{"head_commit":{"author":{"userna"#).expect("write");

        assert_eq!(author_from_event_file(&path.to_string_lossy()).await, None);
    }
}
