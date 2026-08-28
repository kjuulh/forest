//! Working out who a release should be attributed to when nobody said.
//!
//! `--source-username` is the explicit answer, and CI almost never passes it —
//! the workflows that annotate releases were written to hand forest a commit,
//! not an identity. What they authenticate with is a personal access token, so
//! the blank gets filled in with that token's owner, and every release from
//! every repo ends up wearing the same name no matter who wrote the change.
//!
//! The runner already knows the answer three times over. A GitHub Actions job
//! has the push event it was started for sitting on disk at `GITHUB_EVENT_PATH`
//! — the same payload a webhook receiver would be sent — plus `GITHUB_ACTOR` in
//! the environment and the commit itself in the checkout. `--detect` reads
//! them, in that order, and fills in only what the caller left blank.
//!
//! Order matters for more than accuracy. `source_user` is rendered as a forest
//! username: the release view asks `/avatars/<user>` for the picture and links
//! `/users/<user>`, and both resolve by username. GitHub logins are what forest
//! usernames are made from for anyone who signed in through GitHub, so
//! `head_commit.author.username` and `GITHUB_ACTOR` — logins — rank above `git
//! log --format=%an`, which is a display name ("Dennis Tychsen") and resolves
//! to nobody.

use crate::services::project::ProjectParserState;
use crate::state::State;
use crate::user_state::UserStateLoaderState;

use super::annotate::git_output;

/// An identity a release can be attributed to.
///
/// The two halves travel together on purpose. Each candidate below describes
/// one person from one source, and nothing ever borrows a username from one
/// and an email from another — a `workflow_dispatch` run is exactly the case
/// where the person who pressed the button and the person who wrote `HEAD` are
/// different, and pairing one's name with the other's address would invent a
/// third person who does not exist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Author {
    /// The best handle for this person — a GitHub login where the rung has
    /// one, a display name where it does not.
    pub username: Option<String>,
    pub email: Option<String>,
    /// The GitHub login, when this rung is a GitHub one. Kept apart from
    /// `username` because the server links on it: a forest username and a
    /// GitHub login are the same string for most people here and are not
    /// required to be.
    pub github_login: Option<String>,
    /// GitHub's numeric account id — stable across renames, which logins are
    /// not, so it is the key the server prefers to link on. Only ever set when
    /// it provably belongs to `github_login`; see `assemble`.
    pub github_user_id: Option<String>,
    /// The human-readable name off the commit, when the rung has one distinct
    /// from the handle ("Dennis Tychsen" against `dentych`).
    pub display_name: Option<String>,
}

impl Author {
    fn named(&self) -> bool {
        self.username.is_some()
    }
}

/// A candidate identity together with the name of the signal it came from,
/// which `--detect` logs so a surprising attribution can be traced back to
/// whichever rung produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Candidate {
    pub origin: &'static str,
    pub author: Author,
}

/// What `--detect` concluded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Detected {
    pub author: Author,
    /// The rung that answered, or `None` when nothing in the environment did.
    pub origin: Option<&'static str>,
}

/// Metadata key prefix. Namespaced because `--metadata` is a free-for-all that
/// projects put their own keys in, and the server reads these back by name.
pub(super) const META_PREFIX: &str = "forest.author";

impl Detected {
    /// The raw signals, verbatim, to travel with the annotation.
    ///
    /// The server resolves these to a forest user where it can, but it records
    /// them either way: a commit by somebody with no forest account still says
    /// who wrote it, and a resolution that later turns out wrong can be read
    /// back against what was actually sniffed rather than guessed at.
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

/// Pick the first candidate that names somebody.
///
/// Kept separate from the reading of the environment so the precedence rules
/// are testable without a git repository, a GitHub runner, or a login.
pub(super) fn choose(candidates: &[Candidate]) -> Detected {
    match candidates.iter().find(|c| c.author.named()) {
        Some(c) => Detected {
            author: c.author.clone(),
            origin: Some(c.origin),
        },
        None => Detected::default(),
    }
}

/// Read `$name`, treating unset and empty as the same thing. GitHub Actions
/// exports the variables it has no value for as empty strings rather than
/// leaving them out, so `is_some()` alone would accept `""` as an author.
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

/// GitHub account ids are JSON numbers on webhook payloads and strings in the
/// environment. Normalise to the string the server stores them as.
fn json_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => non_empty(s),
        _ => None,
    }
}

