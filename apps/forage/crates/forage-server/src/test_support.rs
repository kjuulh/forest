use std::sync::{Arc, Mutex};

use axum::Router;
use chrono::Utc;
use forage_core::auth::{self, LoginResult, MfaSetup, *};
use forage_core::integrations::InMemoryIntegrationStore;
use forage_core::platform::{
    Artifact, ArtifactContext, ClientPrincipal, CreateOrgRuleSetInput, CreatePolicyInput,
    CreateReleasePipelineInput, CreateTriggerInput, CreatedOAuthApp, DelegatingUser,
    DelegatingUserOrganisation, Destination, DestinationTypeInfo, DirectoryLookup, DirectoryUser,
    Environment, ForestOAuthApps, ForestPlatform, NotificationPreference, OAuthApp,
    OAuthClientInfo, OAuthClientToken, OAuthDeviceAuthorization, OAuthDeviceToken, OAuthFlowError,
    OAuthGrant, OAuthIssuedTokens, OAuthUserinfo, OrgMember, OrgRuleSet, Organisation,
    PendingOAuthDeviceAuthorization, PlatformError, Policy, ReleasePipeline, Trigger,
    UpdateOrgRuleSetInput, UpdatePolicyInput, UpdateReleasePipelineInput, UpdateTriggerInput,
};
use forage_core::registry::{
    ComponentDetail, ComponentSearchResult, ComponentVersionInfo, ForestRegistry, ToolSummary,
};
use forage_core::session::{
    CachedOrg, CachedUser, InMemorySessionStore, SessionData, SessionStore,
};

use crate::state::AppState;
use crate::templates::TemplateEngine;

#[derive(Default)]
pub(crate) struct MockBehavior {
    pub register_result: Option<Result<RegisterResult, AuthError>>,
    pub login_result: Option<Result<LoginResult, AuthError>>,
    pub refresh_result: Option<Result<AuthTokens, AuthError>>,
    pub get_user_result: Option<Result<User, AuthError>>,
    pub get_user_by_username_result: Option<Result<UserProfile, AuthError>>,
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

#[derive(Default)]
pub(crate) struct MockPlatformBehavior {
    pub list_orgs_result: Option<Result<Vec<Organisation>, PlatformError>>,
    pub list_projects_result: Option<Result<Vec<String>, PlatformError>>,
    pub get_project_result: Option<Result<Option<forage_core::platform::Project>, PlatformError>>,
    pub list_artifacts_result: Option<Result<Vec<Artifact>, PlatformError>>,
    pub create_organisation_result: Option<Result<String, PlatformError>>,
    pub approval_error: Option<PlatformError>,
    pub list_members_result: Option<Result<Vec<OrgMember>, PlatformError>>,
    pub add_member_result: Option<Result<OrgMember, PlatformError>>,
    pub remove_member_result: Option<Result<(), PlatformError>>,
    pub update_member_role_result: Option<Result<OrgMember, PlatformError>>,
    pub get_artifact_by_slug_result: Option<Result<Artifact, PlatformError>>,
    pub list_environments_result: Option<Result<Vec<Environment>, PlatformError>>,
    pub update_environment_result: Option<Result<Environment, PlatformError>>,
    pub list_destinations_result: Option<Result<Vec<Destination>, PlatformError>>,
    pub list_org_rule_sets_result: Option<Result<Vec<OrgRuleSet>, PlatformError>>,
    pub create_org_rule_set_result: Option<Result<OrgRuleSet, PlatformError>>,
    pub update_org_rule_set_result: Option<Result<OrgRuleSet, PlatformError>>,
    pub delete_org_rule_set_result: Option<Result<(), PlatformError>>,
    pub rule_action_calls: Option<Arc<Mutex<Vec<String>>>>,
    pub list_triggers_result: Option<Result<Vec<Trigger>, PlatformError>>,
    pub list_policies_result: Option<Result<Vec<Policy>, PlatformError>>,
    pub create_trigger_result: Option<Result<Trigger, PlatformError>>,
    pub update_trigger_result: Option<Result<Trigger, PlatformError>>,
    pub delete_trigger_result: Option<Result<(), PlatformError>>,
    pub list_release_pipelines_result: Option<Result<Vec<ReleasePipeline>, PlatformError>>,
    pub create_release_pipeline_result: Option<Result<ReleasePipeline, PlatformError>>,
    pub update_release_pipeline_result: Option<Result<ReleasePipeline, PlatformError>>,
    pub delete_release_pipeline_result: Option<Result<(), PlatformError>>,
    pub get_artifact_spec_result: Option<Result<String, PlatformError>>,
    pub get_notification_preferences_result:
        Option<Result<Vec<NotificationPreference>, PlatformError>>,
    pub set_notification_preference_result: Option<Result<(), PlatformError>>,
    pub reset_notification_preference_result: Option<Result<(), PlatformError>>,
    pub preference_calls: Vec<(String, String, Option<bool>)>,
    pub list_destination_types_result: Option<Result<Vec<DestinationTypeInfo>, PlatformError>>,
    pub reveal_destination_metadata_result: Option<Result<String, PlatformError>>,
    pub list_allowed_domains_result:
        Option<Result<Vec<forage_core::platform::AllowedDomain>, PlatformError>>,
    pub add_allowed_domain_result:
        Option<Result<forage_core::platform::AllowedDomain, PlatformError>>,
    pub remove_allowed_domain_result: Option<Result<bool, PlatformError>>,
    pub verify_allowed_domain_result:
        Option<Result<forage_core::platform::VerifyDomainOutcome, PlatformError>>,
    pub list_join_offers_result:
        Option<Result<Vec<forage_core::platform::JoinOffer>, PlatformError>>,
    pub accept_join_offer_result: Option<Result<OrgMember, PlatformError>>,
    pub get_destination_states_result:
        Option<Result<forage_core::platform::DeploymentStates, PlatformError>>,
    pub get_release_intent_states_result:
        Option<Result<Vec<forage_core::platform::ReleaseIntentState>, PlatformError>>,
    pub list_artifacts_by_project: Option<std::collections::HashMap<String, Vec<Artifact>>>,
    pub get_plan_output_result: Option<Result<forage_core::platform::PlanOutput, PlatformError>>,
    pub get_visibility_result: Option<Result<forage_core::platform::Visibility, PlatformError>>,
    pub set_visibility_result: Option<Result<(), PlatformError>>,
    pub visibility_calls: Option<Arc<Mutex<Vec<String>>>>,
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

