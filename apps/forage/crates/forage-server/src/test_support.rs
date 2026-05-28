use std::sync::{Arc, Mutex};

use axum::Router;
use chrono::Utc;
use forage_core::auth::{self, LoginResult, MfaSetup, *};
use forage_core::platform::{
    Artifact, ArtifactContext, CreatePolicyInput, CreateReleasePipelineInput, CreateTriggerInput,
    Destination, DestinationTypeInfo, Environment, ForestPlatform, NotificationPreference,
    Organisation, OrgMember, PlatformError, Policy, ReleasePipeline, Trigger, UpdatePolicyInput,
    UpdateReleasePipelineInput, UpdateTriggerInput,
};
use forage_core::registry::{
    ComponentDetail, ComponentSearchResult, ComponentVersionInfo, ForestRegistry, ToolSummary,
};
use forage_core::integrations::InMemoryIntegrationStore;
use forage_core::session::{
    CachedOrg, CachedUser, InMemorySessionStore, SessionData, SessionStore,
};

use crate::state::AppState;
use crate::templates::TemplateEngine;

/// Configurable mock behavior for testing different scenarios.
#[derive(Default)]
pub(crate) struct MockBehavior {
    pub register_result: Option<Result<RegisterResult, AuthError>>,
    pub login_result: Option<Result<LoginResult, AuthError>>,
    pub refresh_result: Option<Result<AuthTokens, AuthError>>,
    pub get_user_result: Option<Result<User, AuthError>>,
    pub list_tokens_result: Option<Result<Vec<PersonalAccessToken>, AuthError>>,
    pub create_token_result: Option<Result<CreatedToken, AuthError>>,
    pub delete_token_result: Option<Result<(), AuthError>>,
    pub update_username_result: Option<Result<User, AuthError>>,
    pub change_password_result: Option<Result<(), AuthError>>,
    pub add_email_result: Option<Result<AddEmailResult, AuthError>>,
    pub remove_email_result: Option<Result<(), AuthError>>,
    pub confirm_email_verification_result: Option<Result<(), AuthError>>,
    pub oauth_login_result: Option<Result<OAuthLoginResult, AuthError>>,
    pub verify_login_mfa_result: Option<Result<AuthTokens, AuthError>>,
    pub setup_mfa_result: Option<Result<MfaSetup, AuthError>>,
    pub verify_mfa_setup_result: Option<Result<(), AuthError>>,
    pub disable_mfa_result: Option<Result<(), AuthError>>,
    pub list_linked_identities_result:
        Option<Result<Vec<forage_core::auth::LinkedIdentity>, AuthError>>,
    pub link_oauth_provider_result: Option<Result<(), AuthError>>,
    pub unlink_oauth_provider_result: Option<Result<(), AuthError>>,
    pub approve_device_login_result: Option<Result<(), AuthError>>,
    pub deny_device_login_result: Option<Result<(), AuthError>>,
}

/// Configurable mock behavior for platform (orgs, projects, artifacts).
#[derive(Default)]
pub(crate) struct MockPlatformBehavior {
    pub list_orgs_result: Option<Result<Vec<Organisation>, PlatformError>>,
    pub list_projects_result: Option<Result<Vec<String>, PlatformError>>,
    pub get_project_result: Option<Result<Option<forage_core::platform::Project>, PlatformError>>,
    pub list_artifacts_result: Option<Result<Vec<Artifact>, PlatformError>>,
    pub create_organisation_result: Option<Result<String, PlatformError>>,
    pub list_members_result: Option<Result<Vec<OrgMember>, PlatformError>>,
    pub add_member_result: Option<Result<OrgMember, PlatformError>>,
    pub remove_member_result: Option<Result<(), PlatformError>>,
    pub update_member_role_result: Option<Result<OrgMember, PlatformError>>,
    pub get_artifact_by_slug_result: Option<Result<Artifact, PlatformError>>,
    pub list_environments_result: Option<Result<Vec<Environment>, PlatformError>>,
    pub update_environment_result: Option<Result<Environment, PlatformError>>,
    pub list_destinations_result: Option<Result<Vec<Destination>, PlatformError>>,
    pub list_triggers_result: Option<Result<Vec<Trigger>, PlatformError>>,
    pub create_trigger_result: Option<Result<Trigger, PlatformError>>,
    pub update_trigger_result: Option<Result<Trigger, PlatformError>>,
    pub delete_trigger_result: Option<Result<(), PlatformError>>,
    pub list_release_pipelines_result: Option<Result<Vec<ReleasePipeline>, PlatformError>>,
    pub create_release_pipeline_result: Option<Result<ReleasePipeline, PlatformError>>,
    pub update_release_pipeline_result: Option<Result<ReleasePipeline, PlatformError>>,
    pub delete_release_pipeline_result: Option<Result<(), PlatformError>>,
    pub get_artifact_spec_result: Option<Result<String, PlatformError>>,
    pub get_notification_preferences_result: Option<Result<Vec<NotificationPreference>, PlatformError>>,
    pub set_notification_preference_result: Option<Result<(), PlatformError>>,
    pub list_destination_types_result: Option<Result<Vec<DestinationTypeInfo>, PlatformError>>,
}

pub(crate) fn ok_tokens() -> AuthTokens {
    AuthTokens {
        access_token: "mock-access".into(),
        refresh_token: "mock-refresh".into(),
        expires_in_seconds: 3600,
    }
}

pub(crate) fn ok_user() -> User {
    User {
        user_id: "user-123".into(),
        username: "testuser".into(),
        profile_picture_url: None,
        mfa_enabled: false,
        emails: vec![UserEmail {
            email: "test@example.com".into(),
            verified: true,
        }],
    }
}

