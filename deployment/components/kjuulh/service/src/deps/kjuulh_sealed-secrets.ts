// Generated dependency client for kjuulh/sealed-secrets. Do not edit.

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
  name: string;
  namespace: string;
}

export interface SealInput {
  cert: string;
  env: string;
  key: string;
  value: string;
}

export interface SealOutput {
}

/** Add or update a sealed secret key */
export function commandsSeal(spec: Spec, input: SealInput): Promise<SealOutput> {
  return callComponent("kjuulh/sealed-secrets", "commands/seal", spec, input);
}

export interface ForestDeploymentPrepareInput {
}

export interface ForestDeploymentPrepareOutput {
  manifests: Manifest[];
}

/** Inject sealed secrets manifest */
export function hooksForestDeploymentPrepare(spec: Spec, input: ForestDeploymentPrepareInput): Promise<ForestDeploymentPrepareOutput> {
  return callComponent("kjuulh/sealed-secrets", "hooks/forest/deployment/prepare", spec, input);
}

export interface ForestDeploymentReleaseInput {
  release_id: string;
}

export interface ForestDeploymentReleaseOutput {
}

/** No-op */
export function hooksForestDeploymentRelease(spec: Spec, input: ForestDeploymentReleaseInput): Promise<ForestDeploymentReleaseOutput> {
  return callComponent("kjuulh/sealed-secrets", "hooks/forest/deployment/release", spec, input);
}

export interface ForestDeploymentRollbackInput {
  release_id: string;
  target_revision: string;
}

/** No-op */
export function hooksForestDeploymentRollback(spec: Spec, input: ForestDeploymentRollbackInput): Promise<void> {
  return callComponent("kjuulh/sealed-secrets", "hooks/forest/deployment/rollback", spec, input);
}

