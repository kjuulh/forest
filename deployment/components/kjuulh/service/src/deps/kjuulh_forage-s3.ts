// Generated dependency client for kjuulh/forage-s3. Do not edit.

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
  bucket_name: string;
  key_name?: string;
  name: string;
  namespace: string;
  permissions?: { owner: boolean; read: boolean; write: boolean };
  quotas?: { max_objects?: number; max_size?: number };
  secret_name?: string;
}

export interface ForestDeploymentPrepareInput {
}

export interface ForestDeploymentPrepareOutput {
  manifests: Manifest[];
}

/** Inject S3Bucket CR manifest */
export function hooksForestDeploymentPrepare(spec: Spec, input: ForestDeploymentPrepareInput): Promise<ForestDeploymentPrepareOutput> {
  return callComponent("kjuulh/forage-s3", "hooks/forest/deployment/prepare", spec, input);
}

export interface ForestDeploymentReleaseInput {
  release_id: string;
}

export interface ForestDeploymentReleaseOutput {
}

/** No-op */
export function hooksForestDeploymentRelease(spec: Spec, input: ForestDeploymentReleaseInput): Promise<ForestDeploymentReleaseOutput> {
  return callComponent("kjuulh/forage-s3", "hooks/forest/deployment/release", spec, input);
}

export interface ForestDeploymentRollbackInput {
  release_id: string;
  target_revision: string;
}

/** No-op */
export function hooksForestDeploymentRollback(spec: Spec, input: ForestDeploymentRollbackInput): Promise<void> {
  return callComponent("kjuulh/forage-s3", "hooks/forest/deployment/rollback", spec, input);
}

