use crate::state::State;

pub mod models;
use models::*;

pub struct TemplatesServices {}

impl TemplatesServices {
    pub async fn list_templates(&self) -> anyhow::Result<Templates> {
        Ok(Templates {
            templates: Vec::default(),
        })
    }
}

pub trait TemplatesServiceState {
    fn templates_service(&self) -> TemplatesServices;
}

impl TemplatesServiceState for State {
    fn templates_service(&self) -> TemplatesServices {
        TemplatesServices {}
    }
}
