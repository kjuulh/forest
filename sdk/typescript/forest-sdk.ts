// Forest SDK for Deno/TypeScript
//
// Implements the Forest component protocol v2.
// Components are invoked as subprocesses with JSON lines over stdin/stdout.
//
// Protocol v2 messages (all JSON lines with a "type" field):
//   Runtime → Component:  {"type":"invoke","method":"...","spec":{},"input":{},"context":{}}
//   Component → Runtime:  {"type":"call","id":"...","component":"...","method":"...","spec":{},"input":{}}
//   Runtime → Component:  {"type":"call_result","id":"...","result":{}}
//   Component → Runtime:  {"type":"return","result":{}}
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
// Protocol v2 — streaming JSON lines
// ============================================================

interface InvokeMessage {
  type: "invoke";
  method: string;
  spec: unknown;
  input: unknown;
  context: CallContext;
}

interface CallMessage {
  type: "call";
  id: string;
  component: string;
  method: string;
  spec: unknown;
  input: unknown;
  context?: CallContext;
}

interface CallResultMessage {
  type: "call_result";
  id: string;
  result: unknown;
}

interface ReturnMessage {
  type: "return";
  result: unknown;
}

type RuntimeMessage = InvokeMessage | CallResultMessage;

// Shared line reader for stdin — components read multiple messages during their lifetime.
let stdinReader: ReadableStreamDefaultReader<string> | null = null;

function getStdinLineReader(): ReadableStreamDefaultReader<string> {
  if (!stdinReader) {
    stdinReader = Deno.stdin.readable
      .pipeThrough(new TextDecoderStream())
      .pipeThrough(new TextLineStream())
      .getReader();
  }
  return stdinReader;
}

// TextLineStream splits a text stream into individual lines.
class TextLineStream extends TransformStream<string, string> {
  #buf = "";
  constructor() {
    super({
      transform: (chunk, controller) => {
        this.#buf += chunk;
        const lines = this.#buf.split("\n");
        this.#buf = lines.pop() ?? "";
        for (const line of lines) {
          if (line.length > 0) {
            controller.enqueue(line);
          }
        }
      },
      flush: (controller) => {
        if (this.#buf.length > 0) {
          controller.enqueue(this.#buf);
        }
      },
    });
  }
}

async function readMessage(): Promise<RuntimeMessage> {
  const reader = getStdinLineReader();
  const { done, value } = await reader.read();
  if (done || !value) {
    throw new ForestError("stdin closed unexpectedly");
  }
  try {
    return JSON.parse(value);
  } catch {
    throw new DeserializationError(`invalid JSON line: ${value.slice(0, 200)}`);
  }
}

function writeMessage(msg: CallMessage | ReturnMessage): void {
  const line = JSON.stringify(msg) + "\n";
  Deno.stdout.writeSync(new TextEncoder().encode(line));
}

// ============================================================
// callComponent — inter-component RPC
// ============================================================

let callIdCounter = 0;
let currentContext: CallContext = {};

/**
 * Call another component's method via the forest runtime.
 *
 * Writes a "call" message to stdout, waits for the runtime to send
 * a "call_result" back on stdin, and returns the result.
 */
export async function callComponent<R = unknown>(
  component: string,
  method: string,
  spec: unknown,
  input: unknown,
): Promise<R> {
  const id = String(++callIdCounter);

  writeMessage({
    type: "call",
    id,
    component,
    method,
    spec,
    input,
    context: currentContext,
  });

  const response = await readMessage();
  if (response.type !== "call_result") {
    throw new ForestError(`expected call_result, got ${response.type}`);
  }
  if ((response as CallResultMessage).id !== id) {
    throw new ForestError(`call_result id mismatch: expected ${id}, got ${(response as CallResultMessage).id}`);
  }

  return (response as CallResultMessage).result as R;
}

// ============================================================
// Legacy helpers (for meta-methods that still use single-shot)
// ============================================================

async function readStdinAll(): Promise<string> {
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
  Deno.stdout.writeSync(new TextEncoder().encode(data));
}

// ============================================================
// Runtime
// ============================================================

/**
 * Run the component service once, handling a single method invocation.
 *
 * Protocol v2: reads an {"type":"invoke",...} message from stdin,
 * dispatches to the service, writes {"type":"return",...} to stdout.
 * The component may call other components during execution via callComponent().
 *
 * Meta-methods (_meta/describe, _meta/template_config) still use the
 * legacy single-shot protocol for compatibility with forest build.
 */
export async function runOnce<S>(service: ComponentService<S>): Promise<void> {
  const args = Deno.args;

  if (args.length < 1) {
    console.error("usage: component <method>");
    Deno.exit(1);
  }

  const method = args[0];

  try {
    // Meta-methods use legacy single-shot protocol
    if (method === "_meta/describe") {
      const descriptor: ComponentDescriptor = {
        protocol_version: "2.0",
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

    // Protocol v2: read invoke message
    const msg = await readMessage();
    if (msg.type !== "invoke") {
      throw new ForestError(`expected invoke message, got ${msg.type}`);
    }

    const invoke = msg as InvokeMessage;
    const spec = invoke.spec as S;
    const input = invoke.input ?? {};
    const context: CallContext = invoke.context ?? {};

    // Store context so callComponent can forward it to sub-components
    currentContext = context;

    const result = await service.call(invoke.method, spec, input, context);

    writeMessage({ type: "return", result });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    console.error(`error: ${message}`);
    Deno.exit(1);
  }
}
