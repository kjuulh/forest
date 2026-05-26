// ForagePostgresql CR manifest generation.
//
// Generates a ForagePostgresql custom resource that the
// forage-postgresql-controller reconciles into a real PostgreSQL
// database, user, and Kubernetes Secret with connection credentials.

export interface ForagePostgresqlOptions {
  name: string;
  namespace: string;
  databaseName: string;
  secretName: string;
  secretNamespace?: string;
}

export interface Manifest {
  name: string;
  content: string;
}

/**
 * Build a ForagePostgresql CR manifest.
 */
export function buildForagePostgresqlManifest(opts: ForagePostgresqlOptions): Manifest {
  const secretNamespaceField = opts.secretNamespace
    ? `\n  secretNamespace: ${opts.secretNamespace}`
    : "";

  const content = `---
apiVersion: forage.rawpotion.io/v1alpha1
kind: ForagePostgresql
metadata:
  name: ${opts.name}
  namespace: ${opts.namespace}
spec:
  databaseName: ${opts.databaseName}
  secretName: ${opts.secretName}${secretNamespaceField}
`;

  return { name: "25-forage-postgresql.yaml", content };
}