/// Mock forest client with per-test configurable behavior.
pub(crate) struct MockForestClient {
    behavior: Mutex<MockBehavior>,
}

impl MockForestClient {
    pub fn new() -> Self {
        Self {
            behavior: Mutex::new(MockBehavior::default()),
        }
    }

    pub fn with_behavior(behavior: MockBehavior) -> Self {
        Self {
            behavior: Mutex::new(behavior),
        }
    }
}

#[async_trait::async_trait]
impl ForestAuth for MockForestClient {
    async fn register(
        &self,
        _username: &str,
        _email: &str,
        _password: &str,
    ) -> Result<RegisterResult, AuthError> {
        let b = self.behavior.lock().unwrap();
        b.register_result
            .clone()
            .unwrap_or(Ok(RegisterResult::Success(ok_tokens())))
    }

    async fn login(
        &self,
        identifier: &str,
        password: &str,
    ) -> Result<LoginResult, AuthError> {
        let b = self.behavior.lock().unwrap();
        if let Some(result) = b.login_result.clone() {
            return result;
        }
        if identifier == "testuser" && password == "CorrectPass123" {
            Ok(LoginResult::Success(ok_tokens()))
        } else {
            Err(AuthError::InvalidCredentials)
        }
    }

    async fn verify_login_mfa(
        &self,
        _mfa_session_token: &str,
        _code: &str,
    ) -> Result<AuthTokens, AuthError> {
        let b = self.behavior.lock().unwrap();
        b.verify_login_mfa_result.clone().unwrap_or(Ok(ok_tokens()))
    }

    async fn setup_mfa(
        &self,
        _access_token: &str,
        _user_id: &str,
    ) -> Result<MfaSetup, AuthError> {
        let b = self.behavior.lock().unwrap();
        b.setup_mfa_result.clone().unwrap_or(Ok(MfaSetup {
            mfa_id: "mfa-mock-1".into(),
            provisioning_uri: "otpauth://totp/Forest:testuser?secret=JBSWY3DPEHPK3PXP&issuer=Forest".into(),
            secret: "JBSWY3DPEHPK3PXP".into(),
        }))
    }

    async fn verify_mfa_setup(
        &self,
        _access_token: &str,
        _mfa_id: &str,
        _code: &str,
    ) -> Result<(), AuthError> {
        let b = self.behavior.lock().unwrap();
        b.verify_mfa_setup_result.clone().unwrap_or(Ok(()))
    }

    async fn disable_mfa(
        &self,
        _access_token: &str,
        _user_id: &str,
        _code: &str,
    ) -> Result<(), AuthError> {
        let b = self.behavior.lock().unwrap();
        b.disable_mfa_result.clone().unwrap_or(Ok(()))
    }

    async fn refresh_token(&self, _refresh_token: &str) -> Result<AuthTokens, AuthError> {
        let b = self.behavior.lock().unwrap();
        b.refresh_result.clone().unwrap_or(Ok(AuthTokens {
            access_token: "refreshed-access".into(),
            refresh_token: "refreshed-refresh".into(),
            expires_in_seconds: 3600,
        }))
    }

    async fn logout(&self, _refresh_token: &str) -> Result<(), AuthError> {
        Ok(())
    }

    async fn get_user(&self, access_token: &str) -> Result<User, AuthError> {
        let b = self.behavior.lock().unwrap();
        if let Some(result) = b.get_user_result.clone() {
            return result;
        }
        if access_token == "mock-access" || access_token == "refreshed-access" {
            Ok(ok_user())
        } else {
            Err(AuthError::NotAuthenticated)
        }
    }

    async fn list_tokens(
        &self,
        _access_token: &str,
        _user_id: &str,
    ) -> Result<Vec<PersonalAccessToken>, AuthError> {
        let b = self.behavior.lock().unwrap();
        b.list_tokens_result.clone().unwrap_or(Ok(vec![]))
    }

    async fn create_token(
        &self,
        _access_token: &str,
        _user_id: &str,
        name: &str,
    ) -> Result<CreatedToken, AuthError> {
        let b = self.behavior.lock().unwrap();
        b.create_token_result.clone().unwrap_or(Ok(CreatedToken {
            token: PersonalAccessToken {
                token_id: "tok-1".into(),
                name: name.into(),
                scopes: vec![],
                created_at: None,
                last_used: None,
                expires_at: None,
            },
            raw_token: "forg_abcdef1234567890".into(),
        }))
    }

    async fn delete_token(
        &self,
        _access_token: &str,
        _token_id: &str,
    ) -> Result<(), AuthError> {
        let b = self.behavior.lock().unwrap();
        b.delete_token_result.clone().unwrap_or(Ok(()))
    }

    async fn update_username(
        &self,
        _access_token: &str,
        _user_id: &str,
        new_username: &str,
    ) -> Result<User, AuthError> {
        let b = self.behavior.lock().unwrap();
        b.update_username_result.clone().unwrap_or(Ok(User {
            user_id: "user-123".into(),
            username: new_username.into(),
            profile_picture_url: None,
            mfa_enabled: false,
            emails: vec![UserEmail {
                email: "test@example.com".into(),
                verified: true,
            }],
        }))
    }

    async fn change_password(
        &self,
        _access_token: &str,
        _user_id: &str,
        _current_password: &str,
        _new_password: &str,
    ) -> Result<(), AuthError> {
        let b = self.behavior.lock().unwrap();
        b.change_password_result.clone().unwrap_or(Ok(()))
    }

