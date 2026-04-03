use anyhow::Context;

use crate::{
    services::project::ProjectParserState,
    state::State,
    user_state::UserStateLoaderState,
};

use super::{
    annotate::{self, git_output, AnnotateParams},
    commit::CommitCommand,
    prepare::PrepareCommand,
};

/// Combined command: prepare → annotate (without auto-release) → release.
///
/// Auto-detects organisation, project, git context, and source info from
/// the local environment. Only `--env` is required.
///
/// Usage: `forest release create --env prod`
#[derive(clap::Parser)]
pub struct CreateCommand {
    // ── Required ─────────────────────────────────────────────────────

    /// Target environment to release to.
    #[arg(long, short = 'e', alias = "env")]
    environment: String,

    // ── Optional overrides ───────────────────────────────────────────

    /// Release title. Defaults to the latest git commit subject.
    #[arg(long)]
    title: Option<String>,

    /// Release description. Defaults to the git commit body (if any).
    #[arg(long)]
    description: Option<String>,

    /// Organisation name. Auto-detected from forest.cue if not specified.
    #[arg(long, short = 'o')]
    organisation: Option<String>,

    /// Project name. Auto-detected from forest.cue if not specified.
    #[arg(long, short = 'p')]
    project: Option<String>,

    /// Git commit SHA. Auto-detected from HEAD if not specified.
    #[arg(long = "commit-sha")]
    commit_sha: Option<String>,

    /// Git branch. Auto-detected from the current branch if not specified.
    #[arg(long = "commit-branch")]
    commit_branch: Option<String>,

    /// Git commit message. Auto-detected from HEAD if not specified.
    #[arg(long = "commit-message")]
    commit_message: Option<String>,

    /// Release version tag (e.g. v1.2.3).
    #[arg(long)]
    version: Option<String>,

    /// Repository URL. Auto-detected from git remote if not specified.
    #[arg(long = "repo-url")]
    repo_url: Option<String>,

    /// Arbitrary key=value metadata pairs.
    #[arg(long)]
    metadata: Vec<String>,

    /// Source username (only used by app tokens; ignored for user tokens).
    #[arg(long = "source-username")]
    source_username: Option<String>,

    /// Source email (only used by app tokens; ignored for user tokens).
    #[arg(long = "source-email")]
    source_email: Option<String>,

    /// Source type (e.g. "ci", "local"). Defaults to "local".
    #[arg(long = "source-type")]
    source_type: Option<String>,

    /// CI run URL.
    #[arg(long = "run-url")]
    run_url: Option<String>,

    /// Web URL context for the release.
    #[arg(long = "context-web")]
    context_web: Option<String>,

    /// Pull request reference.
    #[arg(long = "context-pr")]
    context_pr: Option<String>,

    /// Path to the spec file (e.g. forest.cue). Auto-detected from cwd if not specified.
    #[arg(long = "spec-file")]
    spec_file: Option<String>,

    /// Skip uploading the spec file even if one is found.
    #[arg(long = "no-spec")]
    no_spec: bool,

    /// Additional files to include as attachments. Can be specified multiple times.
    #[arg(long = "include-file")]
    include_files: Vec<String>,

    /// Target destination(s). If omitted, the server decides.
    #[arg(long, short = 'd')]
    destination: Option<Vec<String>>,

    /// Override config values. Format: org/component.key=value
    /// Example: --set kjuulh/service.tag=abc123
    #[arg(long = "set", value_name = "KEY=VALUE")]
    overrides: Vec<String>,

    /// Skip waiting for the release to complete.
    #[arg(long)]
    no_wait: bool,

    /// Skip health monitoring after release.
    #[arg(long)]
    no_health: bool,

    /// Force release: cancel queued releases and jump to front of queue.
    #[arg(long)]
    force: bool,

    /// Use the project's release pipeline instead of deploying directly.
    #[arg(long)]
    pipeline: bool,
}

