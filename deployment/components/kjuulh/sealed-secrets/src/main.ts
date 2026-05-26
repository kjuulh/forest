import { runOnce, type CallContext } from "@rawpotion/forest-sdk";
import {
  createRouter,
  type Spec,
  type CommandHandler,
  type ForestDeploymentHookHandler,
  type SealInput,
  type SealOutput,
  type ForestDeploymentPrepareInput,
  type ForestDeploymentPrepareOutput,
  type ForestDeploymentReleaseInput,
  type ForestDeploymentReleaseOutput,
  type ForestDeploymentRollbackInput,
} from "./forestgen.ts";
import { sealSecret, loadSealedSecretManifest } from "./lib.ts";

// ── Commands ──

const commands: CommandHandler = {
  async seal(
    spec: Spec,
    input: SealInput,
    _context: CallContext,
  ): Promise<SealOutput> {
    await sealSecret({
      namespace: spec.namespace,
      secretName: spec.name,
      key: input.key,
      value: input.value,
      cert: input.cert,
      env: input.env,
    });
    return {};
  },
};

// ── Deployment hooks ──

const deploymentHooks: ForestDeploymentHookHandler = {
  async prepare(
    spec: Spec,
    _input: ForestDeploymentPrepareInput,
    context: CallContext,
  ): Promise<ForestDeploymentPrepareOutput> {
    const env = context.environment;
    if (!env) {
      console.error("warning: no environment in context, skipping sealed secrets");
      return { manifests: [] };
    }

    const manifest = await loadSealedSecretManifest(env);
    if (manifest) {
      console.error(`loaded sealed secrets for ${spec.name}/${env}`);
      return { manifests: [manifest] };
    }

    console.error(`no sealed secrets for ${spec.name}/${env}, skipping`);
    return { manifests: [] };
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

const router = createRouter(commands, deploymentHooks);
runOnce(router);
