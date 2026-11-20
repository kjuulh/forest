use forage_core::platform::PendingOAuthDeviceAuthorization;
use forage_ui::chrome::{Chrome, PublicNav, Tab};
use forage_ui::format::urlencode;

use crate::auth::Session;
use crate::routes::oauth::scope_description;
use crate::state::AppState;
use crate::views::chrome::{Page, signed_in};

pub fn visitor(
    state: &AppState,
    title: impl Into<String>,
    description: impl Into<String>,
) -> Chrome {
    Chrome {
        title: title.into(),
        description: description.into(),
        assets: state.templates.assets().clone(),
        build: state.templates.build_stamp().clone(),
        viewer: None,
        public: PublicNav {
            is_landing: false,
            is_auth_page: true,
        },
    }
}

fn session_chrome(
    state: &AppState,
    session: &Session,
    org: Option<&str>,
    title: String,
    description: &str,
) -> Chrome {
    signed_in(
        state,
        session,
        Page {
            title,
            description: description.into(),
            org,
            project: None,
            projects: Vec::new(),
            tab: Tab::None,
        },
    )
}

fn present(return_to: Option<&str>) -> Option<String> {
    return_to.filter(|r| !r.is_empty()).map(Into::into)
}

pub struct Providers {
    pub google: bool,
    pub github: bool,
    pub magic_link: bool,
    query: String,
}

impl Providers {
    fn new(state: &AppState, return_to: Option<&str>) -> Self {
        Self {
            google: state.google_oauth_config.is_some(),
            github: state.github_oauth_config.is_some(),
            magic_link: state.magic_link_store.is_some(),
            query: return_to_query(return_to),
        }
    }

    pub fn href(&self, path: &str) -> String {
        format!("{path}{}", self.query)
    }
}

fn return_to_query(return_to: Option<&str>) -> String {
    match present(return_to) {
        Some(rt) => format!("?return_to={}", urlencode(&rt)),
        None => String::new(),
    }
}

pub struct LoginPage {
    pub chrome: Chrome,
    pub identifier: String,
    pub error: Option<String>,
    pub return_to: Option<String>,
    pub providers: Providers,
}

impl LoginPage {
    pub fn new(
        state: &AppState,
        identifier: &str,
        error: Option<String>,
        return_to: Option<&str>,
    ) -> Self {
        Self {
            chrome: visitor(state, "Sign In - Forest", "Sign in to your Forest account"),
            identifier: identifier.into(),
            error,
            return_to: present(return_to),
            providers: Providers::new(state, return_to),
        }
    }
}

pub struct SignupPage {
    pub chrome: Chrome,
    pub username: String,
    pub email: String,
    pub error: Option<String>,
    pub return_to: Option<String>,
    pub providers: Providers,
}

impl SignupPage {
    pub fn new(
        state: &AppState,
        username: &str,
        email: &str,
        error: Option<String>,
        return_to: Option<&str>,
    ) -> Self {
        Self {
            chrome: visitor(state, "Sign Up - Forest", "Create your Forest account"),
            username: username.into(),
            email: email.into(),
            error,
            return_to: present(return_to),
            providers: Providers::new(state, return_to),
        }
    }
}

pub struct CompleteProfilePage {
    pub chrome: Chrome,
    pub csrf_token: String,
    pub username: String,
    pub error: Option<String>,
    pub return_to: Option<String>,
}

impl CompleteProfilePage {
    pub fn new(
        state: &AppState,
        csrf_token: &str,
        username: &str,
        error: Option<String>,
        return_to: Option<&str>,
    ) -> Self {
        Self {
            chrome: visitor(
                state,
                "Choose Username - Forest",
                "Pick a username for your new account",
            ),
            csrf_token: csrf_token.into(),
            username: username.into(),
            error,
            return_to: present(return_to),
        }
    }
}

pub struct MfaChallengePage {
    pub chrome: Chrome,
    pub error: Option<String>,
    pub return_to: Option<String>,
}

impl MfaChallengePage {
    pub fn new(state: &AppState, error: Option<String>, return_to: Option<&str>) -> Self {
        Self {
            chrome: visitor(
                state,
                "Two-factor authentication - Forest",
                "Enter your authenticator code to continue",
            ),
            error,
            return_to: present(return_to),
        }
    }
}

pub struct MagicLinkPage {
    pub chrome: Chrome,
    pub email: String,
    pub error: Option<String>,
    pub return_to: Option<String>,
}

impl MagicLinkPage {
    pub fn new(state: &AppState, email: &str, error: Option<String>, return_to: Option<&str>) -> Self {
        Self {
            chrome: visitor(
                state,
                "Sign in with Email - Forest",
                "Get a magic link sent to your email",
            ),
            email: email.into(),
            error,
            return_to: present(return_to),
        }
    }
}

pub struct EmailNoticePage {
    pub chrome: Chrome,
    pub email: String,
}

