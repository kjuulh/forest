use serde::{Deserialize, Serialize};

/// NATS JetStream stream for transactional emails (magic links, welcome, etc.).
pub const EMAIL_STREAM_NAME: &str = "FORAGE_EMAIL";
pub const EMAIL_STREAM_SUBJECTS: &str = "forage.email.>";
pub const EMAIL_CONSUMER_NAME: &str = "forage-email-sender";

/// Build a NATS subject for a given email type.
pub fn email_subject(email_type: &str) -> String {
    format!("forage.email.{email_type}")
}

/// Wire format for email jobs published to NATS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailEnvelope {
    pub to: String,
    pub subject: String,
    pub body_html: String,
    pub body_text: String,
    pub email_type: String,
}
