use std::ops::Deref;

pub struct Namespace(String);
impl Deref for Namespace {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl From<String> for Namespace {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<Namespace> for non_grpc_interface::Namespace {
    fn from(value: Namespace) -> Self {
        Self { namespace: value.0 }
    }
}
impl From<non_grpc_interface::Namespace> for Namespace {
    fn from(value: non_grpc_interface::Namespace) -> Self {
        Self(value.namespace)
    }
}

pub struct ProjectName(String);
impl Deref for ProjectName {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl From<String> for ProjectName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

pub struct Project {
    pub namespace: Namespace,
    pub name: ProjectName,
}

impl From<non_grpc_interface::Project> for Project {
    fn from(value: non_grpc_interface::Project) -> Self {
        Self {
            namespace: value.namespace.into(),
            name: value.project.into(),
        }
    }
}
impl From<Project> for non_grpc_interface::Project {
    fn from(value: Project) -> Self {
        Self {
            namespace: value.namespace.to_string(),
            project: value.name.to_string(),
        }
    }
}

pub struct Destination(String);

impl Deref for Destination {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<String> for Destination {
    fn from(value: String) -> Self {
        Self(value)
    }
}