/// Pull the author out of the event payload GitHub Actions wrote for this job.
///
/// Pure, and takes the parsed payload rather than a path, so the shapes this
/// has to cope with can be pinned down in tests.
pub(super) fn author_from_event(event: &serde_json::Value) -> Option<Author> {
    // A push carries the commit that was pushed, which is the change being
    // released. `username` is the author's GitHub login and the field a plain
    // checkout cannot give us; `name` is the display name, kept only so a
    // commit whose author never linked a GitHub account still says something.
    if let Some(author) = event.pointer("/head_commit/author") {
        let login = str_at(author, "username");
        let name = str_at(author, "name");
        let candidate = Author {
            username: login.clone().or_else(|| name.clone()),
            email: str_at(author, "email"),
            github_login: login,
            // A push payload names the author by login and never by id. The id
            // is recoverable when the author is also the actor the run belongs
            // to, which `assemble` checks; it cannot be taken from `sender`
            // here, because on a pull request merged by somebody else the
            // sender is the person who pressed the button.
            github_user_id: None,
            display_name: name,
        };
        if candidate.named() {
            return Some(candidate);
        }
    }

    // A `pull_request` event has no commit on it, but the person whose change
    // is about to ship is the one who opened it. GitHub does not put an email
    // on this object, and guessing one from elsewhere is the mixing this
    // module refuses to do, so the email stays blank.
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
            // Unlike a push, this object carries the opener's id directly.
            github_user_id: id,
            display_name: None,
        });
    }

    None
}

/// Read and parse the event payload at `path`. Unreadable and malformed mean
/// the same thing here — this rung has no answer, try the next one.
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

/// The `GITHUB_EVENT_PATH` rung. Absent outside GitHub Actions, which is not an
/// error — every other rung still applies.
async fn github_event_author() -> Option<Author> {
    let path = env_non_empty("GITHUB_EVENT_PATH")?;
    author_from_event_file(&path).await
}