    async fn login(&self, identifier: &str, password: &str) -> Result<LoginResult, AuthError> {
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

    async fn setup_mfa(&self, _access_token: &str, _user_id: &str) -> Result<MfaSetup, AuthError> {
        let b = self.behavior.lock().unwrap();
        b.setup_mfa_result.clone().unwrap_or(Ok(MfaSetup {
            mfa_id: "mfa-mock-1".into(),
            provisioning_uri:
                "otpauth://totp/Forest:testuser?secret=JBSWY3DPEHPK3PXP&issuer=Forest".into(),
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

    async fn delete_token(&self, _access_token: &str, _token_id: &str) -> Result<(), AuthError> {
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
        b.confirm_email_verification_result
            .clone()
            .unwrap_or(Ok(()))
    }

    async fn get_user_by_username(
        &self,
        _access_token: &str,
        username: &str,
    ) -> Result<UserProfile, AuthError> {
        let b = self.behavior.lock().unwrap();
        b.get_user_by_username_result.clone().unwrap_or_else(|| {
            Ok(UserProfile {
                user_id: "user-123".into(),
                username: username.into(),
                profile_picture_url: None,
                created_at: Some("2025-01-15T10:00:00Z".into()),
            })
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
        b.list_linked_identities_result
            .clone()
            .unwrap_or(Ok(vec![]))
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

    async fn deny_device_login(&self, _user_code: &str, _user_id: &str) -> Result<(), AuthError> {
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

    fn check_approval(&self) -> Result<(), PlatformError> {
        let behavior = self
            .behavior
            .lock()
            .map_err(|_| PlatformError::Other("approval fixture lock poisoned".into()))?;
        match &behavior.approval_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    pub fn preference_calls(&self) -> Vec<(String, String, Option<bool>)> {
        self.behavior.lock().unwrap().preference_calls.clone()
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
        b.get_project_result.clone().unwrap_or_else(|| {
            Ok(Some(forage_core::platform::Project {
                organisation: organisation.into(),
                project: project.into(),
                ..Default::default()
            }))
        })
    }

    async fn list_artifacts(
        &self,
        _access_token: &str,
        _organisation: &str,
        project: &str,
    ) -> Result<Vec<Artifact>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        if let Some(by_project) = b.list_artifacts_by_project.as_ref() {
            return Ok(by_project.get(project).cloned().unwrap_or_default());
        }
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
        _dest: forage_core::platform::NewDestination<'_>,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn list_destination_types(
        &self,
        _access_token: &str,
    ) -> Result<Vec<DestinationTypeInfo>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_destination_types_result
            .clone()
            .unwrap_or(Ok(vec![]))
    }

    async fn update_destination(
        &self,
        _access_token: &str,
        _organisation: &str,
        _name: &str,
        _metadata: &std::collections::HashMap<String, String>,
        _sensitive_keys: Option<&[String]>,
    ) -> Result<(), PlatformError> {
        Ok(())
    }

    async fn reveal_destination_metadata(
        &self,
        _access_token: &str,
        _organisation: &str,
        _name: &str,
        key: &str,
    ) -> Result<String, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.reveal_destination_metadata_result
            .clone()
            .unwrap_or_else(|| Ok(format!("revealed-{key}")))
    }

    async fn get_destination_states(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: Option<&str>,
    ) -> Result<forage_core::platform::DeploymentStates, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.get_destination_states_result.clone().unwrap_or(Ok(
            forage_core::platform::DeploymentStates {
                destinations: vec![],
            },
        ))
    }

    async fn get_release_intent_states(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: Option<&str>,
        _include_completed: bool,
    ) -> Result<Vec<forage_core::platform::ReleaseIntentState>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.get_release_intent_states_result
            .clone()
            .unwrap_or(Ok(vec![]))
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

    async fn list_org_rule_sets(
        &self,
        _access_token: &str,
        _organisation: &str,
    ) -> Result<Vec<OrgRuleSet>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_org_rule_sets_result.clone().unwrap_or(Ok(vec![]))
    }

    async fn create_org_rule_set(
        &self,
        _access_token: &str,
        organisation: &str,
        input: &CreateOrgRuleSetInput,
    ) -> Result<OrgRuleSet, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.create_org_rule_set_result.clone().unwrap_or_else(|| {
            Ok(OrgRuleSet {
                organisation: organisation.into(),
                name: input.name.clone(),
                enabled: input.enabled,
                selector: input.selector.clone(),
                policies: input.policies.clone(),
                triggers: input.triggers.clone(),
                release_pipelines: input.release_pipelines.clone(),
                created_at: "2026-03-08T00:00:00Z".into(),
                updated_at: "2026-03-08T00:00:00Z".into(),
            })
        })
    }

    async fn update_org_rule_set(
        &self,
        _access_token: &str,
        organisation: &str,
        _name: &str,
        input: &UpdateOrgRuleSetInput,
    ) -> Result<OrgRuleSet, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.update_org_rule_set_result.clone().unwrap_or_else(|| {
            Ok(OrgRuleSet {
                organisation: organisation.into(),
                name: input.name.clone(),
                enabled: input.enabled,
                selector: input.selector.clone(),
                policies: input.policies.clone(),
                triggers: input.triggers.clone(),
                release_pipelines: input.release_pipelines.clone(),
                created_at: "2026-03-08T00:00:00Z".into(),
                updated_at: "2026-03-08T00:00:00Z".into(),
            })
        })
    }

    async fn delete_org_rule_set(
        &self,
        _access_token: &str,
        _organisation: &str,
        _name: &str,
    ) -> Result<(), PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.delete_org_rule_set_result.clone().unwrap_or(Ok(()))
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
        if let Some(calls) = &b.rule_action_calls {
            calls.lock().unwrap().push(format!("update_trigger:{name}"));
        }
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
        name: &str,
    ) -> Result<(), PlatformError> {
        let b = self.behavior.lock().unwrap();
        if let Some(calls) = &b.rule_action_calls {
            calls.lock().unwrap().push(format!("delete_trigger:{name}"));
        }
        b.delete_trigger_result.clone().unwrap_or(Ok(()))
    }

    async fn list_policies(
        &self,
        _access_token: &str,
        _organisation: &str,
        _project: &str,
    ) -> Result<Vec<Policy>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_policies_result.clone().unwrap_or(Ok(vec![]))
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
        notification_type: &str,
        channel: &str,
        enabled: bool,
    ) -> Result<(), PlatformError> {
        let mut b = self.behavior.lock().unwrap();
        b.preference_calls
            .push((notification_type.into(), channel.into(), Some(enabled)));
        b.set_notification_preference_result
            .clone()
            .unwrap_or(Ok(()))
    }

    async fn reset_notification_preference(
        &self,
        _access_token: &str,
        notification_type: &str,
        channel: &str,
    ) -> Result<(), PlatformError> {
        let mut b = self.behavior.lock().unwrap();
        b.preference_calls
            .push((notification_type.into(), channel.into(), None));
        b.reset_notification_preference_result
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
        self.check_approval()?;
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
        self.check_approval()?;
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
        self.check_approval()
    }

    async fn reject_plan_stage(
        &self,
        _access_token: &str,
        _release_intent_id: &str,
        _stage_id: &str,
        _reason: Option<&str>,
    ) -> Result<(), PlatformError> {
        self.check_approval()
    }

    async fn get_plan_output(
        &self,
        _access_token: &str,
        _release_intent_id: &str,
        _stage_id: &str,
    ) -> Result<forage_core::platform::PlanOutput, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.get_plan_output_result.clone().unwrap_or_else(|| {
            Ok(forage_core::platform::PlanOutput {
                plan_output: String::new(),
                status: "RUNNING".into(),
                outputs: vec![],
            })
        })
    }

