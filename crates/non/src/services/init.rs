use crate::state::State;

pub mod models;

use models::*;

pub struct InitService {}

impl InitService {
    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn init(&self, choice: Option<String>) -> anyhow::Result<()> {
        let sources = self.fetch_sources().await?;

        let Some(choice) = self.get_choice(&sources, choice).await? else {
            tracing::warn!("user choice was not found in list of items");
            anyhow::bail!("failed to find source");
        };

        let template = self.download_choice(&choice).await?;

        self.move_template(&template).await?;

        Ok(())
    }

    pub async fn fetch_sources(&self) -> anyhow::Result<Choices> {
        tracing::debug!("fetching init sources");

        Ok(Choices {
            choices: vec![Choice {
                name: "rust:service".into(),
            }],
        })
    }

    #[tracing::instrument(skip(self), level = "trace")]
    pub async fn get_choice(
        &self,
        choices: &Choices,
        choice: Option<String>,
    ) -> anyhow::Result<Option<Choice>> {
        tracing::debug!("providing user choice of source");

        let user_choice = match choice {
            Some(user_choice) => user_choice,
            None => inquire::Select::new(
                "choose a template to bootstrap your project",
                choices.to_string_vec(),
            )
            .with_vim_mode(true)
            .prompt()?,
        };

        let Some(choice) = choices.get(&user_choice) else {
            tracing::warn!(user_choice, "failed to find choice");

            return Ok(None);
        };

        Ok(Some(choice))
    }

    pub async fn download_choice(&self, choice: &Choice) -> anyhow::Result<Template> {
        tracing::debug!("fetching template into temp");
        todo!()
    }

    pub async fn move_template(&self, template: &Template) -> anyhow::Result<()> {
        tracing::debug!("putting template in path");
        todo!()
    }
}

pub trait InitServiceState {
    fn init_service(&self) -> InitService;
}

impl InitServiceState for State {
    fn init_service(&self) -> InitService {
        InitService {}
    }
}
