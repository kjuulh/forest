use uuid::Uuid;

use crate::{
    State,
    repositories::organisations::{OrganisationRepository, OrganisationRepositoryState},
};

pub struct OrganisationService {
    repo: OrganisationRepository,
}

impl OrganisationService {
    fn db(&self) -> &sqlx::PgPool {
        self.repo.pool()
    }

    pub async fn create_organisation(
        &self,
        name: &str,
        creator_id: Uuid,
    ) -> anyhow::Result<CreatedOrganisation> {
        let id = Uuid::now_v7();

        let org = self
            .repo
            .create_organisation(self.db(), id, name)
            .await?;

        // Add the creator as an admin member automatically
        self.repo
            .add_member(self.db(), org.id, creator_id, "admin")
            .await?;

        Ok(CreatedOrganisation {
            organisation_id: org.id,
            name: org.name,
        })
    }

    pub async fn get_organisation_by_id(
        &self,
        id: Uuid,
    ) -> anyhow::Result<Option<OrganisationInfo>> {
        let row = self.repo.get_organisation(self.db(), id).await?;
        Ok(row.map(|r| OrganisationInfo {
            organisation_id: r.id,
            name: r.name,
            created_at: r.created_at,
        }))
    }

    pub async fn get_organisation_by_name(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<OrganisationInfo>> {
        let row = self.repo.get_organisation_by_name(self.db(), name).await?;
        Ok(row.map(|r| OrganisationInfo {
            organisation_id: r.id,
            name: r.name,
            created_at: r.created_at,
        }))
    }

    pub async fn search_organisations(
        &self,
        query: &str,
        page_size: i64,
        offset: i64,
    ) -> anyhow::Result<OrganisationSearchResult> {
        let rows = self
            .repo
            .search_organisations(self.db(), query, page_size, offset)
            .await?;
        let total_count = self
            .repo
            .count_organisations_search(self.db(), query)
            .await?;

        Ok(OrganisationSearchResult {
            organisations: rows
                .into_iter()
                .map(|r| OrganisationInfo {
                    organisation_id: r.id,
                    name: r.name,
                    created_at: r.created_at,
                })
                .collect(),
            total_count,
        })
    }
}

// -- Return types -------------------------------------------------------------

pub struct CreatedOrganisation {
    pub organisation_id: Uuid,
    pub name: String,
}

pub struct OrganisationInfo {
    pub organisation_id: Uuid,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct OrganisationSearchResult {
    pub organisations: Vec<OrganisationInfo>,
    pub total_count: i64,
}

// -- State trait --------------------------------------------------------------

pub trait OrganisationServiceState {
    fn organisation_service(&self) -> OrganisationService;
}

impl OrganisationServiceState for State {
    fn organisation_service(&self) -> OrganisationService {
        OrganisationService {
            repo: self.organisation_repository(),
        }
    }
}
