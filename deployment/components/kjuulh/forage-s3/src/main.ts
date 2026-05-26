import { runOnce, type CallContext } from "@rawpotion/forest-sdk";
import {
  createRouter,
  type Spec,
  type ForestDeploymentHookHandler,
  type ForestDeploymentPrepareInput,
  type ForestDeploymentPrepareOutput,
  type ForestDeploymentReleaseInput,
  type ForestDeploymentReleaseOutput,
  type ForestDeploymentRollbackInput,
} from "./forestgen.ts";
import { buildS3BucketManifest } from "./lib.ts";

// ── Commands ──


// ── Deployment hooks ──

const deploymentHooks: ForestDeploymentHookHandler = {
  async prepare(
    spec: Spec,
    _input: ForestDeploymentPrepareInput,
    context: CallContext,
  ): Promise<ForestDeploymentPrepareOutput> {
    const env = context.environment;
    if (!env) {
      console.error("warning: no environment in context, skipping forage-s3");
      return { manifests: [] };
    }

    const manifest = buildS3BucketManifest({
      name: spec.name,
      namespace: spec.namespace,
      bucketName: spec.bucket_name,
      keyName: spec.key_name,
      secretName: spec.secret_name,
      quotas: spec.quotas ? {
        maxSize: spec.quotas.max_size,
        maxObjects: spec.quotas.max_objects,
      } : undefined,
      permissions: spec.permissions,
    });

    console.error(`generated S3Bucket CR for ${spec.name}/${env}`);
    return { manifests: [manifest] };
  },

  async release(
    _spec: Spec,
    _input: ForestDeploymentReleaseInput,
    _context: CallContext,
  ): Promise<ForestDeploymentReleaseOutput> {
    return {};
  },

  async rollback(
    _spec: Spec,
    _input: ForestDeploymentRollbackInput,
    _context: CallContext,
  ): Promise<void> {},
};

// ── Entry point ──

const router = createRouter(deploymentHooks);
runOnce(router);