impl CreateCommand {
    pub async fn execute(&self, state: &State) -> anyhow::Result<()> {
        // ── Resolve project identity from forest.cue ─────────────────
        let (detected_org, detected_project) = detect_project(state).await?;

        let organisation = self
            .organisation
            .clone()
            .or(detected_org)
            .context("organisation not found: set project.organisation in forest.cue or pass --organisation")?;

        let project_name = self
            .project
            .clone()
            .or(detected_project)
            .context("project name not found: set project.name in forest.cue or pass --project")?;

        // ── Resolve git context ──────────────────────────────────────
        let git = GitInfo::detect().await;

        if git.dirty {
            tracing::warn!(
                "git working tree is dirty — the reported commit SHA will be suffixed with '-dirty'"
            );
        }

        let commit_sha = self
            .commit_sha
            .clone()
            .or(git.sha.clone())
            .context("commit sha not found: are you in a git repository? or pass --commit-sha")?;

        let commit_branch = self.commit_branch.clone().or(git.branch.clone());
        let commit_message = self.commit_message.clone().or(git.message.clone());
        let repo_url = self.repo_url.clone().or(git.remote_url.clone());

        // ── Build title from git context if not provided ─────────────
        let title = self.title.clone().unwrap_or_else(|| {
            git.subject
                .clone()
                .unwrap_or_else(|| format!("release to {}", self.environment))
        });
        let description = self.description.clone().or(git.body.clone());

        let source_type = self
            .source_type
            .clone()
            .or(Some("local".to_string()));

        // ── Resolve author info ──────────────────────────────────────
        // Prefer explicit flags, then local auth state, then git config.
        let (auth_username, auth_email) = detect_author(state).await;

        let source_username = self
            .source_username
            .clone()
            .or(auth_username)
            .or(git.author_name.clone());
        let source_email = self
            .source_email
            .clone()
            .or(auth_email)
            .or(git.author_email.clone());

        // ── 1. Prepare ───────────────────────────────────────────────
        tracing::info!("step 1/3: prepare");
        let prepare = PrepareCommand {
            overrides: self.overrides.clone(),
        };
        prepare.execute(state).await.context("prepare")?;

        // ── 2. Annotate (annotation_only — no auto-release) ─────────
        tracing::info!("step 2/3: annotate");

        // Include --set overrides in annotation metadata for traceability
        let mut metadata = self.metadata.clone();
        for (i, kv) in self.overrides.iter().enumerate() {
            metadata.push(format!("override.{i}={kv}"));
        }

        let slug = annotate::annotate(
            state,
            &AnnotateParams {
                metadata,
                source_username,
                source_email,
                context_title: title,
                context_description: description,
                context_web: self.context_web.clone(),
                organisation: organisation.clone(),
                project_name,
                commit_sha: Some(commit_sha),
                commit_branch,
                source_type,
                run_url: self.run_url.clone(),
                context_pr: self.context_pr.clone(),
                commit_message,
                version: self.version.clone(),
                repo_url,
                spec_file: self.spec_file.clone(),
                no_spec: self.no_spec,
                include_files: self.include_files.clone(),
                annotation_only: true,
            },
        )
        .await
        .context("annotate")?;

        println!("published artifact: {slug}");

        // ── 3. Release ───────────────────────────────────────────────
        tracing::info!("step 3/3: release");
        let commit = CommitCommand {
            slug: Some(slug),
            organisation: Some(organisation),
            environment: Some(self.environment.clone()),
            destination: self.destination.clone(),
            no_wait: self.no_wait,
            no_health: self.no_health,
            force: self.force,
            pipeline: self.pipeline,
            ..Default::default()
        };
        commit.execute(state).await.context("release")?;

        Ok(())
    }
}

/// Reads organisation and project name from the forest.cue/toml project file.
async fn detect_project(state: &State) -> anyhow::Result<(Option<String>, Option<String>)> {
    match state.project_parser().get_project().await {
        Ok(project) => Ok((
            project.organisation.clone(),
            Some(project.name.clone()),
        )),
        Err(e) => {
            tracing::debug!("could not parse project file for auto-detection: {e:#}");
            Ok((None, None))
        }
    }
}

/// Reads author identity from the local forest auth state.
/// Falls back gracefully — returns (None, None) if not logged in.
async fn detect_author(state: &State) -> (Option<String>, Option<String>) {
    match state.user_state().get_state().await {
        Ok(Some(user)) => {
            let email = user.emails.into_iter().next();
            (Some(user.username), email)
        }
        Ok(None) => {
            tracing::debug!("no local auth state found, skipping author auto-detection");
            (None, None)
        }
        Err(e) => {
            tracing::debug!("could not read auth state for author detection: {e:#}");
            (None, None)
        }
    }
}

/// Best-effort git information collected from the local repository.
struct GitInfo {
    sha: Option<String>,
    branch: Option<String>,
    /// Full commit message (subject + body).
    message: Option<String>,
    /// First line of the commit message.
    subject: Option<String>,
    /// Remaining lines after the subject (if any).
    body: Option<String>,
    remote_url: Option<String>,
    /// Whether the working tree has uncommitted changes.
    dirty: bool,
    /// Git user.name from config.
    author_name: Option<String>,
    /// Git user.email from config.
    author_email: Option<String>,
}

impl GitInfo {
    /// Collect git info from the working directory. Never fails — missing
    /// values are simply `None`.
    async fn detect() -> Self {
        let (sha, branch, message, remote_url, dirty, author_name, author_email) = tokio::join!(
            git_output(&["rev-parse", "HEAD"]),
            git_output(&["rev-parse", "--abbrev-ref", "HEAD"]),
            git_output(&["log", "-1", "--format=%B"]),
            git_output(&["remote", "get-url", "origin"]),
            git_is_dirty(),
            git_output(&["config", "user.name"]),
            git_output(&["config", "user.email"]),
        );

        let (subject, body) = match &message {
            Some(msg) => {
                let trimmed = msg.trim();
                match trimmed.split_once('\n') {
                    Some((subj, rest)) => {
                        let rest = rest.trim();
                        (
                            Some(subj.trim().to_string()),
                            if rest.is_empty() {
                                None
                            } else {
                                Some(rest.to_string())
                            },
                        )
                    }
                    None => (Some(trimmed.to_string()), None),
                }
            }
            None => (None, None),
        };

        // When dirty, append -dirty to the SHA so it's clear the commit
        // doesn't fully represent what was released.
        let sha = match (sha, dirty) {
            (Some(s), true) => Some(format!("{s}-dirty")),
            (sha, _) => sha,
        };

        Self {
            sha,
            branch,
            message: message.map(|m| m.trim().to_string()),
            subject,
            body,
            remote_url,
            dirty,
            author_name,
            author_email,
        }
    }
}

/// Returns true if the git working tree has uncommitted changes
/// (staged or unstaged, including untracked files).
async fn git_is_dirty() -> bool {
    // `git status --porcelain` outputs nothing when clean.
    git_output(&["status", "--porcelain"])
        .await
        .is_some_and(|s| !s.is_empty())
}
