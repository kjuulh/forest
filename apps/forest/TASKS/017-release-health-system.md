# 017: Release Health Monitoring System

## Overview

End-to-end release health monitoring: an in-cluster agent watches kubernetes resources annotated with forest release metadata, pushes health observations to the forest server via gRPC, and policies gate pipeline progression based on health status.

## Architecture

```
forest release create → manifests with forest.sh/* annotations deployed
  → health agent detects annotated Kustomizations
  → agent watches Kustomization status + managed Deployments/Pods
  → agent pushes HealthObservation to forest server via gRPC
  → server stores in release_health_observations table
  → server publishes to NATS forest.release.health.{intent_id}
  → WaitRelease stream includes health events
  → health policy evaluates: all destinations HEALTHY for N seconds?
  → pipeline stage advances (or blocks)
```

## Phase 1: Resource Annotations (Flux Destination)

**Goal:** Kustomization CRs carry release identity so the agent can correlate.

**File:** `crates/forest-runner/src/destinations/fluxv1.rs`

Add to `generate_kustomization_cr()`:
```yaml
metadata:
  labels:
    forest.sh/managed: "true"
    forest.sh/organisation: "{org}"
    forest.sh/project: "{project}"
  annotations:
    forest.sh/release-intent-id: "{release_intent_id}"
    forest.sh/artifact-id: "{artifact_id}"
    forest.sh/destination: "{destination_name}"
    forest.sh/environment: "{environment}"
```

Requires threading release identity through `DestinationBackend` → `FluxV1Handler::run()` → CR generator. The `WorkAssignment` proto already has `release_id`, `release_intent_id`, `artifact_id`.

## Phase 2: Proto Definitions

**File:** `interface/proto/forest/v1/health.proto`

```protobuf
service ReleaseHealthService {
  rpc ReportHealth(ReportHealthRequest) returns (ReportHealthResponse);
  rpc GetReleaseHealth(GetReleaseHealthRequest) returns (GetReleaseHealthResponse);
  rpc WatchReleaseHealth(WatchReleaseHealthRequest) returns (stream ReleaseHealthEvent);
}

message ReportHealthRequest {
  string release_intent_id = 1;
  string organisation = 3;
  string project = 4;
  string destination = 5;
  string environment = 6;
  HealthObservation observation = 10;
}

message HealthObservation {
  ResourceHealthSummary kustomization = 1;
  repeated ResourceHealth resources = 2;
  string observed_at = 3;
}

message ResourceHealth {
  string api_version = 1;
  string kind = 2;
  string name = 3;
  string namespace = 4;
  HealthStatus status = 5;
  string message = 6;
  optional int32 desired_replicas = 10;
  optional int32 ready_replicas = 11;
}

enum HealthStatus {
  HEALTH_STATUS_UNSPECIFIED = 0;
  HEALTH_STATUS_HEALTHY = 1;
  HEALTH_STATUS_PROGRESSING = 2;
  HEALTH_STATUS_DEGRADED = 3;
  HEALTH_STATUS_UNHEALTHY = 4;
  HEALTH_STATUS_MISSING = 5;
}
```

Extend `releases.proto` WaitReleaseEvent:
```protobuf
message WaitReleaseEvent {
  oneof event {
    ReleaseStatusUpdate status_update = 1;
    ReleaseLogLine log_line = 2;
    PipelineStageUpdate stage_update = 3;
    ReleaseHealthEvent health_update = 4;
  }
}
```

## Phase 3: Database Schema

```sql
CREATE TABLE release_health_observations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    release_intent_id UUID NOT NULL,
    destination_name TEXT NOT NULL,
    environment TEXT NOT NULL,
    organisation TEXT NOT NULL,
    project TEXT NOT NULL,
    observation JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'PROGRESSING',
    observed_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX idx_health_obs_intent_destination
    ON release_health_observations (release_intent_id, destination_name);
```

Upsert on each agent report. Publish NATS `forest.release.health.{intent_id}`.

## Phase 4: Server Health Service

- `services/release_health.rs` — upsert, query, evaluate
- `grpc/health.rs` — ReportHealth, GetReleaseHealth, WatchReleaseHealth
- Extend `grpc/release.rs` WaitRelease to subscribe to health NATS subject
- Agent authenticates via service account API key

## Phase 5: Health Policy

New `PolicyType::ReleaseHealth` with config:
- `healthy_duration_seconds` — minimum time all destinations must be HEALTHY
- `acceptable_statuses` — which statuses pass (default: only HEALTHY)

IntentCoordinator evaluates health policy during pipeline stage sweep. Subscribes to `forest.release.health.*` for wake signals.

## Phase 6: Health Agent

New repo `forest-kubernetes-agent` (like forage-postgresql-controller, forage-nats-controller):
- Own repo with Cargo workspace, Dagger CI, Forest deployment components
- Deployed via its own forest.cue + controller-service component
- Watches Flux Kustomization CRs with label `forest.sh/managed=true`
- Reads `forest.sh/*` annotations for release identity
- Checks Kustomization `.status.conditions[Ready]`
- Lists managed resources from Kustomization `.status.inventory`
- For Deployments: checks `spec.replicas` vs `status.readyReplicas`
- Synthesizes `HealthObservation`, calls `ReportHealth` via gRPC
- Reports every 15-30s or on watch events

Dependencies: `kube`, `tonic`, `forest-grpc-interface`

Deployed via forest-infrastructure as a Deployment + ClusterRole (read Kustomizations, Deployments, Pods).

## Phase 7: CLI Integration

Health events flow through existing WaitRelease stream. No new CLI flags needed:

```
[dev] flux-dev/home/001  HEALTH: PROGRESSING - Deployment forest: 0/1 ready
[dev] flux-dev/home/001  HEALTH: HEALTHY - All resources healthy
```

## Implementation Order

1. Phase 1 (annotations) + Phase 2 (proto) — parallel, prerequisites
2. Phase 3 (schema) — depends on proto
3. Phase 4 (server) — depends on schema + proto
4. Phase 6 (agent) — depends on proto, parallel with Phase 4
5. Phase 5 (policy) — depends on Phase 4
6. Phase 7 (CLI) — depends on Phase 4