    async fn add_email(
        &self,
        _access_token: &str,
        _user_id: &str,
        email: &str,
    ) -> Result<AddEmailResult, AuthError> {
        let b = self.behavior.lock().unwrap();
        b.add_email_result.clone().unwrap_or(Ok(AddEmailResult {
            email: UserEmail {
                email: email.into(),
                verified: false,
            },
            email_verification_required: false,
        }))
    }

    async fn confirm_email_verification(&self, _email: &str) -> Result<(), AuthError> {
        let b = self.behavior.lock().unwrap();
        b.confirm_email_verification_result.clone().unwrap_or(Ok(()))
    }

    async fn get_user_by_username(
        &self,
        _access_token: &str,
        username: &str,
    ) -> Result<UserProfile, AuthError> {
        Ok(UserProfile {
            user_id: "user-123".into(),
            username: username.into(),
            profile_picture_url: None,
            created_at: Some("2025-01-15T10:00:00Z".into()),
        })
    }

    async fn get_user_by_email(
        &self,
        _access_token: &str,
        email: &str,
    ) -> Result<UserProfile, AuthError> {
        Ok(UserProfile {
            user_id: "user-123".into(),
            username: email.split('@').next().unwrap_or("user").into(),
            profile_picture_url: None,
            created_at: Some("2025-01-15T10:00:00Z".into()),
        })
    }

    async fn remove_email(
        &self,
        _access_token: &str,
        _user_id: &str,
        _email: &str,
    ) -> Result<(), AuthError> {
        let b = self.behavior.lock().unwrap();
        b.remove_email_result.clone().unwrap_or(Ok(()))
    }

    async fn update_profile_picture_url(
        &self,
        _access_token: &str,
        _user_id: &str,
        _profile_picture_url: Option<&str>,
    ) -> Result<User, AuthError> {
        Ok(ok_user())
    }

    async fn oauth_login(
        &self,
        _provider: &str,
        _provider_user_id: &str,
        _provider_email: &str,
        _provider_display_name: &str,
        _picture_url: Option<&str>,
    ) -> Result<OAuthLoginResult, AuthError> {
        let b = self.behavior.lock().unwrap();
        b.oauth_login_result.clone().unwrap_or(Ok(OAuthLoginResult {
            user: ok_user(),
            tokens: ok_tokens(),
            is_new_user: false,
        }))
    }

    async fn list_linked_identities(
        &self,
        _access_token: &str,
        _user_id: &str,
    ) -> Result<Vec<forage_core::auth::LinkedIdentity>, AuthError> {
        let b = self.behavior.lock().unwrap();
        b.list_linked_identities_result.clone().unwrap_or(Ok(vec![]))
    }

    async fn link_oauth_provider(
        &self,
        _access_token: &str,
        _user_id: &str,
        _input: &forage_core::auth::LinkOAuthInput,
    ) -> Result<(), AuthError> {
        let b = self.behavior.lock().unwrap();
        b.link_oauth_provider_result.clone().unwrap_or(Ok(()))
    }

    async fn unlink_oauth_provider(
        &self,
        _access_token: &str,
        _user_id: &str,
        _provider: forage_core::auth::LinkedProvider,
    ) -> Result<(), AuthError> {
        let b = self.behavior.lock().unwrap();
        b.unlink_oauth_provider_result.clone().unwrap_or(Ok(()))
    }

    async fn approve_device_login(
        &self,
        _user_code: &str,
        _user_id: &str,
        _approving_ip: &str,
        _approving_user_agent: &str,
    ) -> Result<(), AuthError> {
        let b = self.behavior.lock().unwrap();
        b.approve_device_login_result.clone().unwrap_or(Ok(()))
    }

    async fn deny_device_login(
        &self,
        _user_code: &str,
        _user_id: &str,
    ) -> Result<(), AuthError> {
        let b = self.behavior.lock().unwrap();
        b.deny_device_login_result.clone().unwrap_or(Ok(()))
    }
}

pub(crate) struct MockPlatformClient {
    behavior: Mutex<MockPlatformBehavior>,
}

impl MockPlatformClient {
    pub fn new() -> Self {
        Self {
            behavior: Mutex::new(MockPlatformBehavior::default()),
        }
    }

    pub fn with_behavior(behavior: MockPlatformBehavior) -> Self {
        Self {
            behavior: Mutex::new(behavior),
        }
    }
}

pub(crate) fn default_orgs() -> Vec<Organisation> {
    vec![Organisation {
        organisation_id: "org-1".into(),
        name: "testorg".into(),
        role: "admin".into(),
    }]
}

