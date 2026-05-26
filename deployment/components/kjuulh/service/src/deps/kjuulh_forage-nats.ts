// Generated dependency client for kjuulh/forage-nats. Do not edit.

import { callComponent } from "@rawpotion/forest-sdk";

export interface DeploymentHooks {
  prepare: { description: string; input: Record<string, unknown>; output: { manifests: Manifest[] } };
  release: { description: string; input: { release_id: string }; output: Record<string, unknown> };
  rollback: { description: string; input: { release_id: string; target_revision: string } };
}

export interface Manifest {
  content: string;
  name: string;
}

export interface Spec {
  account: string;
  name: string;
  namespace: string;
  publish: { subject: string }[];
  secret_name?: string;
  subscribe: { subject: string }[];
}

export interface ForestDeploymentPrepareInput {
}

export interface ForestDeploymentPrepareOutput {
  manifests: Manifest[];
}

/** Inject NatsUser CR manifest */
export function hooksForestDeploymentPrepare(spec: Spec, input: ForestDeploymentPrepareInput): Promise<ForestDeploymentPrepareOutput> {
  return callComponent("kjuulh/forage-nats", "hooks/forest/deployment/prepare", spec, input);
}

export interface ForestDeploymentReleaseInput {
  release_id: string;
}

export interface ForestDeploymentReleaseOutput {
}

/** No-op */
export function hooksForestDeploymentRelease(spec: Spec, input: ForestDeploymentReleaseInput): Promise<ForestDeploymentReleaseOutput> {
  return callComponent("kjuulh/forage-nats", "hooks/forest/deployment/release", spec, input);
}

export interface ForestDeploymentRollbackInput {
  release_id: string;
  target_revision: string;
}

/** No-op */
export function hooksForestDeploymentRollback(spec: Spec, input: ForestDeploymentRollbackInput): Promise<void> {
  return callComponent("kjuulh/forage-nats", "hooks/forest/deployment/rollback", spec, input);
}

