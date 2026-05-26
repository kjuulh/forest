// Sealed secrets library for forest components.
//
// Provides kubeseal integration for sealing secret values and
// reading sealed secret manifests per environment.

export interface SealOptions {
  namespace: string;
  secretName: string;
  key: string;
  value: string;
  cert: string;
  env: string;
}

export interface Manifest {
  name: string;
  content: string;
}

const SEALED_SECRET_TEMPLATE = (name: string, namespace: string) => `---
apiVersion: bitnami.com/v1alpha1
kind: SealedSecret
metadata:
  name: ${name}
  namespace: ${namespace}
spec:
  encryptedData: {}
  template:
    metadata:
      name: ${name}
      namespace: ${namespace}
    type: Opaque
`;

/**
 * Encrypt a single value using kubeseal --raw.
 */
export async function kubesealRaw(
  value: string,
  namespace: string,
  secretName: string,
  cert: string,
): Promise<string> {
  try {
    await Deno.stat(cert);
  } catch {
    throw new Error(
      `kubeseal certificate file not found: ${cert}\n` +
      `hint: the cert parameter must be a file path, not the certificate content.\n` +
      `      download the cert with: kubeseal --fetch-cert --controller-namespace <ns> > pub-cert.pem`,
    );
  }

  const cmd = new Deno.Command("kubeseal", {
    args: [
      "--raw",
      "--from-file=/dev/stdin",
      "--namespace", namespace,
      "--name", secretName,
      "--scope", "strict",
      "--cert", cert,
    ],
    stdin: "piped",
    stdout: "piped",
    stderr: "piped",
  });

  const child = cmd.spawn();
  const writer = child.stdin.getWriter();
  await writer.write(new TextEncoder().encode(value));
  await writer.close();

  const output = await child.output();
  if (!output.success) {
    const stderr = new TextDecoder().decode(output.stderr);
    throw new Error(`kubeseal failed: ${stderr}`);
  }

  return new TextDecoder().decode(output.stdout).trim();
}

/**
 * Patch a sealed value into a SealedSecret YAML string.
 * Adds or replaces the key under spec.encryptedData.
 */
export function patchEncryptedData(yaml: string, key: string, sealedValue: string): string {
  const lines = yaml.split("\n");
  const result: string[] = [];
  let inEncryptedData = false;
  let inserted = false;
  const indent = "    ";

  for (const line of lines) {
    if (line.trimEnd() === "  encryptedData: {}") {
      result.push("  encryptedData:");
      result.push(`${indent}${key}: ${sealedValue}`);
      inserted = true;
      continue;
    }

    if (line.trimEnd() === "  encryptedData:") {
      inEncryptedData = true;
      result.push(line);
      continue;
    }

    if (inEncryptedData) {
      if (line.startsWith(indent) && line.trim() !== "") {
        const existingKey = line.trim().split(":")[0];
        if (existingKey === key) {
          result.push(`${indent}${key}: ${sealedValue}`);
          inserted = true;
          continue;
        }
        result.push(line);
        continue;
      }
      if (!inserted) {
        result.push(`${indent}${key}: ${sealedValue}`);
        inserted = true;
      }
      inEncryptedData = false;
    }

    result.push(line);
  }

  return result.join("\n");
}

/**
 * Seal a secret key into the per-env sealed secret file.
 * Creates the file if it doesn't exist.
 */
export async function sealSecret(opts: SealOptions): Promise<void> {
  const secretFile = `secrets/${opts.env}.sealed-secret.yaml`;

  let yaml: string;
  try {
    yaml = await Deno.readTextFile(secretFile);
  } catch (err) {
    if (err instanceof Deno.errors.NotFound) {
      await Deno.mkdir("secrets", { recursive: true });
      yaml = SEALED_SECRET_TEMPLATE(opts.secretName, opts.namespace);
    } else {
      throw err;
    }
  }

  console.error(`sealing ${opts.key} for ${opts.namespace}/${opts.env}...`);
  const sealedValue = await kubesealRaw(opts.value, opts.namespace, opts.secretName, opts.cert);

  yaml = patchEncryptedData(yaml, opts.key, sealedValue);
  await Deno.writeTextFile(secretFile, yaml);

  console.error(`sealed ${opts.key} into ${secretFile}`);
}

/**
 * Load the sealed secret manifest for an environment.
 * Returns null if no sealed secret file exists.
 */
export async function loadSealedSecretManifest(env: string): Promise<Manifest | null> {
  const secretFile = `secrets/${env}.sealed-secret.yaml`;
  try {
    const content = await Deno.readTextFile(secretFile);
    return { name: "20-sealed-secrets.yaml", content };
  } catch (err) {
    if (err instanceof Deno.errors.NotFound) {
      return null;
    }
    throw err;
  }
}