#[async_trait::async_trait]
impl ForestPlatform for MockPlatformClient {
    async fn list_my_organisations(
        &self,
        _access_token: &str,
    ) -> Result<Vec<Organisation>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_orgs_result.clone().unwrap_or(Ok(default_orgs()))
    }

    async fn list_projects(
        &self,
        _access_token: &str,
        _organisation: &str,
    ) -> Result<Vec<String>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_projects_result
            .clone()
            .unwrap_or(Ok(vec!["my-api".into()]))
    }

    async fn get_project(
        &self,
        _access_token: &str,
        organisation: &str,
        project: &str,
    ) -> Result<Option<forage_core::platform::Project>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.get_project_result
            .clone()
            .unwrap_or_else(|| Ok(Some(forage_core::platform::Project {
                organisation: organisation.into(),
                project: project.into(),
                ..Default::default()
            })))
    }

    async fn list_artifacts(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
    ) -> Result<Vec<Artifact>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_artifacts_result.clone().unwrap_or(Ok(vec![Artifact {
            artifact_id: "art-1".into(),
            slug: "my-api-abc123".into(),
            context: ArtifactContext {
                title: "Deploy v1.0".into(),
                description: Some("Initial release".into()),
                web: None,
                pr: None,
            },
            source: None,
            git_ref: None,
            destinations: vec![],
            created_at: "2026-03-07T12:00:00Z".into(),
        }]))
    }

    async fn create_organisation(
        &self,
        _access_token: &str,
        name: &str,
    ) -> Result<String, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.create_organisation_result
            .clone()
            .unwrap_or(Ok(format!("org-{name}")))
    }

    async fn list_members(
        &self,
        _access_token: &str,
        _organisation_id: &str,
    ) -> Result<Vec<OrgMember>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_members_result.clone().unwrap_or(Ok(vec![OrgMember {
            user_id: "user-123".into(),
            username: "testuser".into(),
            role: "owner".into(),
            joined_at: Some("2026-01-01T00:00:00Z".into()),
        }]))
    }

    async fn add_member(
        &self,
        _access_token: &str,
        _organisation_id: &str,
        user_id: &str,
        role: &str,
    ) -> Result<OrgMember, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.add_member_result.clone().unwrap_or(Ok(OrgMember {
            user_id: user_id.into(),
            username: "newuser".into(),
            role: role.into(),
            joined_at: Some("2026-03-07T00:00:00Z".into()),
        }))
    }

    async fn remove_member(
        &self,
        _access_token: &str,
        _organisation_id: &str,
        _user_id: &str,
    ) -> Result<(), PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.remove_member_result.clone().unwrap_or(Ok(()))
    }

    async fn update_member_role(
        &self,
        _access_token: &str,
        _organisation_id: &str,
        user_id: &str,
        role: &str,
    ) -> Result<OrgMember, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.update_member_role_result.clone().unwrap_or(Ok(OrgMember {
            user_id: user_id.into(),
            username: "testuser".into(),
            role: role.into(),
            joined_at: Some("2026-01-01T00:00:00Z".into()),
        }))
    }

    async fn get_artifact_by_slug(
        &self,
        _access_token: &str,
        slug: &str,
    ) -> Result<Artifact, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.get_artifact_by_slug_result
            .clone()
            .unwrap_or(Ok(Artifact {
                artifact_id: "art-1".into(),
                slug: slug.into(),
                context: ArtifactContext {
                    title: "Deploy v1.0".into(),
                    description: Some("Initial release".into()),
                    web: None,
                    pr: None,
                },
                source: None,
                git_ref: None,
                destinations: vec![],
                created_at: "2026-03-07T12:00:00Z".into(),
            }))
    }

    async fn list_environments(
        &self,
        _access_token: &str,
        _organisation: &str,
    ) -> Result<Vec<Environment>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_environments_result.clone().unwrap_or(Ok(vec![]))
    }

    async fn list_destinations(
        &self,
        _access_token: &str,
        _organisation: &str,
    ) -> Result<Vec<Destination>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_destinations_result.clone().unwrap_or(Ok(vec![]))
    }

    async fn create_environment(
        &self,
        _access_token: &str,
        organisation: &str,
        name: &str,
        description: Option<&str>,
        sort_order: i32,
    ) -> Result<Environment, PlatformError> {
        Ok(Environment {
            id: format!("env-{name}"),
            organisation: organisation.into(),
            name: name.into(),
            description: description.map(|s| s.to_string()),
            sort_order,
            created_at: "2026-03-08T00:00:00Z".into(),
        })
    }

    async fn update_environment(
        &self,
        _access_token: &str,
        id: &str,
        description: Option<&str>,
        sort_order: Option<i32>,
    ) -> Result<Environment, PlatformError> {
        let b = self.behavior.lock().unwrap();
        if let Some(result) = b.update_environment_result.clone() {
            return result;
        }
        Ok(Environment {
            id: id.into(),
            organisation: "testorg".into(),
            name: "env".into(),
            description: description.map(|s| s.to_string()),
            sort_order: sort_order.unwrap_or(0),
            created_at: "2026-03-08T00:00:00Z".into(),
        })
    }

    async fn create_destination(
        &self,
        _access_token: &str,
        _organisation: &str,
        _name: &str,
        _environment: &str,
        _metadata: &std::collections::HashMap<String, String>,
        _dest_type: Option<&forage_core::platform::DestinationType>,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn list_destination_types(
        &self,
        _access_token: &str,
    ) -> Result<Vec<DestinationTypeInfo>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_destination_types_result.clone().unwrap_or(Ok(vec![]))
    }

    async fn update_destination(
        &self,
        _access_token: &str,
        _organisation: &str,
        _name: &str,
        _metadata: &std::collections::HashMap<String, String>,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn get_destination_states(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: Option<&str>,
    ) -> Result<forage_core::platform::DeploymentStates, PlatformError> {
        Ok(forage_core::platform::DeploymentStates {
            destinations: vec![],
        })
    }

    async fn get_release_intent_states(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: Option<&str>,
        _include_completed: bool,
    ) -> Result<Vec<forage_core::platform::ReleaseIntentState>, PlatformError> {
        Ok(vec![])
    }

    async fn release_artifact(
        &self,
        _access_token: &str,
        _artifact_id: &str,
        _destinations: &[String],
        _environments: &[String],
        _use_pipeline: bool,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn list_triggers(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
    ) -> Result<Vec<Trigger>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_triggers_result.clone().unwrap_or(Ok(vec![]))
    }

    async fn create_trigger(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
        input: &CreateTriggerInput,
    ) -> Result<Trigger, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.create_trigger_result.clone().unwrap_or(Ok(Trigger {
            id: "trigger-1".into(),
            name: input.name.clone(),
            enabled: true,
            branch_pattern: input.branch_pattern.clone(),
            title_pattern: input.title_pattern.clone(),
            author_pattern: input.author_pattern.clone(),
            commit_message_pattern: input.commit_message_pattern.clone(),
            source_type_pattern: input.source_type_pattern.clone(),
            target_environments: input.target_environments.clone(),
            target_destinations: input.target_destinations.clone(),
            force_release: input.force_release,
            use_pipeline: input.use_pipeline,
            created_at: "2026-03-08T00:00:00Z".into(),
            updated_at: "2026-03-08T00:00:00Z".into(),
        }))
    }

    async fn update_trigger(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
        name: &str,
        input: &UpdateTriggerInput,
    ) -> Result<Trigger, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.update_trigger_result.clone().unwrap_or(Ok(Trigger {
            id: "trigger-1".into(),
            name: name.into(),
            enabled: input.enabled.unwrap_or(true),
            branch_pattern: input.branch_pattern.clone(),
            title_pattern: input.title_pattern.clone(),
            author_pattern: input.author_pattern.clone(),
            commit_message_pattern: input.commit_message_pattern.clone(),
            source_type_pattern: input.source_type_pattern.clone(),
            target_environments: input.target_environments.clone(),
            target_destinations: input.target_destinations.clone(),
            force_release: input.force_release.unwrap_or(false),
            use_pipeline: input.use_pipeline.unwrap_or(false),
            created_at: "2026-03-08T00:00:00Z".into(),
            updated_at: "2026-03-08T00:00:00Z".into(),
        }))
    }

    async fn delete_trigger(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
        _name: &str,
    ) -> Result<(), PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.delete_trigger_result.clone().unwrap_or(Ok(()))
    }

    async fn list_policies(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
    ) -> Result<Vec<Policy>, PlatformError> {
        Ok(vec![])
    }

    async fn create_policy(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
        _input: &CreatePolicyInput,
    ) -> Result<Policy, PlatformError> {
        Err(PlatformError::Other("not implemented in mock".into()))
    }

    async fn update_policy(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
        _name: &str,
        _input: &UpdatePolicyInput,
    ) -> Result<Policy, PlatformError> {
        Err(PlatformError::Other("not implemented in mock".into()))
    }

    async fn delete_policy(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
        _name: &str,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn list_release_pipelines(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
    ) -> Result<Vec<ReleasePipeline>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_release_pipelines_result
            .clone()
            .unwrap_or(Ok(vec![]))
    }

    async fn create_release_pipeline(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
        input: &CreateReleasePipelineInput,
    ) -> Result<ReleasePipeline, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.create_release_pipeline_result
            .clone()
            .unwrap_or(Ok(ReleasePipeline {
                id: "pipeline-1".into(),
                name: input.name.clone(),
                enabled: true,
                stages: input.stages.clone(),
                created_at: "2026-03-08T00:00:00Z".into(),
                updated_at: "2026-03-08T00:00:00Z".into(),
            }))
    }

    async fn update_release_pipeline(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
        name: &str,
        input: &UpdateReleasePipelineInput,
    ) -> Result<ReleasePipeline, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.update_release_pipeline_result
            .clone()
            .unwrap_or(Ok(ReleasePipeline {
                id: "pipeline-1".into(),
                name: name.into(),
                enabled: input.enabled.unwrap_or(true),
                stages: input.stages.clone().unwrap_or_default(),
                created_at: "2026-03-08T00:00:00Z".into(),
                updated_at: "2026-03-08T00:00:00Z".into(),
            }))
    }

    async fn delete_release_pipeline(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
        _name: &str,
    ) -> Result<(), PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.delete_release_pipeline_result.clone().unwrap_or(Ok(()))
    }

    async fn get_artifact_spec(
        &self,
        _access_token: &str,
        _artifact_id: &str,
    ) -> Result<String, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.get_artifact_spec_result
            .clone()
            .unwrap_or(Ok(String::new()))
    }

    async fn get_notification_preferences(
        &self,
        _access_token: &str,
    ) -> Result<Vec<NotificationPreference>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.get_notification_preferences_result
            .clone()
            .unwrap_or(Ok(Vec::new()))
    }

    async fn set_notification_preference(
        &self,
        _access_token: &str,
        _notification_type: &str,
        _channel: &str,
        _enabled: bool,
    ) -> Result<(), PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.set_notification_preference_result
            .clone()
            .unwrap_or(Ok(()))
    }

    async fn evaluate_policies(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
        _target_environment: &str,
        _release_intent_id: Option<&str>,
    ) -> Result<Vec<forage_core::platform::PolicyEvaluation>, PlatformError> {
        Ok(vec![])
    }

    async fn approve_release(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
        _release_intent_id: &str,
        _target_environment: &str,
        _comment: Option<&str>,
        _force_bypass: bool,
    ) -> Result<forage_core::platform::ApprovalState, PlatformError> {
        Ok(forage_core::platform::ApprovalState {
            required_approvals: 1,
            current_approvals: 1,
            decisions: vec![],
        })
    }

    async fn reject_release(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
        _release_intent_id: &str,
        _target_environment: &str,
        _comment: Option<&str>,
    ) -> Result<forage_core::platform::ApprovalState, PlatformError> {
        Ok(forage_core::platform::ApprovalState {
            required_approvals: 1,
            current_approvals: 0,
            decisions: vec![],
        })
    }

    async fn get_approval_state(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
        _release_intent_id: &str,
        _target_environment: &str,
    ) -> Result<forage_core::platform::ApprovalState, PlatformError> {
        Ok(forage_core::platform::ApprovalState {
            required_approvals: 1,
            current_approvals: 0,
            decisions: vec![],
        })
    }

    async fn approve_plan_stage(
        &self,
        _access_token: &str,
        _release_intent_id: &str,
        _stage_id: &str,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn reject_plan_stage(
        &self,
        _access_token: &str,
        _release_intent_id: &str,
        _stage_id: &str,
        _reason: Option<&str>,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn get_plan_output(
        &self,
        _access_token: &str,
        _release_intent_id: &str,
        _stage_id: &str,
    ) -> Result<forage_core::platform::PlanOutput, PlatformError> {
        Ok(forage_core::platform::PlanOutput {
            plan_output: String::new(),
            status: "RUNNING".into(),
            outputs: vec![],
        })
    }

}

pub(crate) fn make_templates() -> TemplateEngine {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    TemplateEngine::from_path(&workspace_root.join("templates"))
        .expect("templates must load for tests")
}

pub(crate) fn test_state() -> (AppState, Arc<InMemorySessionStore>) {
    test_state_with(MockForestClient::new(), MockPlatformClient::new())
}

pub(crate) fn test_state_with(
    mock: MockForestClient,
    platform: MockPlatformClient,
) -> (AppState, Arc<InMemorySessionStore>) {
    let sessions = Arc::new(InMemorySessionStore::new());
    let state = AppState::new(
        make_templates(),
        Arc::new(mock),
        Arc::new(platform),
        sessions.clone(),
    )
    .with_oauth_state_store(Arc::new(
        forage_core::auth::oauth_state::InMemoryOAuthStateStore::new(),
    ));
    (state, sessions)
}

pub(crate) fn test_state_with_integrations(
    mock: MockForestClient,
    platform: MockPlatformClient,
) -> (AppState, Arc<InMemorySessionStore>, Arc<InMemoryIntegrationStore>) {
    let sessions = Arc::new(InMemorySessionStore::new());
    let integrations = Arc::new(InMemoryIntegrationStore::new());
    let state = AppState::new(
        make_templates(),
        Arc::new(mock),
        Arc::new(platform),
        sessions.clone(),
    )
    .with_integration_store(integrations.clone());
    (state, sessions, integrations)
}

/// Mock OIDC exchange that returns a fixed identity.
pub(crate) struct MockOidcExchange {
    pub result: Mutex<Option<Result<auth::OidcIdentity, AuthError>>>,
}

impl MockOidcExchange {
    pub fn new() -> Self {
        Self {
            result: Mutex::new(None),
        }
    }

    pub fn with_result(result: Result<auth::OidcIdentity, AuthError>) -> Self {
        Self {
            result: Mutex::new(Some(result)),
        }
    }
}

#[async_trait::async_trait]
impl auth::OidcExchange for MockOidcExchange {
    async fn exchange_code(
        &self,
        _code: &str,
        _redirect_uri: &str,
    ) -> Result<auth::OidcIdentity, AuthError> {
        let r = self.result.lock().unwrap();
        r.clone().unwrap_or(Ok(auth::OidcIdentity {
            sub: "google-user-123".into(),
            email: "test@example.com".into(),
            name: "Test User".into(),
            picture_url: None,
            login: None,
        }))
    }
}

pub(crate) fn test_state_with_magic_link() -> (AppState, Arc<InMemorySessionStore>) {
    let (state, sessions) = test_state();
    let state = state.with_magic_link_store(Arc::new(
        forage_core::auth::magic_link::InMemoryMagicLinkStore::new(),
    ));
    (state, sessions)
}

pub(crate) fn test_state_with_google_oauth() -> (AppState, Arc<InMemorySessionStore>) {
    let (state, sessions) = test_state();
    let state = state
        .with_google_oauth_config(crate::state::GoogleOAuthConfig {
            client_id: "test-google-client-id".into(),
            client_secret: "test-google-client-secret".into(),
            redirect_host: "http://localhost:3000".into(),
        })
        .with_google_oidc_exchange(Arc::new(MockOidcExchange::new()));
    (state, sessions)
}

pub(crate) fn test_state_with_github_oauth() -> (AppState, Arc<InMemorySessionStore>) {
    let (state, sessions) = test_state();
    let state = state
        .with_github_oauth_config(crate::state::GitHubOAuthConfig {
            client_id: "test-github-client-id".into(),
            client_secret: "test-github-client-secret".into(),
            redirect_host: "http://localhost:3000".into(),
        })
        .with_github_oidc_exchange(Arc::new(MockOidcExchange::new()));
    (state, sessions)
}

/// Set up both GitHub and Google OAuth for testing the unified linking UI.
pub(crate) fn test_state_with_both_oauth() -> (AppState, Arc<InMemorySessionStore>) {
    let (state, sessions) = test_state();
    let state = state
        .with_google_oauth_config(crate::state::GoogleOAuthConfig {
            client_id: "test-google-client-id".into(),
            client_secret: "test-google-client-secret".into(),
            redirect_host: "http://localhost:3000".into(),
        })
        .with_google_oidc_exchange(Arc::new(MockOidcExchange::new()))
        .with_github_oauth_config(crate::state::GitHubOAuthConfig {
            client_id: "test-github-client-id".into(),
            client_secret: "test-github-client-secret".into(),
            redirect_host: "http://localhost:3000".into(),
        })
        .with_github_oidc_exchange(Arc::new(MockOidcExchange::new()));
    (state, sessions)
}

/// Configurable mock behavior for registry (component discovery).
#[derive(Default)]
pub(crate) struct MockRegistryBehavior {
    pub search_components_result: Option<Result<ComponentSearchResult, PlatformError>>,
    /// Result returned by `search_public_components`. When `None`, falls
    /// back to `search_components_result` so existing tests that set up
    /// public catalog fixtures don't have to change.
    pub search_public_components_result: Option<Result<ComponentSearchResult, PlatformError>>,
    pub get_component_detail_result: Option<Result<ComponentDetail, PlatformError>>,
    /// Result for `get_public_component_detail`. Falls back to
    /// `get_component_detail_result` when `None`.
    pub get_public_component_detail_result: Option<Result<ComponentDetail, PlatformError>>,
    pub list_component_versions_result: Option<Result<Vec<ComponentVersionInfo>, PlatformError>>,
    pub get_component_manifest_result: Option<Result<String, PlatformError>>,
    /// Result for `get_public_component_manifest`. Falls back to
    /// `get_component_manifest_result` when `None`.
    pub get_public_component_manifest_result: Option<Result<String, PlatformError>>,
    pub list_org_tools_result: Option<Result<Vec<ToolSummary>, PlatformError>>,
}

pub(crate) struct MockRegistryClient {
    behavior: Mutex<MockRegistryBehavior>,
}

impl MockRegistryClient {
    pub fn new() -> Self {
        Self {
            behavior: Mutex::new(MockRegistryBehavior::default()),
        }
    }

    pub fn with_behavior(behavior: MockRegistryBehavior) -> Self {
        Self {
            behavior: Mutex::new(behavior),
        }
    }
}

#[async_trait::async_trait]
impl ForestRegistry for MockRegistryClient {
    async fn search_components(
        &self,
        _access_token: &str,
        _query: &str,
        _organisation: Option<&str>,
        _page: i32,
        _page_size: i32,
    ) -> Result<ComponentSearchResult, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.search_components_result.clone().unwrap_or(Ok(ComponentSearchResult {
            components: vec![],
            total_count: 0,
        }))
    }

    async fn search_public_components(
        &self,
        _query: &str,
        _organisation: Option<&str>,
        _page: i32,
        _page_size: i32,
    ) -> Result<ComponentSearchResult, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.search_public_components_result
            .clone()
            .or_else(|| b.search_components_result.clone())
            .unwrap_or(Ok(ComponentSearchResult {
                components: vec![],
                total_count: 0,
            }))
    }

    async fn get_component_detail(
        &self,
        _access_token: &str,
        _organisation: &str,
        _name: &str,
    ) -> Result<ComponentDetail, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.get_component_detail_result.clone().unwrap_or(Err(
            PlatformError::NotFound("component not found".into()),
        ))
    }

    async fn get_public_component_detail(
        &self,
        _organisation: &str,
        _name: &str,
    ) -> Result<ComponentDetail, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.get_public_component_detail_result
            .clone()
            .or_else(|| b.get_component_detail_result.clone())
            .unwrap_or(Err(PlatformError::NotFound("component not found".into())))
    }

    async fn list_component_versions(
        &self,
        _access_token: &str,
        _organisation: &str,
        _name: &str,
    ) -> Result<Vec<ComponentVersionInfo>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_component_versions_result.clone().unwrap_or(Ok(vec![]))
    }

    async fn get_component_manifest(
        &self,
        _access_token: &str,
        _organisation: &str,
        _name: &str,
        _version: &str,
    ) -> Result<String, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.get_component_manifest_result.clone().unwrap_or(Ok(String::new()))
    }

    async fn get_public_component_manifest(
        &self,
        _organisation: &str,
        _name: &str,
        _version: &str,
    ) -> Result<String, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.get_public_component_manifest_result
            .clone()
            .or_else(|| b.get_component_manifest_result.clone())
            .unwrap_or(Ok(String::new()))
    }

    async fn list_org_tools(
        &self,
        _access_token: &str,
        _organisation: &str,
    ) -> Result<Vec<ToolSummary>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_org_tools_result.clone().unwrap_or(Ok(vec![]))
    }
}