/// Put the rungs in order. Pure, so the precedence that actually decides an
/// attribution can be tested without a runner, a repository, or a login.
///
/// The signed-in user is next to last. `--detect` answers "who wrote this", and
/// in CI — the only place the flag is meant to be used — the signed-in user is
/// the CI token's owner, which is precisely the wrong answer the flag exists to
/// stop being given. It stays in the list at all only so that a developer who
/// passes `--detect` by hand, outside a checkout, still gets attributed.
pub(super) fn assemble(
    event: Option<Author>,
    actor: Option<String>,
    actor_id: Option<String>,
    commit: Author,
    auth: Author,
    config: Author,
) -> Vec<Candidate> {
    let mut event = event.unwrap_or_default();

    // A push payload names the commit's author by login only. `GITHUB_ACTOR_ID`
    // is an id, but of the actor the run belongs to — the same person only when
    // the logins agree, which is the ordinary case of pushing or merging your
    // own work. Requiring the match is what keeps somebody else's merge from
    // resolving Dennis's commit to the merger's forest account.
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

/// Everything the environment has to say about who wrote this, best first.
async fn candidates(state: &State) -> Vec<Candidate> {
    let (event, commit_name, commit_email, config_name, config_email) = tokio::join!(
        github_event_author(),
        git_output(&["log", "-1", "--format=%an"]),
        git_output(&["log", "-1", "--format=%ae"]),
        git_output(&["config", "user.name"]),
        git_output(&["config", "user.email"]),
    );

    // `GITHUB_ACTOR` is who the run belongs to; `GITHUB_TRIGGERING_ACTOR`
    // differs only on a re-run, where it is whoever pressed re-run rather than
    // whoever caused the original event. The change is still the first one's.
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

/// The organisation and project named by the `forest.cue` in the working
/// directory.
///
/// Absent rather than fatal when there is no spec file, or it does not parse.
/// A caller that named both on the command line does not need one and should
/// not be made to have one — only a caller that left a blank finds out, and
/// then the error names the blank rather than the file.
pub(super) async fn project(state: &State) -> (Option<String>, Option<String>) {
    match state.project_parser().get_project().await {
        Ok(project) => (project.organisation.clone(), Some(project.name.clone())),
        Err(e) => {
            tracing::debug!("no project file to read organisation/project from: {e:#}");
            (None, None)
        }
    }
}

/// Resolve the author for an annotation — the shared entry point behind
/// `--detect` on both `forest release annotate` and `forest release create`.
///
/// Explicit flags always win: `--detect` answers only when the caller named
/// nobody, and never overrules somebody who took the trouble to say. With the
/// flag off this is the identity function, so the commands can call it
/// unconditionally.
pub(super) async fn resolve(
    state: &State,
    username: Option<String>,
    email: Option<String>,
    detect: bool,
) -> Attribution {
    // Detection contributes a whole identity or none of one. Topping up a
    // half-given identity is the same mixing `Author` exists to prevent, one
    // level up: `--source-username dentych --detect` on a machine whose git
    // config is somebody else's would have pinned Dennis's name to their
    // address. Anything the caller said about who this is wins outright.
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

/// What the annotation should say about who wrote the change: the fields the
/// server renders, plus the raw signals it links and records them from.
pub(super) struct Attribution {
    pub username: Option<String>,
    pub email: Option<String>,
    /// Empty unless `--detect` found something. Merged into the annotation's
    /// `--metadata`, where the server reads it back.
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

    /// A GitHub-shaped rung: handle, address, and the identifiers the server
    /// links on.
    fn gh(login: &str, email: Option<&str>, id: Option<&str>) -> Author {
        Author {
            username: Some(login.to_string()),
            email: email.map(str::to_string),
            github_login: Some(login.to_string()),
            github_user_id: id.map(str::to_string),
            display_name: None,
        }
    }

    /// The bug, pinned end to end.
    ///
    /// understory/infrastructure-hetzner's deploy workflow annotates with a
    /// personal access token belonging to kjuulh, so every release it made was
    /// attributed to kjuulh — including `8f4f8da`, which Dennis wrote. Given
    /// what that runner actually had in its environment, `--detect` has to
    /// reach Dennis and not the signed-in user.
    #[test]
    fn a_ci_push_is_attributed_to_the_commit_author_not_the_token_owner() {
        let chosen = choose(&assemble(
            // GITHUB_EVENT_PATH, as GitHub wrote it for that run.
            Some(author(Some("dentych"), Some("dennis@understory.io"))),
            // GITHUB_ACTOR and GITHUB_ACTOR_ID.
            Some("dentych".to_string()),
            Some("2256372".to_string()),
            // The checkout.
            author(Some("Dennis Tychsen"), Some("dennis@understory.io")),
            // Whoever the CI token belongs to — the wrong answer.
            author(Some("kjuulh"), Some("kasper@understory.io")),
            author(None, None),
        ));

        assert_eq!(chosen.author.username.as_deref(), Some("dentych"));
        assert_eq!(chosen.author.email.as_deref(), Some("dennis@understory.io"));
        assert_eq!(chosen.origin, Some("github-event"));
    }

    /// A release kjuulh really did write still says kjuulh — the fix must not
    /// simply move the mis-attribution onto somebody else.
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

    /// Off a runner there is no event and no actor, and `--detect` should land
    /// on the commit rather than on whoever happens to be signed in.
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

    /// With nothing else to go on — `--detect` passed by hand outside a
    /// checkout — the signed-in user is better than nobody.
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

    /// A push payload has the author's login but never their numeric id.
    /// `GITHUB_ACTOR_ID` supplies it when the actor *is* the author — pushing
    /// or merging your own work — which is what lets the server link on an
    /// identifier that survives a rename.
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

    /// And is withheld when they are not. Somebody else merging Dennis's pull
    /// request must not resolve the commit to the merger's forest account —
    /// the login still says Dennis, so the server links on that or on his
    /// email instead.
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

    /// Case is not significant in a GitHub login, and a case difference must
    /// not be read as "a different person" and cost us the id.
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

    /// What travels to the server: the raw signals, under a namespaced prefix,
    /// so it can link them and still say who wrote the change if it cannot.
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

    /// Nothing detected writes nothing — an annotation that did not sniff must
    /// not carry empty keys that read as "we looked and he is nobody".
    #[test]
    fn detecting_nobody_writes_no_metadata() {
        assert!(Detected::default().metadata().is_empty());
    }

    /// A pull_request payload carries the opener's id directly, so no
    /// correlation with the actor is needed.
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

    /// The case the flag exists for: a name and an address are only ever taken
    /// from the same rung. `github-actor` has no email, and borrowing the one
    /// below it would attach the author of `HEAD` to the person who pressed
    /// "Run workflow" — two different people on a `workflow_dispatch` run.
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

    /// An email with no name cannot answer "who": it would leave `source_user`
    /// empty and the server would fall back to the token owner anyway.
    #[test]
    fn an_email_alone_does_not_name_anybody() {
        let chosen = choose(&[candidate("git-commit", None, Some("dennis@understory.io"))]);

        assert_eq!(chosen, Detected::default());
    }

    /// The real shape, taken from the run that shipped
    /// `8f4f8da` to understory/infrastructure-hetzner.
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

        // The login, not "Dennis Tychsen" — this is the value `/avatars/<user>`
        // and `/users/<user>` resolve by.
        assert_eq!(author.username.as_deref(), Some("dentych"));
        assert_eq!(author.email.as_deref(), Some("dennis@understory.io"));
    }

    /// A commit whose author never linked a GitHub account has no `username`,
    /// and the display name beats attributing it to the token owner.
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

    /// GitHub writes an event file for every trigger, including ones with
    /// nobody on them — `schedule` has neither a commit nor a pull request.
    #[test]
    fn an_event_with_nobody_on_it_detects_nobody() {
        let event = serde_json::json!({ "schedule": "0 3 * * *" });

        assert_eq!(author_from_event(&event), None);
    }

    /// GitHub exports blanks as empty strings rather than omitting them, and an
    /// empty author is not an author.
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

    /// A path that is not there is the normal case off GitHub Actions, and a
    /// truncated payload is the abnormal one. Neither may abort an annotation —
    /// attribution is worth degrading, not failing, a release for.
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