    async fn get_visibility(
        &self,
        _access_token: &str,
        _organisation: &str,
        project: Option<&str>,
    ) -> Result<forage_core::platform::Visibility, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.get_visibility_result.clone().unwrap_or_else(|| {
            Ok(forage_core::platform::Visibility {
                organisation: "private".into(),
                project: project.map(|_| "private".into()),
                can_change: true,
            })
        })
    }

    async fn set_organisation_visibility(
        &self,
        _access_token: &str,
        organisation: &str,
        visibility: &str,
    ) -> Result<(), PlatformError> {
        let b = self.behavior.lock().unwrap();
        if let Some(calls) = &b.visibility_calls {
            calls
                .lock()
                .unwrap()
                .push(format!("org:{organisation}={visibility}"));
        }
        b.set_visibility_result.clone().unwrap_or(Ok(()))
    }

    async fn set_project_visibility(
        &self,
        _access_token: &str,
        organisation: &str,
        project: &str,
        visibility: &str,
    ) -> Result<(), PlatformError> {
        let b = self.behavior.lock().unwrap();
        if let Some(calls) = &b.visibility_calls {
            calls
                .lock()
                .unwrap()
                .push(format!("project:{organisation}/{project}={visibility}"));
        }
        b.set_visibility_result.clone().unwrap_or(Ok(()))
    }