pub(crate) fn test_state_with_registry(
    mock: MockForestClient,
    platform: MockPlatformClient,
    registry: MockRegistryClient,
) -> (AppState, Arc<InMemorySessionStore>) {
    let sessions = Arc::new(InMemorySessionStore::new());
    let state = AppState::new(
        make_templates(),
        Arc::new(mock),
        Arc::new(platform),
        sessions.clone(),
    )
    .with_registry_client(Arc::new(registry))
    .with_service_account_key("test-service-key".into());
    (state, sessions)
}

pub(crate) fn test_app() -> Router {
    let (state, _) = test_state();
    crate::build_router(state)
}

pub(crate) fn test_app_with(mock: MockForestClient) -> Router {
    let (state, _) = test_state_with(mock, MockPlatformClient::new());
    crate::build_router(state)
}

pub(crate) fn default_test_orgs() -> Vec<CachedOrg> {
    vec![CachedOrg {
        organisation_id: "org-1".into(),
        name: "testorg".into(),
        role: "owner".into(),
    }]
}

/// Create a test session and return the cookie header value.
pub(crate) async fn create_test_session(sessions: &Arc<InMemorySessionStore>) -> String {
    let now = Utc::now();
    let data = SessionData {
        access_token: "mock-access".into(),
        refresh_token: "mock-refresh".into(),
        csrf_token: "test-csrf".into(),
        needs_username: false,
        access_expires_at: now + chrono::Duration::hours(1),
        user: Some(CachedUser {
            user_id: "user-123".into(),
            username: "testuser".into(),
            profile_picture_url: None,
            emails: vec![UserEmail {
                email: "test@example.com".into(),
                verified: true,
            }],
            orgs: default_test_orgs(),
        }),
        created_at: now,
        last_seen_at: now,
    };
    let session_id = sessions.create(data).await.unwrap();
    format!("forage_session={}", session_id)
}

