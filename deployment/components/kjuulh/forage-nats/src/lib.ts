// NatsUser CR manifest generation.
//
// Generates a NatsUser custom resource that the
// forage-nats-controller reconciles into NATS user credentials
// (JWT + NKey) via the NATS JWT resolver.

export interface SubjectPermission {
  subject: string;
}

export interface ForageNatsOptions {
  name: string;
  namespace: string;
  account: string;
  publish: SubjectPermission[];
  subscribe: SubjectPermission[];
  secretName?: string;
}

export interface Manifest {
  name: string;
  content: string;
}

/**
 * Build a NatsUser CR manifest.
 */
export function buildNatsUserManifest(opts: ForageNatsOptions): Manifest {
  const publishItems = opts.publish
    .map((p) => `    - subject: "${p.subject}"`)
    .join("\n");

  const subscribeItems = opts.subscribe
    .map((s) => `    - subject: "${s.subject}"`)
    .join("\n");

  const secretNameField = opts.secretName
    ? `\n  secretName: ${opts.secretName}`
    : "";

  const content = `---
apiVersion: forage.rawpotion.io/v1alpha1
kind: NatsUser
metadata:
  name: ${opts.name}
  namespace: ${opts.namespace}
spec:
  account: ${opts.account}
  publish:
${publishItems}
  subscribe:
${subscribeItems}${secretNameField}
`;

  return { name: "25-forage-nats.yaml", content };
}