    async fn list_allowed_domains(
        &self,
        _access_token: &str,
        _organisation_id: &str,
    ) -> Result<Vec<forage_core::platform::AllowedDomain>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_allowed_domains_result.clone().unwrap_or(Ok(vec![]))
    }

    async fn add_allowed_domain(
        &self,
        _access_token: &str,
        _organisation_id: &str,
        domain: &str,
    ) -> Result<forage_core::platform::AllowedDomain, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.add_allowed_domain_result.clone().unwrap_or_else(|| {
            Ok(forage_core::platform::AllowedDomain {
                domain: domain.to_lowercase(),
                policy: "auto_invite_any_verified".into(),
                dns_verified: false,
                dns_verification_token: "mock-token".into(),
                created_at: None,
            })
        })
    }

    async fn remove_allowed_domain(
        &self,
        _access_token: &str,
        _organisation_id: &str,
        _domain: &str,
    ) -> Result<bool, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.remove_allowed_domain_result.clone().unwrap_or(Ok(true))
    }

    async fn verify_allowed_domain(
        &self,
        _access_token: &str,
        _organisation_id: &str,
        _domain: &str,
    ) -> Result<forage_core::platform::VerifyDomainOutcome, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.verify_allowed_domain_result
            .clone()
            .unwrap_or(Ok(forage_core::platform::VerifyDomainOutcome::Verified))
    }

    async fn list_join_offers(
        &self,
        _access_token: &str,
    ) -> Result<Vec<forage_core::platform::JoinOffer>, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.list_join_offers_result.clone().unwrap_or(Ok(vec![]))
    }

    async fn accept_join_offer(
        &self,
        _access_token: &str,
        _organisation_id: &str,
    ) -> Result<OrgMember, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.accept_join_offer_result.clone().unwrap_or_else(|| {
            Ok(OrgMember {
                user_id: "user-123".into(),
                username: "joiner".into(),
                role: "member".into(),
                joined_at: None,
            })
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
) -> (
    AppState,
    Arc<InMemorySessionStore>,
    Arc<InMemoryIntegrationStore>,
) {
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

pub(crate) fn mock_user_delegated_principal(token: &str) -> Option<ClientPrincipal> {
    let scopes: Vec<String> = match token {
        "datactl_kjuulh" => vec!["datagateway:apply".into(), "datagateway:read".into()],
        "holds_machine_scopes" => vec!["tokens:introspect".into(), "directory:read".into()],
        _ => return None,
    };
    Some(ClientPrincipal {
        app_id: "app-datactl".into(),
        organisation_id: "org-1".into(),
        scopes,
        user: Some(DelegatingUser {
            user_id: "user-1".into(),
            username: "kjuulh".into(),
            organisations: vec![
                DelegatingUserOrganisation {
                    organisation_id: "org-1".into(),
                    name: "testorg".into(),
                },
                DelegatingUserOrganisation {
                    organisation_id: "org-2".into(),
                    name: "understory".into(),
                },
            ],
        }),
    })
}

const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

type MockCodes = std::collections::HashMap<String, (String, Vec<String>, String, Option<String>)>;

pub(crate) struct MockOAuthAppsClient {
    apps: Mutex<Vec<OAuthApp>>,
    counter: Mutex<u32>,
    forced_error: Option<PlatformError>,
    codes: Mutex<MockCodes>,
    consents: Mutex<std::collections::HashMap<String, Vec<String>>>,
    device_grants: Mutex<Vec<MockDeviceGrant>>,
}

#[derive(Clone)]
pub(crate) struct MockDeviceGrant {
    pub device_code: String,
    pub user_code: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub state: MockDeviceState,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MockDeviceState {
    Pending,
    Approved { user_id: String, ip: String },
    Denied,
    Consumed,
}

impl MockOAuthAppsClient {
    pub(crate) fn new() -> Self {
        Self {
            apps: Mutex::new(Vec::new()),
            counter: Mutex::new(0),
            forced_error: None,
            codes: Mutex::new(std::collections::HashMap::new()),
            consents: Mutex::new(std::collections::HashMap::new()),
            device_grants: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn with_error(err: PlatformError) -> Self {
        Self {
            apps: Mutex::new(Vec::new()),
            counter: Mutex::new(0),
            forced_error: Some(err),
            codes: Mutex::new(std::collections::HashMap::new()),
            consents: Mutex::new(std::collections::HashMap::new()),
            device_grants: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn device_grants(&self) -> Vec<MockDeviceGrant> {
        self.device_grants.lock().unwrap().clone()
    }

    pub(crate) fn allow_device_grant(&self, client_id: &str) {
        for app in self.apps.lock().unwrap().iter_mut() {
            if app.client_id == client_id {
                app.grant_types.push(DEVICE_GRANT.into());
            }
        }
    }

    fn authenticate_client(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<(), OAuthFlowError> {
        let apps = self.apps.lock().unwrap();
        match apps.iter().find(|a| a.client_id == client_id) {
            Some(app) if !app.public && client_secret.is_empty() => {
                Err(OAuthFlowError::InvalidClient)
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn seed_consent(&self, client_id: &str, scopes: Vec<String>) {
        self.consents
            .lock()
            .unwrap()
            .insert(client_id.into(), scopes);
    }

    pub(crate) fn seed_app(
        &self,
        client_id: &str,
        redirect_uris: Vec<String>,
        scopes: Vec<String>,
    ) {
        self.seed(client_id, redirect_uris, scopes, false);
    }

    pub(crate) fn seed_public_app(
        &self,
        client_id: &str,
        redirect_uris: Vec<String>,
        scopes: Vec<String>,
    ) {
        self.seed(client_id, redirect_uris, scopes, true);
    }

    fn seed(&self, client_id: &str, redirect_uris: Vec<String>, scopes: Vec<String>, public: bool) {
        let mut counter = self.counter.lock().unwrap();
        *counter += 1;
        let id = format!("app-{}", *counter);
        self.apps.lock().unwrap().push(OAuthApp {
            app_id: id,
            organisation_id: "org-1".into(),
            name: "Seeded App".into(),
            description: String::new(),
            homepage_url: String::new(),
            client_id: client_id.into(),
            redirect_uris,
            scopes,
            grant_types: vec!["authorization_code".into()],
            public,
            created_by: "user-1".into(),
            created_at: "0".into(),
            updated_at: "0".into(),
        });
    }
}

#[async_trait::async_trait]
impl ForestOAuthApps for MockOAuthAppsClient {
    async fn create_oauth_app(
        &self,
        _access_token: &str,
        organisation_id: &str,
        name: &str,
        description: &str,
        homepage_url: &str,
        redirect_uris: &[String],
        scopes: &[String],
        grant_types: &[String],
    ) -> Result<CreatedOAuthApp, PlatformError> {
        if let Some(err) = &self.forced_error {
            return Err(err.clone());
        }
        let mut counter = self.counter.lock().unwrap();
        *counter += 1;
        let id = format!("app-{}", *counter);
        let app = OAuthApp {
            app_id: id.clone(),
            organisation_id: organisation_id.into(),
            name: name.into(),
            description: description.into(),
            homepage_url: homepage_url.into(),
            client_id: format!("forest_oa_{id}"),
            redirect_uris: redirect_uris.to_vec(),
            scopes: scopes.to_vec(),
            grant_types: if grant_types.is_empty() {
                vec!["authorization_code".to_string()]
            } else {
                grant_types.to_vec()
            },
            public: false,
            created_by: "user-1".into(),
            created_at: "0".into(),
            updated_at: "0".into(),
        };
        self.apps.lock().unwrap().push(app.clone());
        Ok(CreatedOAuthApp {
            app,
            client_secret: format!("forest_oas_secret_{id}"),
        })
    }

    async fn list_oauth_apps(
        &self,
        _access_token: &str,
        organisation_id: &str,
    ) -> Result<Vec<OAuthApp>, PlatformError> {
        if let Some(err) = &self.forced_error {
            return Err(err.clone());
        }
        Ok(self
            .apps
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a.organisation_id == organisation_id)
            .cloned()
            .collect())
    }

    async fn get_oauth_app(
        &self,
        _access_token: &str,
        organisation_id: &str,
        app_id: &str,
    ) -> Result<OAuthApp, PlatformError> {
        if let Some(err) = &self.forced_error {
            return Err(err.clone());
        }
        self.apps
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.organisation_id == organisation_id && a.app_id == app_id)
            .cloned()
            .ok_or_else(|| PlatformError::NotFound("oauth app not found".into()))
    }

    async fn update_oauth_app(
        &self,
        _access_token: &str,
        organisation_id: &str,
        app_id: &str,
        name: &str,
        description: &str,
        homepage_url: &str,
        redirect_uris: &[String],
        scopes: &[String],
        grant_types: &[String],
    ) -> Result<OAuthApp, PlatformError> {
        if let Some(err) = &self.forced_error {
            return Err(err.clone());
        }
        let mut apps = self.apps.lock().unwrap();
        let app = apps
            .iter_mut()
            .find(|a| a.organisation_id == organisation_id && a.app_id == app_id)
            .ok_or_else(|| PlatformError::NotFound("oauth app not found".into()))?;
        app.name = name.into();
        app.description = description.into();
        app.homepage_url = homepage_url.into();
        app.redirect_uris = redirect_uris.to_vec();
        app.scopes = scopes.to_vec();
        app.grant_types = if grant_types.is_empty() {
            vec!["authorization_code".to_string()]
        } else {
            grant_types.to_vec()
        };
        Ok(app.clone())
    }

    async fn rotate_oauth_app_secret(
        &self,
        _access_token: &str,
        organisation_id: &str,
        app_id: &str,
    ) -> Result<CreatedOAuthApp, PlatformError> {
        if let Some(err) = &self.forced_error {
            return Err(err.clone());
        }
        let apps = self.apps.lock().unwrap();
        let app = apps
            .iter()
            .find(|a| a.organisation_id == organisation_id && a.app_id == app_id)
            .cloned()
            .ok_or_else(|| PlatformError::NotFound("oauth app not found".into()))?;
        Ok(CreatedOAuthApp {
            client_secret: format!("forest_oas_rotated_{}", app.app_id),
            app,
        })
    }

    async fn delete_oauth_app(
        &self,
        _access_token: &str,
        organisation_id: &str,
        app_id: &str,
    ) -> Result<(), PlatformError> {
        if let Some(err) = &self.forced_error {
            return Err(err.clone());
        }
        let mut apps = self.apps.lock().unwrap();
        let before = apps.len();
        apps.retain(|a| !(a.organisation_id == organisation_id && a.app_id == app_id));
        if apps.len() == before {
            return Err(PlatformError::NotFound("oauth app not found".into()));
        }
        Ok(())
    }

    async fn lookup_oauth_client(
        &self,
        client_id: &str,
    ) -> Result<Option<OAuthClientInfo>, PlatformError> {
        if let Some(err) = &self.forced_error {
            return Err(err.clone());
        }
        Ok(self
            .apps
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.client_id == client_id)
            .map(|a| OAuthClientInfo {
                app_id: a.app_id.clone(),
                organisation_id: a.organisation_id.clone(),
                name: a.name.clone(),
                description: a.description.clone(),
                homepage_url: a.homepage_url.clone(),
                redirect_uris: a.redirect_uris.clone(),
                scopes: a.scopes.clone(),
                public: a.public,
            }))
    }

    async fn create_oauth_authorization_code(
        &self,
        client_id: &str,
        user_id: &str,
        redirect_uri: &str,
        scopes: &[String],
        code_challenge: Option<&str>,
        code_challenge_method: Option<&str>,
        _nonce: Option<&str>,
    ) -> Result<String, OAuthFlowError> {
        let apps = self.apps.lock().unwrap();
        let app = apps
            .iter()
            .find(|a| a.client_id == client_id)
            .ok_or(OAuthFlowError::InvalidClient)?;
        if !app
            .redirect_uris
            .iter()
            .any(|u| crate::routes::oauth::redirect_uri_matches(u, redirect_uri))
        {
            return Err(OAuthFlowError::InvalidRequest("redirect_uri".into()));
        }
        let challenge = match (code_challenge, code_challenge_method) {
            (Some(c), Some("S256")) => Some(c.to_string()),
            _ if app.public => {
                return Err(OAuthFlowError::InvalidRequest(
                    "invalid_request: a public client must use PKCE with code_challenge_method S256"
                        .into(),
                ));
            }
            _ => None,
        };
        for s in scopes {
            if !app.scopes.contains(s) {
                return Err(OAuthFlowError::InvalidScope);
            }
        }
        drop(apps);
        let mut counter = self.counter.lock().unwrap();
        *counter += 1;
        let code = format!("code-{}", *counter);
        self.codes.lock().unwrap().insert(
            code.clone(),
            (
                user_id.into(),
                scopes.to_vec(),
                redirect_uri.into(),
                challenge,
            ),
        );
        let mut consents = self.consents.lock().unwrap();
        let entry = consents.entry(client_id.into()).or_default();
        for s in scopes {
            if !entry.contains(s) {
                entry.push(s.clone());
            }
        }
        Ok(code)
    }

    async fn issue_client_credentials_token(
        &self,
        _client_id: &str,
        _client_secret: &str,
        scopes: &[String],
    ) -> Result<OAuthClientToken, OAuthFlowError> {
        Ok(OAuthClientToken {
            access_token: "forest_cat_mocktoken".into(),
            token_type: "bearer".into(),
            expires_in_seconds: 3600,
            scopes: scopes.to_vec(),
        })
    }

    async fn introspect_client_token(
        &self,
        access_token: &str,
    ) -> Result<Option<ClientPrincipal>, OAuthFlowError> {
        if let Some(user_token) = access_token.strip_prefix("forest_oat_") {
            return Ok(mock_user_delegated_principal(user_token));
        }
        let scopes = match access_token {
            "" | "forest_cat_revoked" => return Ok(None),
            t if !t.starts_with("forest_cat_") => return Ok(None),
            "forest_cat_introspector" => vec!["tokens:introspect".into()],
            "forest_cat_gateway_ci" => vec!["datagateway:apply".into(), "datagateway:read".into()],
            _ => vec!["directory:read".into()],
        };
        Ok(Some(ClientPrincipal {
            app_id: "app-1".into(),
            organisation_id: "org-1".into(),
            scopes,
            user: None,
        }))
    }

    async fn resolve_directory_user(
        &self,
        lookup: DirectoryLookup,
    ) -> Result<Option<DirectoryUser>, PlatformError> {
        let known = matches!(&lookup,
            DirectoryLookup::Email(e) if e == "kasper@understory.io")
            || matches!(&lookup, DirectoryLookup::Provider { provider, provider_user_id }
                if provider == "github" && provider_user_id == "26280046");
        Ok(known.then(|| DirectoryUser {
            user_id: "user-1".into(),
            username: "kjuulh".into(),
            emails: vec!["kasper@understory.io".into()],
        }))
    }

    async fn exchange_oauth_code(
        &self,
        client_id: &str,
        client_secret: &str,
        code: &str,
        redirect_uri: &str,
        code_verifier: Option<&str>,
    ) -> Result<OAuthIssuedTokens, OAuthFlowError> {
        self.authenticate_client(client_id, client_secret)?;
        let entry = self.codes.lock().unwrap().remove(code);
        let (_user, scopes, stored_redirect, challenge) =
            entry.ok_or(OAuthFlowError::InvalidGrant)?;
        if stored_redirect != redirect_uri {
            return Err(OAuthFlowError::InvalidGrant);
        }
        if let Some(challenge) = challenge {
            use base64::Engine;
            use sha2::Digest;
            let computed = code_verifier.map(|v| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(sha2::Sha256::digest(v.as_bytes()))
            });
            if computed.as_deref() != Some(challenge.as_str()) {
                return Err(OAuthFlowError::InvalidGrant);
            }
        }
        let id_token = scopes
            .iter()
            .any(|s| s == "openid")
            .then(|| "mock.id.token".to_string());
        Ok(OAuthIssuedTokens {
            access_token: "forest_oat_mocktoken".into(),
            refresh_token: "forest_ort_mocktoken".into(),
            token_type: "bearer".into(),
            expires_in_seconds: 8 * 3600,
            scopes,
            id_token,
        })
    }

    async fn refresh_oauth_token(
        &self,
        client_id: &str,
        client_secret: &str,
        refresh_token: &str,
    ) -> Result<OAuthIssuedTokens, OAuthFlowError> {
        self.authenticate_client(client_id, client_secret)?;
        if refresh_token != "forest_ort_mocktoken" {
            return Err(OAuthFlowError::InvalidGrant);
        }
        Ok(OAuthIssuedTokens {
            access_token: "forest_oat_refreshed".into(),
            refresh_token: "forest_ort_refreshed".into(),
            token_type: "bearer".into(),
            expires_in_seconds: 8 * 3600,
            scopes: vec!["profile".into()],
            id_token: None,
        })
    }

    async fn revoke_oauth_grant(
        &self,
        _user_id: &str,
        _app_id: &str,
    ) -> Result<u32, PlatformError> {
        if let Some(err) = &self.forced_error {
            return Err(err.clone());
        }
        Ok(1)
    }

    async fn list_oauth_grants(&self, _user_id: &str) -> Result<Vec<OAuthGrant>, PlatformError> {
        if let Some(err) = &self.forced_error {
            return Err(err.clone());
        }
        Ok(self
            .apps
            .lock()
            .unwrap()
            .iter()
            .map(|a| OAuthGrant {
                app_id: a.app_id.clone(),
                name: a.name.clone(),
                scopes: a.scopes.clone(),
                authorized_at: "0".into(),
            })
            .collect())
    }

    async fn get_oauth_consent(
        &self,
        client_id: &str,
        _user_id: &str,
    ) -> Result<Vec<String>, PlatformError> {
        if let Some(err) = &self.forced_error {
            return Err(err.clone());
        }
        Ok(self
            .consents
            .lock()
            .unwrap()
            .get(client_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn initiate_oauth_device_authorization(
        &self,
        client_id: &str,
        client_secret: &str,
        scopes: &[String],
    ) -> Result<OAuthDeviceAuthorization, OAuthFlowError> {
        let app = self
            .apps
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.client_id == client_id)
            .cloned()
            .ok_or(OAuthFlowError::InvalidClient)?;
        self.authenticate_client(client_id, client_secret)?;
        if !app.grant_types.iter().any(|g| g == DEVICE_GRANT) {
            return Err(OAuthFlowError::InvalidRequest(
                "unsupported_grant_type".into(),
            ));
        }
        if scopes.is_empty() || scopes.iter().any(|s| !app.scopes.contains(s)) {
            return Err(OAuthFlowError::InvalidScope);
        }
        let mut grants = self.device_grants.lock().unwrap();
        let n = grants.len() + 1;
        let grant = MockDeviceGrant {
            device_code: format!("mock-device-code-{n}"),
            user_code: format!("BCDF-GH{n:02}"),
            client_id: client_id.into(),
            scopes: scopes.to_vec(),
            state: MockDeviceState::Pending,
        };
        let user_code = grant.user_code.clone();
        let device_code = grant.device_code.clone();
        grants.push(grant);
        Ok(OAuthDeviceAuthorization {
            device_code,
            verification_uri: "http://forage.test/device".into(),
            verification_uri_complete: format!(
                "http://forage.test/device?user_code={}",
                user_code.replace('-', "")
            ),
            user_code,
            expires_in_seconds: 900,
            interval_seconds: 5,
        })
    }

    async fn lookup_oauth_device_authorization(
        &self,
        user_code: &str,
    ) -> Result<Option<PendingOAuthDeviceAuthorization>, PlatformError> {
        let wanted = user_code.replace('-', "").to_ascii_uppercase();
        let grants = self.device_grants.lock().unwrap();
        let Some(grant) = grants.iter().find(|g| {
            g.user_code.replace('-', "") == wanted && g.state == MockDeviceState::Pending
        }) else {
            return Ok(None);
        };
        let apps = self.apps.lock().unwrap();
        let app = apps.iter().find(|a| a.client_id == grant.client_id);
        Ok(app.map(|a| PendingOAuthDeviceAuthorization {
            client_id: a.client_id.clone(),
            name: a.name.clone(),
            homepage_url: a.homepage_url.clone(),
            scopes: grant.scopes.clone(),
        }))
    }

    async fn approve_oauth_device_authorization(
        &self,
        user_code: &str,
        user_id: &str,
        approving_ip: &str,
        _approving_user_agent: &str,
    ) -> Result<(), PlatformError> {
        let wanted = user_code.replace('-', "").to_ascii_uppercase();
        let mut grants = self.device_grants.lock().unwrap();
        let grant = grants
            .iter_mut()
            .find(|g| g.user_code.replace('-', "") == wanted && g.state == MockDeviceState::Pending)
            .ok_or_else(|| PlatformError::Other("invalid_grant".into()))?;
        grant.state = MockDeviceState::Approved {
            user_id: user_id.into(),
            ip: approving_ip.into(),
        };
        Ok(())
    }

    async fn deny_oauth_device_authorization(
        &self,
        user_code: &str,
        _user_id: &str,
    ) -> Result<(), PlatformError> {
        let wanted = user_code.replace('-', "").to_ascii_uppercase();
        let mut grants = self.device_grants.lock().unwrap();
        let grant = grants
            .iter_mut()
            .find(|g| g.user_code.replace('-', "") == wanted && g.state == MockDeviceState::Pending)
            .ok_or_else(|| PlatformError::Other("invalid_grant".into()))?;
        grant.state = MockDeviceState::Denied;
        Ok(())
    }

    async fn exchange_oauth_device_code(
        &self,
        client_id: &str,
        client_secret: &str,
        device_code: &str,
    ) -> Result<OAuthDeviceToken, OAuthFlowError> {
        if !self
            .apps
            .lock()
            .unwrap()
            .iter()
            .any(|a| a.client_id == client_id)
        {
            return Err(OAuthFlowError::InvalidClient);
        }
        self.authenticate_client(client_id, client_secret)?;
        let mut grants = self.device_grants.lock().unwrap();
        let Some(grant) = grants
            .iter_mut()
            .find(|g| g.device_code == device_code && g.client_id == client_id)
        else {
            return Ok(OAuthDeviceToken::ExpiredToken);
        };
        Ok(match grant.state.clone() {
            MockDeviceState::Pending => OAuthDeviceToken::AuthorizationPending,
            MockDeviceState::Denied => OAuthDeviceToken::AccessDenied,
            MockDeviceState::Consumed => OAuthDeviceToken::ExpiredToken,
            MockDeviceState::Approved { .. } => {
                grant.state = MockDeviceState::Consumed;
                OAuthDeviceToken::Issued(OAuthIssuedTokens {
                    access_token: "forest_oat_mockdevice".into(),
                    refresh_token: "forest_ort_mockdevice".into(),
                    token_type: "bearer".into(),
                    expires_in_seconds: 28800,
                    scopes: grant.scopes.clone(),
                    id_token: None,
                })
            }
        })
    }

    async fn oauth_userinfo(&self, access_token: &str) -> Result<OAuthUserinfo, OAuthFlowError> {
        if access_token != "forest_oat_mocktoken" {
            return Err(OAuthFlowError::InvalidGrant);
        }
        Ok(OAuthUserinfo {
            sub: "user-1".into(),
            username: Some("testuser".into()),
            profile_picture_url: None,
            email: Some("test@example.com".into()),
            emails: vec!["test@example.com".into()],
            scopes: vec!["profile".into(), "email".into()],
        })
    }
}

pub(crate) fn test_state_with_oauth_apps(
    oauth: Arc<MockOAuthAppsClient>,
) -> (AppState, Arc<InMemorySessionStore>) {
    let sessions = Arc::new(InMemorySessionStore::new());
    let state = AppState::new(
        make_templates(),
        Arc::new(MockForestClient::new()),
        Arc::new(MockPlatformClient::new()),
        sessions.clone(),
    )
    .with_oauth_apps_client(oauth);
    (state, sessions)
}

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

#[derive(Default)]
pub(crate) struct MockRegistryBehavior {
    pub search_components_result: Option<Result<ComponentSearchResult, PlatformError>>,
    pub search_public_components_result: Option<Result<ComponentSearchResult, PlatformError>>,
    pub get_component_detail_result: Option<Result<ComponentDetail, PlatformError>>,
    pub get_public_component_detail_result: Option<Result<ComponentDetail, PlatformError>>,
    pub list_component_versions_result: Option<Result<Vec<ComponentVersionInfo>, PlatformError>>,
    pub get_component_manifest_result: Option<Result<String, PlatformError>>,
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
        b.search_components_result
            .clone()
            .unwrap_or(Ok(ComponentSearchResult {
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
        b.get_component_detail_result
            .clone()
            .unwrap_or(Err(PlatformError::NotFound("component not found".into())))
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
        b.list_component_versions_result
            .clone()
            .unwrap_or(Ok(vec![]))
    }

    async fn get_component_manifest(
        &self,
        _access_token: &str,
        _organisation: &str,
        _name: &str,
        _version: &str,
    ) -> Result<String, PlatformError> {
        let b = self.behavior.lock().unwrap();
        b.get_component_manifest_result
            .clone()
            .unwrap_or(Ok(String::new()))
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