/// Create a test session with an expired access token but valid refresh token.
pub(crate) async fn create_expired_session(sessions: &Arc<InMemorySessionStore>) -> String {
    let now = Utc::now();
    let data = SessionData {
        access_token: "expired-access".into(),
        refresh_token: "mock-refresh".into(),
        csrf_token: "test-csrf".into(),
        needs_username: false,
        access_expires_at: now - chrono::Duration::seconds(10),
        user: Some(CachedUser {
            user_id: "user-123".into(),
            username: "testuser".into(),
            profile_picture_url: None,
            emails: vec![UserEmail {
                email: "test@example.com".into(),
                verified: true,
            }],
            orgs: default_test_orgs(),
        }),
        created_at: now,
        last_seen_at: now,
    };
    let session_id = sessions.create(data).await.unwrap();
    format!("forage_session={}", session_id)
}

/// Create a test session with "member" role (non-admin, for authorization tests).
/// Test session whose primary email is unverified — used by the
/// "Try sending again" flow tests on /settings/account.
pub(crate) async fn create_test_session_unverified_email(
    sessions: &Arc<InMemorySessionStore>,
) -> String {
    let now = Utc::now();
    let data = SessionData {
        access_token: "mock-access".into(),
        refresh_token: "mock-refresh".into(),
        csrf_token: "test-csrf".into(),
        needs_username: false,
        access_expires_at: now + chrono::Duration::hours(1),
        user: Some(CachedUser {
            user_id: "user-123".into(),
            username: "testuser".into(),
            profile_picture_url: None,
            emails: vec![UserEmail {
                email: "test@example.com".into(),
                verified: false,
            }],
            orgs: default_test_orgs(),
        }),
        created_at: now,
        last_seen_at: now,
    };
    let session_id = sessions.create(data).await.unwrap();
    format!("forage_session={}", session_id)
}

