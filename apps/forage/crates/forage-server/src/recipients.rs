use std::collections::HashMap;
use std::sync::Arc;

use forage_core::integrations::baseline::explicit_slack_preferences;
use forage_core::integrations::direct::{RecipientDirectory, RecipientProfile};

use crate::forest_client::GrpcForestClient;

pub struct ForestRecipients {
    pub grpc: Arc<GrpcForestClient>,
    pub service_token: String,
}

#[async_trait::async_trait]
impl RecipientDirectory for ForestRecipients {
    async fn member(
        &self,
        organisation: &str,
        user_id: &str,
    ) -> Result<Option<RecipientProfile>, String> {
        if !self
            .grpc
            .is_org_member_with_token(&self.service_token, organisation, user_id)
            .await?
        {
            return Ok(None);
        }
        self.grpc
            .recipient_profile_with_token(&self.service_token, user_id)
            .await
    }

    async fn slack_preferences(&self, user_id: &str) -> Result<HashMap<String, bool>, String> {
        let prefs = self
            .grpc
            .user_notification_preferences_with_token(&self.service_token, user_id)
            .await?;
        Ok(explicit_slack_preferences(&prefs))
    }
}
