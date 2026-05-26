import { runOnce, type CallContext } from "@rawpotion/forest-sdk";
import {
  createRouter,
  type Spec,
  type CommandHandler,
  type ForestDeploymentHookHandler,
  type SealInput,
  type SealOutput,
  type StatusInput,
  type StatusOutput,
  type ValidateInput,
  type ValidateOutput,
  type ForestDeploymentPrepareInput,
  type ForestDeploymentPrepareOutput,
  type ForestDeploymentReleaseInput,
  type ForestDeploymentReleaseOutput,
  type ForestDeploymentRollbackInput,
  type Manifest,
} from "./forestgen.ts";
import * as sealedSecrets from "./deps/kjuulh_sealed-secrets.ts";
import * as foragePostgresql from "./deps/kjuulh_forage-postgresql.ts";
import * as forageNats from "./deps/kjuulh_forage-nats.ts";
import * as forageS3 from "./deps/kjuulh_forage-s3.ts";

// ── Commands ──

const commands: CommandHandler = {
  async seal(
    spec: Spec,
    input: SealInput,
    _context: CallContext,
  ): Promise<SealOutput> {
    await sealedSecrets.commandsSeal(
      { name: `${spec.name}-secrets`, namespace: input.env },
      input,
    );
    return {};
  },

  async validate(
    spec: Spec,
    _input: ValidateInput,
    _context: CallContext,
  ): Promise<ValidateOutput> {
    const errors: string[] = [];
    if (!spec.name) errors.push("name is required");
    if (!spec.image) errors.push("image is required");
    if (!spec.host) errors.push("host is required");
    return { valid: errors.length === 0, errors };
  },

  async status(
    spec: Spec,
    _input: StatusInput,
    _context: CallContext,
  ): Promise<StatusOutput> {
    console.error(`checking status for ${spec.name}`);
    return { healthy: true };
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
      console.error("warning: no environment in context, skipping prepare hooks");
      return { manifests: [] };
    }

    const manifests: Manifest[] = [];

    // Sealed secrets
    const sealedResult = await sealedSecrets.hooksForestDeploymentPrepare(
      { name: `${spec.name}-secrets`, namespace: env },
      {},
    );
    if (sealedResult.manifests.length > 0) {
      console.error(`loaded sealed secrets for ${spec.name}/${env}`);
      manifests.push(...sealedResult.manifests);
    }

    // ForagePostgresql — derive name/namespace/database from service name
    if (spec.forage_postgresql) {
      const pg = spec.forage_postgresql;
      const pgSpec: foragePostgresql.Spec = {
        name: `${spec.name}-db`,
        namespace: env,
        database_name: pg.database_name ?? spec.name,
        secret_name: pg.secret_name ?? `${spec.name}-db-credentials`,
        secret_namespace: pg.secret_namespace,
      };
      const pgResult = await foragePostgresql.hooksForestDeploymentPrepare(pgSpec, {});
      if (pgResult.manifests.length > 0) {
        console.error(`generated ForagePostgresql CR for ${pgSpec.name}/${env}`);
        manifests.push(...pgResult.manifests);
      }
    }

    // ForageNats — derive name/namespace/account from service name
    if (spec.forage_nats) {
      const natsSpec: forageNats.Spec = {
        name: `${spec.name}-nats`,
        namespace: env,
        account: spec.name.toUpperCase(),
        publish: spec.forage_nats.publish,
        subscribe: spec.forage_nats.subscribe,
        secret_name: spec.forage_nats.secret_name,
      };
      const natsResult = await forageNats.hooksForestDeploymentPrepare(natsSpec, {});
      if (natsResult.manifests.length > 0) {
        console.error(`generated NatsUser CR for ${natsSpec.name}/${env}`);
        manifests.push(...natsResult.manifests);
      }
    }

    // ForageS3 — derive name/namespace/bucket from service name
    if (spec.forage_s3) {
      const s3 = spec.forage_s3;
      const s3Spec: forageS3.Spec = {
        name: `${spec.name}-s3`,
        namespace: env,
        bucket_name: s3.bucket_name ?? spec.name,
        key_name: s3.key_name,
        secret_name: s3.secret_name,
        quotas: s3.quotas,
        permissions: s3.permissions,
      };
      const s3Result = await forageS3.hooksForestDeploymentPrepare(s3Spec, {});
      if (s3Result.manifests.length > 0) {
        console.error(`generated S3Bucket CR for ${s3Spec.name}/${env}`);
        manifests.push(...s3Result.manifests);
      }
    }

    return { manifests };
  },

  async release(
    spec: Spec,
    input: ForestDeploymentReleaseInput,
    _context: CallContext,
  ): Promise<ForestDeploymentReleaseOutput> {
    console.error(`releasing ${spec.name} (release=${input.release_id})`);
    return {};
  },

  async rollback(
    spec: Spec,
    input: ForestDeploymentRollbackInput,
    _context: CallContext,
  ): Promise<void> {
    console.error(`rolling back ${spec.name} (release=${input.release_id}, target=${input.target_revision ?? "latest"})`);
  },
};

// ── Entry point ──

const router = createRouter(commands, deploymentHooks);
runOnce(router);
