use serde::{Deserialize, Serialize};

pub const EMAIL_STREAM_NAME: &str = "FORAGE_EMAIL";
pub const EMAIL_STREAM_SUBJECTS: &str = "forage.email.>";
pub const EMAIL_CONSUMER_NAME: &str = "forage-email-sender";

pub fn email_subject(email_type: &str) -> String {
    format!("forage.email.{email_type}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailEnvelope {
    pub to: String,
    pub subject: String,
    pub body_html: String,
    pub body_text: String,
    pub email_type: String,
}
