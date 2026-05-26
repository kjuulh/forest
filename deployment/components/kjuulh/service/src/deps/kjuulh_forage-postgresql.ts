// Generated dependency client for kjuulh/forage-postgresql. Do not edit.

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
  database_name: string;
  name: string;
  namespace: string;
  secret_name: string;
  secret_namespace?: string;
}

export interface ForestDeploymentPrepareInput {
}

export interface ForestDeploymentPrepareOutput {
  manifests: Manifest[];
}

/** Inject ForagePostgresql CR manifest */
export function hooksForestDeploymentPrepare(spec: Spec, input: ForestDeploymentPrepareInput): Promise<ForestDeploymentPrepareOutput> {
  return callComponent("kjuulh/forage-postgresql", "hooks/forest/deployment/prepare", spec, input);
}

export interface ForestDeploymentReleaseInput {
  release_id: string;
}

export interface ForestDeploymentReleaseOutput {
}

/** No-op */
export function hooksForestDeploymentRelease(spec: Spec, input: ForestDeploymentReleaseInput): Promise<ForestDeploymentReleaseOutput> {
  return callComponent("kjuulh/forage-postgresql", "hooks/forest/deployment/release", spec, input);
}

export interface ForestDeploymentRollbackInput {
  release_id: string;
  target_revision: string;
}

/** No-op */
export function hooksForestDeploymentRollback(spec: Spec, input: ForestDeploymentRollbackInput): Promise<void> {
  return callComponent("kjuulh/forage-postgresql", "hooks/forest/deployment/rollback", spec, input);
}

