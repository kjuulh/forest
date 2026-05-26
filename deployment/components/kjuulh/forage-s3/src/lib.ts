// S3Bucket CR manifest generation.
//
// Generates an S3Bucket custom resource that the
// forage-s3-controller reconciles into a Garage S3 bucket,
// access key, and Kubernetes Secret with credentials.

export interface BucketQuotas {
  maxSize?: number;
  maxObjects?: number;
}

export interface BucketPermissions {
  read: boolean;
  write: boolean;
  owner: boolean;
}

export interface ForageS3Options {
  name: string;
  namespace: string;
  bucketName: string;
  keyName?: string;
  secretName?: string;
  quotas?: BucketQuotas;
  permissions?: BucketPermissions;
}

export interface Manifest {
  name: string;
  content: string;
}

/**
 * Build an S3Bucket CR manifest.
 */
export function buildS3BucketManifest(opts: ForageS3Options): Manifest {
  const specLines: string[] = [
    `  bucketName: ${opts.bucketName}`,
  ];

  if (opts.keyName) {
    specLines.push(`  keyName: ${opts.keyName}`);
  }
  if (opts.secretName) {
    specLines.push(`  secretName: ${opts.secretName}`);
  }
  if (opts.quotas) {
    specLines.push("  quotas:");
    if (opts.quotas.maxSize !== undefined) {
      specLines.push(`    maxSize: ${opts.quotas.maxSize}`);
    }
    if (opts.quotas.maxObjects !== undefined) {
      specLines.push(`    maxObjects: ${opts.quotas.maxObjects}`);
    }
  }
  if (opts.permissions) {
    specLines.push("  permissions:");
    specLines.push(`    read: ${opts.permissions.read}`);
    specLines.push(`    write: ${opts.permissions.write}`);
    specLines.push(`    owner: ${opts.permissions.owner}`);
  }

  const content = `---
apiVersion: forage.rawpotion.io/v1alpha1
kind: S3Bucket
metadata:
  name: ${opts.name}
  namespace: ${opts.namespace}
spec:
${specLines.join("\n")}
`;

  return { name: "25-forage-s3.yaml", content };
}