impl EmailNoticePage {
    pub fn magic_link_sent(state: &AppState, email: &str) -> Self {
        Self {
            chrome: visitor(state, "Check Your Email - Forest", "Magic link sent"),
            email: email.into(),
        }
    }

    pub fn check_inbox(state: &AppState, email: &str) -> Self {
        Self {
            chrome: visitor(
                state,
                "Verify your email - Forest",
                "Check your inbox for a verification link",
            ),
            email: email.into(),
        }
    }

    pub fn verified(state: &AppState, email: &str) -> Self {
        Self {
            chrome: visitor(
                state,
                "Email verified - Forest",
                "Your email has been verified",
            ),
            email: email.into(),
        }
    }
}

pub struct VerifyEmailFailedPage {
    pub chrome: Chrome,
}

impl VerifyEmailFailedPage {
    pub fn new(state: &AppState) -> Self {
        Self {
            chrome: visitor(
                state,
                "Verification link expired - Forest",
                "Verification link expired or already used",
            ),
        }
    }
}

pub struct VerifyEmailResendPage {
    pub chrome: Chrome,
    pub email: String,
    pub error: Option<String>,
}

impl VerifyEmailResendPage {
    pub fn new(state: &AppState) -> Self {
        Self {
            chrome: visitor(
                state,
                "Resend verification email - Forest",
                "Send a new verification link",
            ),
            email: String::new(),
            error: None,
        }
    }
}

pub struct ScopeLine {
    pub name: String,
    pub description: &'static str,
}

fn scope_lines<'a>(scopes: impl IntoIterator<Item = &'a String>) -> Vec<ScopeLine> {
    scopes
        .into_iter()
        .map(|s| ScopeLine {
            name: s.clone(),
            description: scope_description(s),
        })
        .collect()
}

pub struct DeviceApp {
    pub name: String,
    pub client_id: String,
    pub scopes: Vec<ScopeLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceOutcome {
    Pending,
    Unconfigured,
    Approved,
    Denied,
}

impl DeviceOutcome {
    pub fn from_result(result: Option<&str>) -> Self {
        match result {
            Some("unconfigured") => Self::Unconfigured,
            Some("approved") => Self::Approved,
            Some("denied") => Self::Denied,
            _ => Self::Pending,
        }
    }
}

pub struct DevicePage {
    pub chrome: Chrome,
    pub app: Option<DeviceApp>,
    pub username: String,
    pub csrf_token: String,
    pub user_code: String,
    pub error: Option<String>,
    pub outcome: DeviceOutcome,
}

impl DevicePage {
    pub fn new(
        state: &AppState,
        session: &Session,
        user_code: &str,
        app: Option<&PendingOAuthDeviceAuthorization>,
        error: Option<&str>,
        outcome: DeviceOutcome,
    ) -> Self {
        let title = match app {
            Some(app) => format!("Authorise {}", app.name),
            None => "Authorise Forest CLI".to_string(),
        };
        let first_org = session.user.orgs.first().map(|o| o.name.as_str());
        Self {
            chrome: session_chrome(state, session, first_org, title, "Approve a device login"),
            app: app.map(|a| DeviceApp {
                name: a.name.clone(),
                client_id: a.client_id.clone(),
                scopes: scope_lines(&a.scopes),
            }),
            username: session.user.username.clone(),
            csrf_token: session.csrf_token.clone(),
            user_code: user_code.into(),
            error: error.map(Into::into),
            outcome,
        }
    }
}

pub struct OAuthConsentPage {
    pub chrome: Chrome,
    pub username: String,
    pub csrf_token: String,
    pub app_name: String,
    pub app_homepage: String,
    pub scopes: Vec<ScopeLine>,
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: String,
    pub consent_binding: String,
    pub state: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub nonce: String,
}

pub struct ConsentRequest<'a> {
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub scopes: &'a [String],
    pub consent_binding: String,
    pub state: Option<&'a str>,
    pub code_challenge: Option<&'a str>,
    pub code_challenge_method: Option<&'a str>,
    pub nonce: Option<&'a str>,
}

impl OAuthConsentPage {
    pub fn new(
        state: &AppState,
        session: &Session,
        app_name: &str,
        app_homepage: &str,
        request: ConsentRequest<'_>,
    ) -> Self {
        Self {
            chrome: session_chrome(
                state,
                session,
                None,
                format!("Authorize {app_name} - Forest"),
                "Authorize application",
            ),
            username: session.user.username.clone(),
            csrf_token: session.csrf_token.clone(),
            app_name: app_name.into(),
            app_homepage: app_homepage.into(),
            scopes: scope_lines(request.scopes),
            client_id: request.client_id.into(),
            redirect_uri: request.redirect_uri.into(),
            scope: request.scopes.join(" "),
            consent_binding: request.consent_binding,
            state: request.state.unwrap_or_default().into(),
            code_challenge: request.code_challenge.unwrap_or_default().into(),
            code_challenge_method: request.code_challenge_method.unwrap_or_default().into(),
            nonce: request.nonce.unwrap_or_default().into(),
        }
    }
}
