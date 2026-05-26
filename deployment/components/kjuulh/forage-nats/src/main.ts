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
import { buildNatsUserManifest } from "./lib.ts";

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
      console.error("warning: no environment in context, skipping forage-nats");
      return { manifests: [] };
    }

    const manifest = buildNatsUserManifest({
      name: spec.name,
      namespace: spec.namespace,
      account: spec.account,
      publish: spec.publish,
      subscribe: spec.subscribe,
      secretName: spec.secret_name,
    });

    console.error(`generated NatsUser CR for ${spec.name}/${env}`);
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
