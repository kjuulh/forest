use std::path::PathBuf;

use crate::{
    component_cache::models::CacheComponent, services::temp_directories::GuardedTempDirectory,
};

#[derive(Clone, Debug)]
pub struct Choices {
    pub choices: Vec<Choice>,
}
impl Choices {
    pub(crate) fn to_string_vec(&self) -> Vec<String> {
        self.choices.iter().map(|i| i.name.clone()).collect()
    }

    pub(crate) fn get(&self, output: &str) -> Option<Choice> {
        self.choices.iter().find(|c| c.name == output).cloned()
    }
}

#[derive(Clone, Debug)]
pub struct Choice {
    pub name: String,

    pub init: String,
    pub component: CacheComponent,
}
pub struct Template {
    pub path: GuardedTempDirectory,
}
