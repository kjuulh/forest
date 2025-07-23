use crate::component_cache::models::LocalComponent;

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
    pub component: LocalComponent,
}
pub struct Template {}
