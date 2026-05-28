use base64::Engine;
use uuid::Uuid;

use crate::{
    State,
    repositories::organisations::{OrganisationRepository, OrganisationRepositoryState},
    services::domain_policy::{
        self, AllowedDomainError, AllowedDomainPolicy, PolicyParseError,
    },
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

    /// Typed variant of [`require_admin`] used by the allowed-domain paths
    /// so the gRPC layer can map "not a member" / "not an admin" to
    /// PermissionDenied instead of the generic Internal that
    /// `anyhow::Error` collapses to. The standalone gRPC `require_org_access_by_id`
    /// fires first in normal flow; this is the service-side belt that
    /// keeps the right status if the gRPC gate is ever bypassed.
    async fn require_admin_typed(
        &self,
        organisation_id: Uuid,
        requester_id: Uuid,
    ) -> Result<(), MembershipError> {
        let member = self
            .repo
            .get_member(self.db(), organisation_id, requester_id)
            .await
            .map_err(|e| MembershipError::Db(anyhow::Error::from(e)))?
            .ok_or(MembershipError::NotMember)?;
        if member.role != "admin" {
            return Err(MembershipError::NotAdmin);
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

    // -- Allowed-domain auto-invite (DATA-252) --------------------------------

    pub async fn add_allowed_domain(
        &self,
        organisation_id: Uuid,
        raw_domain: &str,
        policy: AllowedDomainPolicy,
        requester_id: Uuid,
    ) -> Result<AllowedDomainInfo, AllowedDomainServiceError> {
        // v1: schema supports it, runtime doesn't. Reject explicitly so an
        // admin can't enable silent JIT on an unverified domain claim.
        if policy == AllowedDomainPolicy::AutoJoinOauth {
            return Err(AllowedDomainServiceError::PolicyNotYetSupported);
        }

        self.require_admin_typed(organisation_id, requester_id).await?;

        let canonical = domain_policy::normalize_domain(raw_domain)?;
        let token = generate_dns_verification_token();

        let row = self
            .repo
            .add_allowed_domain(
                self.db(),
                organisation_id,
                &canonical,
                policy.as_str(),
                &token,
                requester_id,
            )
            .await
            .map_err(|e| match e {
                crate::repositories::error::DbError::AlreadyExists(_) => {
                    AllowedDomainServiceError::AlreadyExists
                }
                other => AllowedDomainServiceError::other(other),
            })?;

        tracing::info!(
            organisation_id = %organisation_id,
            domain = %canonical,
            policy = policy.as_str(),
            created_by = %requester_id,
            "org_allowed_domain.added"
        );

        Ok(row_to_allowed_domain_info(row))
    }

    pub async fn list_allowed_domains(
        &self,
        organisation_id: Uuid,
    ) -> anyhow::Result<Vec<AllowedDomainInfo>> {
        let rows = self.repo.list_allowed_domains(self.db(), organisation_id).await?;
        Ok(rows.into_iter().map(row_to_allowed_domain_info).collect())
    }

    pub async fn remove_allowed_domain(
        &self,
        organisation_id: Uuid,
        raw_domain: &str,
        requester_id: Uuid,
    ) -> Result<bool, AllowedDomainServiceError> {
        self.require_admin_typed(organisation_id, requester_id).await?;

        // Normalize so "@Understory.IO" matches what's stored.
        let canonical = domain_policy::normalize_domain(raw_domain)?;

        let removed = self
            .repo
            .remove_allowed_domain(self.db(), organisation_id, &canonical)
            .await
            .map_err(AllowedDomainServiceError::other)?;

        if removed {
            tracing::info!(
                organisation_id = %organisation_id,
                domain = %canonical,
                removed_by = %requester_id,
                "org_allowed_domain.removed"
            );
        }
        Ok(removed)
    }

    /// Look up the allowed-domain row's verification token, query DNS for
    /// `_forest-verify.<domain>` TXT, and flip `dns_verified_at` if any
    /// returned record matches the token verbatim.
    ///
    /// Admin-gated. Returns `Ok(true)` when the row is verified after the
    /// call (either freshly or already-verified), `Ok(false)` when the
    /// expected TXT is absent. Network/DNS errors map to
    /// `VerifyAllowedDomainError::DnsLookup`.
    pub async fn verify_allowed_domain(
        &self,
        organisation_id: Uuid,
        raw_domain: &str,
        requester_id: Uuid,
        resolver: &dyn crate::dns::DnsResolver,
    ) -> Result<VerifyAllowedDomainOutcome, VerifyAllowedDomainError> {
        self.require_admin_typed(organisation_id, requester_id)
            .await
            .map_err(VerifyAllowedDomainError::from_membership)?;

        let canonical = domain_policy::normalize_domain(raw_domain)
            .map_err(VerifyAllowedDomainError::Invalid)?;

        let row = self
            .repo
            .get_allowed_domain(self.db(), organisation_id, &canonical)
            .await
            .map_err(VerifyAllowedDomainError::Other)?
            .ok_or(VerifyAllowedDomainError::NotFound)?;

        // Already verified: no need to re-query DNS. Idempotent success.
        if row.dns_verified_at.is_some() {
            return Ok(VerifyAllowedDomainOutcome::AlreadyVerified);
        }

        let lookup_name = format!("_forest-verify.{canonical}");
        let records = resolver
            .lookup_txt(&lookup_name)
            .await
            .map_err(VerifyAllowedDomainError::DnsLookup)?;

        let matched = records
            .iter()
            .any(|r| r.trim() == row.dns_verification_token);
        if !matched {
            tracing::info!(
                organisation_id = %organisation_id,
                domain = %canonical,
                records_found = records.len(),
                "org_allowed_domain.verify_missing_txt"
            );
            return Ok(VerifyAllowedDomainOutcome::Missing);
        }

        self.repo
            .mark_dns_verified(self.db(), organisation_id, &canonical)
            .await
            .map_err(|e| VerifyAllowedDomainError::Other(anyhow::Error::from(e)))?;

        tracing::info!(
            organisation_id = %organisation_id,
            domain = %canonical,
            verified_by = %requester_id,
            "org_allowed_domain.verified"
        );

        Ok(VerifyAllowedDomainOutcome::Verified)
    }

    pub async fn list_join_offers(
        &self,
        user_id: Uuid,
    ) -> anyhow::Result<Vec<JoinOffer>> {
        let rows = self.repo.list_join_offers(self.db(), user_id).await?;
        Ok(rows
            .into_iter()
            .map(|r| JoinOffer {
                organisation_id: r.organisation_id,
                organisation_name: r.organisation_name,
                matched_domain: r.matched_domain,
            })
            .collect())
    }

    /// Add the user as a `'member'` of the org *iff* they currently have a
    /// verified email at one of the org's allowed domains. The eligibility
    /// check is run inside the same transaction as the insert so a domain
    /// removed concurrently can't be raced.
    pub async fn accept_join_offer(
        &self,
        user_id: Uuid,
        organisation_id: Uuid,
    ) -> Result<MemberInfo, AcceptJoinOfferError> {
        let mut tx = self
            .db()
            .begin()
            .await
            .map_err(|e| AcceptJoinOfferError::Other(anyhow::Error::from(e)))?;

        let offer = self
            .repo
            .find_join_offer(&mut *tx, user_id, organisation_id)
            .await
            .map_err(AcceptJoinOfferError::Other)?
            .ok_or(AcceptJoinOfferError::NotEligible)?;

        let inserted = self
            .repo
            .add_member(&mut *tx, organisation_id, user_id, "member")
            .await
            .map_err(|e| AcceptJoinOfferError::Other(anyhow::Error::from(e)))?;

        let row = self
            .repo
            .get_member_with_username(&mut *tx, organisation_id, user_id)
            .await
            .map_err(AcceptJoinOfferError::Other)?
            .ok_or_else(|| {
                AcceptJoinOfferError::Other(anyhow::anyhow!("member not found after insert"))
            })?;

        tx.commit()
            .await
            .map_err(|e| AcceptJoinOfferError::Other(anyhow::Error::from(e)))?;

        tracing::info!(
            organisation_id = %organisation_id,
            user_id = %user_id,
            domain = %offer.matched_domain,
            "org_auto_invite.accepted"
        );

        Ok(MemberInfo {
            user_id: inserted.user_id,
            username: row.username,
            role: inserted.role,
            joined_at: inserted.created_at,
        })
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

pub struct AllowedDomainInfo {
    pub domain: String,
    pub policy: AllowedDomainPolicy,
    pub dns_verification_token: String,
    pub dns_verified_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: Uuid,
}

pub struct JoinOffer {
    pub organisation_id: Uuid,
    pub organisation_name: String,
    pub matched_domain: String,
}

#[derive(Debug, thiserror::Error)]
pub enum AllowedDomainServiceError {
    #[error(transparent)]
    Invalid(#[from] AllowedDomainError),
    #[error("policy not yet supported in this version")]
    PolicyNotYetSupported,
    #[error("domain is already allowed for this organisation")]
    AlreadyExists,
    #[error("you are not a member of this organisation")]
    NotMember,
    #[error("only admins can perform this action")]
    NotAdmin,
    #[error(transparent)]
    Other(anyhow::Error),
}

impl AllowedDomainServiceError {
    fn other<E: Into<anyhow::Error>>(e: E) -> Self {
        Self::Other(e.into())
    }
}

#[derive(Debug, thiserror::Error)]
enum MembershipError {
    #[error("you are not a member of this organisation")]
    NotMember,
    #[error("only admins can perform this action")]
    NotAdmin,
    #[error(transparent)]
    Db(anyhow::Error),
}

impl From<MembershipError> for AllowedDomainServiceError {
    fn from(e: MembershipError) -> Self {
        match e {
            MembershipError::NotMember => Self::NotMember,
            MembershipError::NotAdmin => Self::NotAdmin,
            MembershipError::Db(e) => Self::Other(e),
        }
    }
}

impl From<PolicyParseError> for AllowedDomainServiceError {
    fn from(e: PolicyParseError) -> Self {
        Self::Other(anyhow::Error::from(e))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyAllowedDomainOutcome {
    /// First-time verification succeeded — `dns_verified_at` was set.
    Verified,
    /// The row was already verified before this call. No-op success.
    AlreadyVerified,
    /// DNS lookup succeeded but no TXT record matched the expected token.
    /// The admin needs to add `_forest-verify.<domain>` with the token.
    Missing,
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyAllowedDomainError {
    #[error(transparent)]
    Invalid(AllowedDomainError),
    #[error("you are not a member of this organisation")]
    NotMember,
    #[error("only admins can verify domains")]
    NotAdmin,
    #[error("allowed-domain entry not found")]
    NotFound,
    #[error("DNS lookup failed: {0}")]
    DnsLookup(anyhow::Error),
    #[error(transparent)]
    Other(anyhow::Error),
}

impl VerifyAllowedDomainError {
    fn from_membership(e: MembershipError) -> Self {
        match e {
            MembershipError::NotMember => Self::NotMember,
            MembershipError::NotAdmin => Self::NotAdmin,
            MembershipError::Db(e) => Self::Other(e),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AcceptJoinOfferError {
    #[error("you are not currently eligible to join this organisation")]
    NotEligible,
    #[error(transparent)]
    Other(anyhow::Error),
}

fn row_to_allowed_domain_info(
    row: crate::repositories::organisations::AllowedDomainRow,
) -> AllowedDomainInfo {
    let policy = AllowedDomainPolicy::parse(&row.policy)
        .unwrap_or(AllowedDomainPolicy::ManualOnly);
    AllowedDomainInfo {
        domain: row.domain,
        policy,
        dns_verification_token: row.dns_verification_token,
        dns_verified_at: row.dns_verified_at,
        created_at: row.created_at,
        created_by: row.created_by,
    }
}

/// 32 bytes of randomness, base64-url-safe-no-pad → 43-char token.
/// Stored alongside the domain row; surfaced to admins for v1.1 DNS TXT
/// verification. v1 generates and stores but doesn't validate.
fn generate_dns_verification_token() -> String {
    let mut buf = [0u8; 32];
    rand::fill(&mut buf[..]);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
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
