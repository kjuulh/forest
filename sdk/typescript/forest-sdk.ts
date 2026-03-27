// Forest SDK for Deno/TypeScript
//
// Single-file SDK that implements the Forest component protocol.
// Components are invoked as subprocesses with JSON over stdin/stdout.
//
// Usage:
//   import { runOnce, type ComponentService } from "./forest-sdk.ts";
//   runOnce(myRouter);

// ============================================================
// Types
// ============================================================

export interface CallContext {
  project?: string;
  organisation?: string;
  environment?: string;
  release_id?: string;
  work_dir?: string;
  dry_run?: boolean;
}

export type MethodKind = "command" | "hook";

export interface MethodDescriptor {
  name: string;
  kind: MethodKind;
  topic?: string;
  description?: string;
}

export interface ComponentDescriptor {
  protocol_version: string;
  methods: MethodDescriptor[];
}

export interface TemplateConfig {
  skip: string[];
  rename: Record<string, string>;
  vars: Record<string, unknown>;
}

export interface ComponentService<S> {
  call(
    method: string,
    spec: S,
    input: unknown,
    context: CallContext,
  ): Promise<unknown>;

  methods(): MethodDescriptor[];

  templateConfig?(): TemplateConfig;
}

// ============================================================
// Errors
// ============================================================

export class ForestError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ForestError";
  }
}

export class MethodNotFoundError extends ForestError {
  constructor(method: string) {
    super(`unknown method: ${method}`);
    this.name = "MethodNotFoundError";
  }
}

export class DeserializationError extends ForestError {
  constructor(message: string) {
    super(`deserialization error: ${message}`);
    this.name = "DeserializationError";
  }
}

// ============================================================
// Protocol
// ============================================================

interface Payload {
  spec: unknown;
  input?: unknown;
  context?: CallContext;
}

async function readStdin(): Promise<string> {
  const chunks: Uint8Array[] = [];
  const reader = Deno.stdin.readable.getReader();
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }
  return new TextDecoder().decode(
    chunks.reduce((acc, chunk) => {
      const result = new Uint8Array(acc.length + chunk.length);
      result.set(acc);
      result.set(chunk, acc.length);
      return result;
    }, new Uint8Array()),
  );
}

function writeStdout(data: string): void {
  const encoder = new TextEncoder();
  Deno.stdout.writeSync(encoder.encode(data));
}

// ============================================================
// Runtime
// ============================================================

/**
 * Run the component service once, handling a single method invocation.
 *
 * Reads the method name from Deno.args[0], handles meta-methods,
 * then reads JSON payload from stdin and dispatches to the service.
 */
export async function runOnce<S>(service: ComponentService<S>): Promise<void> {
  const args = Deno.args;

  if (args.length < 1) {
    console.error("usage: component <method>");
    Deno.exit(1);
  }

  const method = args[0];

  try {
    // Meta-methods
    if (method === "_meta/describe") {
      const descriptor: ComponentDescriptor = {
        protocol_version: "1.1",
        methods: service.methods(),
      };
      writeStdout(JSON.stringify(descriptor, null, 2));
      return;
    }

    if (method === "_meta/template_config") {
      const config = service.templateConfig?.() ?? {
        skip: [],
        rename: {},
        vars: {},
      };
      writeStdout(JSON.stringify(config, null, 2));
      return;
    }

    // Regular method invocation — read payload from stdin
    const raw = await readStdin();
    if (!raw.trim()) {
      throw new ForestError("no payload received on stdin");
    }

    let payload: Payload;
    try {
      payload = JSON.parse(raw);
    } catch {
      throw new DeserializationError(`invalid JSON payload: ${raw.slice(0, 200)}`);
    }

    const spec = payload.spec as S;
    const input = payload.input ?? {};
    const context: CallContext = payload.context ?? {};

    const result = await service.call(method, spec, input, context);
    writeStdout(JSON.stringify(result, null, 2));
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.error(`error: ${message}`);
    Deno.exit(1);
  }
}
