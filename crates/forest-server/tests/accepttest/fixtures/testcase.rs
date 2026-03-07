use std::sync::{Arc, RwLock};

use super::{Fixture, fixture};

#[derive(Clone)]
pub struct TestCase<TData> {
    pub fixture: Fixture,
    pub data: Arc<RwLock<TData>>,
}

pub async fn testcase<TData: Default>() -> anyhow::Result<(Given<TData>, When<TData>, Then<TData>)> {
    let fixture = fixture().await?;
    let data = Arc::new(RwLock::new(TData::default()));

    Ok((
        Given {
            testcase: TestCase {
                fixture: fixture.clone(),
                data: data.clone(),
            },
        },
        When {
            testcase: TestCase {
                fixture: fixture.clone(),
                data: data.clone(),
            },
        },
        Then {
            testcase: TestCase {
                fixture: fixture.clone(),
                data: data.clone(),
            },
        },
    ))
}

#[derive(Clone)]
pub struct Given<TData> {
    pub testcase: TestCase<TData>,
}

impl<TData> Given<TData> {
    pub fn fixture(&self) -> &Fixture {
        &self.testcase.fixture
    }
    pub fn data(&self) -> std::sync::RwLockReadGuard<'_, TData> {
        self.testcase.data.read().unwrap()
    }
    pub fn data_mut(&self) -> std::sync::RwLockWriteGuard<'_, TData> {
        self.testcase.data.write().unwrap()
    }
}

#[derive(Clone)]
pub struct When<TData> {
    pub testcase: TestCase<TData>,
}

impl<TData> When<TData> {
    pub fn fixture(&self) -> &Fixture {
        &self.testcase.fixture
    }
    pub fn data(&self) -> std::sync::RwLockReadGuard<'_, TData> {
        self.testcase.data.read().unwrap()
    }
    pub fn data_mut(&self) -> std::sync::RwLockWriteGuard<'_, TData> {
        self.testcase.data.write().unwrap()
    }
}

#[derive(Clone)]
pub struct Then<TData> {
    pub testcase: TestCase<TData>,
}

impl<TData> Then<TData> {
    pub fn fixture(&self) -> &Fixture {
        &self.testcase.fixture
    }
    pub fn data(&self) -> std::sync::RwLockReadGuard<'_, TData> {
        self.testcase.data.read().unwrap()
    }
}