pub(crate) async fn create_test_session_member(sessions: &Arc<InMemorySessionStore>) -> String {
    let now = Utc::now();
    let data = SessionData {
        access_token: "mock-access".into(),
        refresh_token: "mock-refresh".into(),
        csrf_token: "test-csrf".into(),
        needs_username: false,
        access_expires_at: now + chrono::Duration::hours(1),
        user: Some(CachedUser {
            user_id: "user-123".into(),
            username: "testuser".into(),
            profile_picture_url: None,
            emails: vec![UserEmail {
                email: "test@example.com".into(),
                verified: true,
            }],
            orgs: vec![CachedOrg {
                organisation_id: "org-1".into(),
                name: "testorg".into(),
                role: "member".into(),
            }],
        }),
        created_at: now,
        last_seen_at: now,
    };
    let session_id = sessions.create(data).await.unwrap();
    format!("forage_session={}", session_id)
}

/// Create a test session for an OAuth user who needs to pick a username.
pub(crate) async fn create_test_session_needs_username(
    sessions: &Arc<InMemorySessionStore>,
) -> String {
    let now = Utc::now();
    let data = SessionData {
        access_token: "mock-access".into(),
        refresh_token: "mock-refresh".into(),
        csrf_token: "test-csrf".into(),
        needs_username: true,
        access_expires_at: now + chrono::Duration::hours(1),
        user: Some(CachedUser {
            user_id: "user-123".into(),
            username: "".into(),
            profile_picture_url: None,
            emails: vec![UserEmail {
                email: "test@example.com".into(),
                verified: true,
            }],
            orgs: vec![],
        }),
        created_at: now,
        last_seen_at: now,
    };
    let session_id = sessions.create(data).await.unwrap();
    format!("forage_session={}", session_id)
}

