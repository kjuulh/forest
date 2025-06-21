#[derive(Clone, Debug)]
pub struct RegistryComponent {
    pub namespace: String,
    pub name: String,
    pub version: String,
}

impl RegistryComponent {
    pub fn fqn(&self) -> String {
        format!("{}/{}@{}", self.namespace, self.name, self.version)
    }
}

#[derive(Clone, Debug, Default)]
pub struct RegistryComponents {
    components: Vec<RegistryComponent>,
}

impl RegistryComponents {
    pub(super) fn merge(&mut self, mut reg_components: RegistryComponents) -> &mut Self {
        self.components.append(&mut reg_components.components);

        self
    }

    pub(crate) fn items(&self) -> Vec<&RegistryComponent> {
        self.components.iter().collect()
    }
}

pub type RegistryName = String;
