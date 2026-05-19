import { runOnce } from "@forest/sdk";
import {
  createRouter,
  type Spec,
  type CommandHandler,
  type ForestDeploymentHookHandler,
  type PrepareInput,
  type PrepareOutput,
  type StatusInput,
  type StatusOutput,
  type ValidateInput,
  type ValidateOutput,
  type ForestDeploymentPrepareInput,
  type ForestDeploymentPrepareOutput,
  type ForestDeploymentReleaseInput,
  type ForestDeploymentReleaseOutput,
  type ForestDeploymentRollbackInput,
} from "./forestgen.ts";

// ── Commands ──

const commands: CommandHandler = {
  async prepare(
    spec: Spec,
    _input: PrepareInput,
  ): Promise<PrepareOutput> {
    console.error(`[deno-terraform] preparing ${spec.name}`);
    return { manifests: [] };
  },

  async status(
    spec: Spec,
    _input: StatusInput,
  ): Promise<StatusOutput> {
    console.error(`[deno-terraform] checking status for ${spec.name}`);
    return { healthy: true };
  },

  async validate(
    spec: Spec,
    _input: ValidateInput,
  ): Promise<ValidateOutput> {
    const errors: string[] = [];

    if (!spec.name) {
      errors.push("name is required");
    }
    if (spec.replicas < 1 || spec.replicas > 100) {
      errors.push(`replicas must be between 1 and 100, got ${spec.replicas}`);
    }
    for (const port of spec.ports ?? []) {
      if (port.port < 1 || port.port > 65535) {
        errors.push(`port ${port.name}: value ${port.port} out of range`);
      }
    }

    return { valid: errors.length === 0, errors };
  },
};

// ── Deployment hooks ──

const deploymentHooks: ForestDeploymentHookHandler = {
  async prepare(
    spec: Spec,
    _input: ForestDeploymentPrepareInput,
  ): Promise<ForestDeploymentPrepareOutput> {
    console.error(`[deno-terraform] deployment prepare for ${spec.name}`);
    return { manifests: [] };
  },

  async release(
    spec: Spec,
    input: ForestDeploymentReleaseInput,
  ): Promise<ForestDeploymentReleaseOutput> {
    console.error(
      `[deno-terraform] releasing ${spec.name} (release=${input.releaseId})`,
    );
    return {};
  },

  async rollback(
    spec: Spec,
    input: ForestDeploymentRollbackInput,
  ): Promise<void> {
    console.error(
      `[deno-terraform] rolling back ${spec.name} (release=${input.releaseId}, target=${input.targetRevision ?? "latest"})`,
    );
  },
};

// ── Entry point ──

const router = createRouter(commands, deploymentHooks);
runOnce(router);