/// Create a test session that's a member of multiple orgs. The first
/// entry is treated as the session's "default" org by `default_test_orgs`
/// callers; later entries cover routes that take an org from the URL
/// path and let us assert the path org (not the session default) drives
/// rendering.
pub(crate) async fn create_test_session_with_orgs(
    sessions: &Arc<InMemorySessionStore>,
    org_names: &[&str],
) -> String {
    let now = Utc::now();
    let orgs = org_names
        .iter()
        .enumerate()
        .map(|(i, name)| CachedOrg {
            organisation_id: format!("org-{}", i + 1),
            name: (*name).into(),
            role: "admin".into(),
        })
        .collect();
    let data = SessionData {
        access_token: "mock-access".into(),
        refresh_token: "mock-refresh".into(),
        csrf_token: "test-csrf".into(),
        needs_username: false,
        access_expires_at: now + chrono::Duration::hours(1),
        user: Some(CachedUser {
            user_id: "user-123".into(),
            username: "testuser".into(),
            profile_picture_url: None,
            emails: vec![UserEmail {
                email: "test@example.com".into(),
                verified: true,
            }],
            orgs,
        }),
        created_at: now,
        last_seen_at: now,
    };
    let session_id = sessions.create(data).await.unwrap();
    format!("forage_session={}", session_id)
}

/// Create a test session with no cached orgs (for onboarding tests).
pub(crate) async fn create_test_session_no_orgs(sessions: &Arc<InMemorySessionStore>) -> String {
    let now = Utc::now();
    let data = SessionData {
        access_token: "mock-access".into(),
        refresh_token: "mock-refresh".into(),
        csrf_token: "test-csrf".into(),
        needs_username: false,
        access_expires_at: now + chrono::Duration::hours(1),
        user: Some(CachedUser {
            user_id: "user-123".into(),
            username: "testuser".into(),
            profile_picture_url: None,
            emails: vec![UserEmail {
                email: "test@example.com".into(),
                verified: true,
            }],
            orgs: vec![],
        }),
        created_at: now,
        last_seen_at: now,
    };
    let session_id = sessions.create(data).await.unwrap();
    format!("forage_session={}", session_id)
}
