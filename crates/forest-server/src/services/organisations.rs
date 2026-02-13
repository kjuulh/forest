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

    // -- Member management --------------------------------------------------------

    async fn require_admin(
        &self,
        organisation_id: Uuid,
        requester_id: Uuid,
    ) -> anyhow::Result<()> {
        let member = self
            .repo
            .get_member(self.db(), organisation_id, requester_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("you are not a member of this organisation"))?;

        if member.role != "admin" {
            anyhow::bail!("only admins can perform this action");
        }

        Ok(())
    }

    pub async fn add_member(
        &self,
        organisation_id: Uuid,
        user_id: Uuid,
        role: &str,
        requester_id: Uuid,
    ) -> anyhow::Result<MemberInfo> {
        validate_role(role)?;
        self.require_admin(organisation_id, requester_id).await?;

        self.repo
            .add_member(self.db(), organisation_id, user_id, role)
            .await?;

        let row = self
            .repo
            .get_member_with_username(self.db(), organisation_id, user_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("member not found after insert"))?;

        Ok(MemberInfo {
            user_id: row.user_id,
            username: row.username,
            role: row.role,
            joined_at: row.created_at,
        })
    }

    pub async fn remove_member(
        &self,
        organisation_id: Uuid,
        user_id: Uuid,
        requester_id: Uuid,
    ) -> anyhow::Result<()> {
        self.require_admin(organisation_id, requester_id).await?;

        self.repo
            .remove_member(self.db(), organisation_id, user_id)
            .await?;

        Ok(())
    }

    pub async fn update_member_role(
        &self,
        organisation_id: Uuid,
        user_id: Uuid,
        role: &str,
        requester_id: Uuid,
    ) -> anyhow::Result<MemberInfo> {
        validate_role(role)?;
        self.require_admin(organisation_id, requester_id).await?;

        self.repo
            .update_member_role(self.db(), organisation_id, user_id, role)
            .await?;

        let row = self
            .repo
            .get_member_with_username(self.db(), organisation_id, user_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("member not found after update"))?;

        Ok(MemberInfo {
            user_id: row.user_id,
            username: row.username,
            role: row.role,
            joined_at: row.created_at,
        })
    }

    pub async fn list_my_organisations(
        &self,
        user_id: Uuid,
        role_filter: Option<&str>,
    ) -> anyhow::Result<Vec<MyOrganisation>> {
        let rows = self
            .repo
            .list_organisations_by_user(self.db(), user_id, role_filter)
            .await?;

        Ok(rows
            .into_iter()
            .map(|r| MyOrganisation {
                organisation_id: r.id,
                name: r.name,
                role: r.role,
                created_at: r.created_at,
            })
            .collect())
    }

    pub async fn list_members(
        &self,
        organisation_id: Uuid,
        page_size: i64,
        offset: i64,
    ) -> anyhow::Result<MemberListResult> {
        let rows = self
            .repo
            .list_members(self.db(), organisation_id, page_size, offset)
            .await?;
        let total_count = self.repo.count_members(self.db(), organisation_id).await?;

        Ok(MemberListResult {
            members: rows
                .into_iter()
                .map(|r| MemberInfo {
                    user_id: r.user_id,
                    username: r.username,
                    role: r.role,
                    joined_at: r.created_at,
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

pub struct MyOrganisation {
    pub organisation_id: Uuid,
    pub name: String,
    pub role: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct MemberInfo {
    pub user_id: Uuid,
    pub username: String,
    pub role: String,
    pub joined_at: chrono::DateTime<chrono::Utc>,
}

pub struct MemberListResult {
    pub members: Vec<MemberInfo>,
    pub total_count: i64,
}

fn validate_role(role: &str) -> anyhow::Result<()> {
    match role {
        "admin" | "member" => Ok(()),
        _ => anyhow::bail!("invalid role: {role}, must be 'admin' or 'member'"),
    }
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
