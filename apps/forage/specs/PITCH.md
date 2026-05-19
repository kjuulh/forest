# Forage - The Platform for Forest

## Elevator Pitch

Forage is the managed platform for Forest. Push a `forest.cue` manifest, get production infrastructure. Think Heroku meets infrastructure-as-code, but built on the composable component model of Forest.

## The Problem

Modern infrastructure tooling is fragmented:
- Kubernetes is powerful but complex - teams spend months just on platform engineering
- Heroku is simple but inflexible - you outgrow it fast
- Infrastructure-as-code tools (Terraform, Pulumi) require deep expertise
- CI/CD pipelines are copy-pasted across projects with slight variations
- Component sharing across teams is ad-hoc at best

Forest solves the composability problem: define workflows, components, and deployments in shareable, typed CUE files. But Forest still needs infrastructure to run on.

## The Solution: Forage

Forage is the missing runtime layer for Forest. It provides:

### 1. Component Registry
- Publish and discover forest components
- Semantic versioning and dependency resolution
- Organisation-scoped and public components
- `forest components publish` pushes to Forage registry

### 2. Managed Deployments
- Push a `forest.cue` with destinations pointing to Forage
- Forage provisions and manages the infrastructure
- Zero-config container runtime (no Kubernetes knowledge needed)
- Automatic scaling, health checks, rollbacks
- Multi-environment support (dev/staging/prod) out of the box

### 3. Managed Services
- **Databases**: PostgreSQL, Redis - provisioned alongside your app
- **Object Storage**: S3-compatible storage
- **User Management**: Auth, teams, RBAC
- **Observability**: Logs, metrics, traces - included by default
- **Secrets Management**: Encrypted at rest, injected at runtime

### 4. Organisation Management
- Team workspaces with role-based access
- Billing per organisation
- Audit logs for compliance
- SSO/SAML integration

## How It Works

```cue
// forest.cue - This is all you need
project: {
  name: "my-api"
  organisation: "acme"
}

dependencies: {
  "forage/service": version: "1.0"
  "forage/postgres": version: "1.0"
}

forage: service: {
  config: {
    name: "my-api"
    image: "my-api:latest"
    ports: [{ container: 8080, protocol: "http" }]
  }
  env: {
    prod: {
      destinations: [{
        type: { organisation: "forage", name: "managed", version: "1" }
        metadata: { region: "eu-west-1", size: "small" }
      }]
    }
  }
}

forage: postgres: {
  config: {
    name: "my-db"
    version: "16"
    size: "small"
  }
}
```

Then:
```bash
forest release create --env prod
# Forage handles everything: container runtime, database provisioning,
# networking, TLS, DNS, health checks, scaling
```

## Target Users

### Primary: Small-to-Medium Engineering Teams (5-50 engineers)
- Need production infrastructure without a dedicated platform team
- Want the flexibility of IaC without the complexity
- Already using or willing to adopt Forest for workflow management

### Secondary: Individual Developers / Startups
- Want to ship fast without infrastructure overhead
- Need a path that scales from prototype to production
- Price-sensitive - pay only for what you use

### Tertiary: Enterprise Teams
- Want to standardize deployment across many teams
- Need compliance, audit, and access control
- Want to share internal components via private registry

## Pricing Model

### Free Tier
- 1 project, 1 environment
- 256MB RAM, shared CPU
- Community components only
- Ideal for experimentation

### Developer - $10/month
- 3 projects, 3 environments each
- 512MB RAM per service, dedicated CPU
- 1GB PostgreSQL included
- Custom domains

### Team - $25/user/month
- Unlimited projects and environments
- Configurable resources (up to 4GB RAM, 2 vCPU)
- 10GB PostgreSQL per project
- Private component registry
- Team management, RBAC

### Enterprise - Custom
- Dedicated infrastructure
- SLA guarantees
- SSO/SAML
- Audit logs
- Priority support
- On-premise registry option

### Usage-Based Add-ons
- Additional compute: $0.05/vCPU-hour
- Additional memory: $0.01/GB-hour
- Additional storage: $0.10/GB-month
- Bandwidth: $0.05/GB after 10GB free
- Managed databases: Starting at $5/month per instance

## Competitive Positioning

| Feature | Forage | Heroku | Railway | Fly.io | K8s (self-managed) |
|---------|--------|--------|---------|--------|---------------------|
| Simplicity | High | High | High | Medium | Low |
| Flexibility | High (CUE) | Low | Medium | Medium | Very High |
| Component Sharing | Native | None | None | None | Helm (limited) |
| Multi-environment | Native | Add-on | Basic | Manual | Manual |
| IaC Integration | Native (Forest) | None | None | Partial | Full |
| Price Entry | Free | $5/mo | $5/mo | $0 (usage) | $$$$ |
| Workflow Automation | Forest native | CI add-ons | Basic | Basic | Custom |

## Differentiators

1. **Forest-native**: Not another generic PaaS. Built specifically to make Forest's component model a deployable reality.
2. **Typed Manifests**: CUE gives you type-safe infrastructure definitions with validation before deploy.
3. **Component Ecosystem**: Publish once, use everywhere. Components are the unit of sharing.
4. **Progressive Complexity**: Start simple, add complexity only when needed. No cliff.
5. **Transparent Pricing**: No surprises. Usage-based with clear ceilings.

## Technical Architecture

### The Site (this repo)
- **Rust + Axum**: Fast, safe, minimal dependencies
- **MiniJinja**: Server-side rendered - fast page loads, SEO-friendly
- **Tailwind CSS**: Utility-first, consistent design
- **PostgreSQL**: Battle-tested data layer

### The Platform (future repos)
- **Container Runtime**: Built on Firecracker/Cloud Run/ECS depending on region
- **Registry Service**: gRPC service for component distribution (extends forest-server)
- **Deployment Engine**: Receives forest manifests, provisions infrastructure
- **Billing Service**: Usage tracking, Stripe integration

## Roadmap

### Phase 0 - Foundation (Current)
- [ ] Marketing site with pitch, pricing, and waitlist
- [ ] Component registry browser (read-only, pulls from forest-server)
- [ ] Authentication (sign up, sign in, API keys)
- [ ] Organisation and project management UI

### Phase 1 - Registry
- [ ] Component publishing via CLI (`forest components publish`)
- [ ] Component discovery and browsing
- [ ] Version management and dependency resolution
- [ ] Private organisation registries

### Phase 2 - Managed Deployments
- [ ] Container runtime integration
- [ ] Push-to-deploy from forest CLI
- [ ] Health checks and automatic rollbacks
- [ ] Environment management (dev/staging/prod)
- [ ] Custom domains and TLS

### Phase 3 - Managed Services
- [ ] PostgreSQL provisioning
- [ ] Redis provisioning
- [ ] Object storage
- [ ] Secrets management

### Phase 4 - Enterprise
- [ ] SSO/SAML
- [ ] Audit logging
- [ ] Compliance features
- [ ] On-premise options
