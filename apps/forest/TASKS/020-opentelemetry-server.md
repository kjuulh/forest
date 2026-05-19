# 020: OpenTelemetry for forest-server and forage-server

> Status: **IMPLEMENTING.** Spec was originally a bespoke design;
> revised to drop in Understory's existing `canopy-otel` library
> instead.

## Intent

Both forest-server and forage-server should be able to opt into
exporting traces, logs, and metrics to a remote OTLP collector for
production observability. Currently:

- **forest-server** only sets up `tracing_subscriber::fmt` (stdout).
  No OTEL.
- **forage-server** has a bespoke OTEL setup (gRPC traces only, no
  shutdown guard, no logs/metrics) that re-implements 80% of what
  canopy-otel already does.

Replace both with [`canopy-otel`](https://github.com/understory-io/canopy-util-rs/tree/main/crates/canopy-otel),
the existing Understory library used by `canopy-data-gateway` and
related services. One mental model across the org.

## What canopy-otel gives us

Pulled directly from the lib's `init()` (`crates/canopy-otel/src/lib.rs`):

- **Toggle:** presence of `OTEL_SERVICE_NAME` env var. Set ⇒ full OTLP
  pipeline. Unset ⇒ pretty fmt-only fallback. Identical to today's
  no-OTEL behavior.
- **Signals:** traces, logs, metrics — all via OTLP HTTP (port 4318).
- **Resource:** `service.name` from `OTEL_SERVICE_NAME` (or crate
  name fallback) + `service.instance.id` (UUIDv7 per process).
  Operators pass anything else via `OTEL_RESOURCE_ATTRIBUTES`.
- **Sampler:** `parentbased_always_on` with `TraceIdRatioBased(1.0)`.
  Operator-overridable via `OTEL_TRACES_SAMPLER` / `OTEL_TRACES_SAMPLER_ARG`.
- **Subscriber:** `Registry::default().with(env_filter).with(otel_log_bridge).with(fmt).with(otel_trace_layer).with(metrics_layer)` —
  the canonical composition.
- **Shutdown:** RAII `DropGuard` returned from `init()`. Drop runs
  `shutdown()` on tracer + meter + logger. Hold it in `main()` for
  the lifetime of the process.

## Implementation

### Cargo.toml

Add to both apps' Cargo.toml (not workspace-level — forest doesn't
need this in the workspace, forage already has direct OTEL deps that
canopy-otel supersedes):

```toml
canopy-otel = { git = "https://github.com/understory-io/canopy-util-rs", version = "0.1.0" }
```

### forest-server `main.rs`

Replaces today's `match log_level { "json" / "short" / _ }` ladder
with `canopy_otel::init()`. We lose the `LOG_LEVEL=json|short`
formatting modes — canopy-otel always uses pretty fmt in fallback
and adds the OTEL bridge in OTLP mode. If we later want JSON
fmt, that's a canopy-otel patch (or a forest-specific layer wrapped
around its `init()`), not forest's problem to solve in-tree.

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let _otel = canopy_otel::init();
    forest_server::cli::execute().await?;
    Ok(())
}
```

The `_otel` binding holds the guard until end of `main`. Drop runs
the flush.

### forage-server `main.rs`

Replaces the existing `init_telemetry()` function (39 lines of bespoke
OTLP wiring). Same one-line `canopy_otel::init()`. Forage's existing
OTEL deps in `Cargo.toml` stay — they're transitively required by
canopy-otel anyway and removing them is a no-op cleanup we can do
separately.

### Env vars (operator surface)

Standard OTEL — same set both apps respect:

| Var | Effect |
|---|---|
| `OTEL_SERVICE_NAME` | **The toggle.** Unset ⇒ fmt-only. Set ⇒ OTLP export. Recommend `forest-server` and `forage-server`. |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP/HTTP endpoint (e.g. `http://otel-collector:4318`). Defaults to `http://localhost:4318` per OTEL spec. |
| `OTEL_EXPORTER_OTLP_HEADERS` | Auth headers (`api-key=...`). |
| `OTEL_RESOURCE_ATTRIBUTES` | Extra attrs (`deployment.environment=prod,service.version=1.2.3`). |
| `OTEL_TRACES_SAMPLER` / `OTEL_TRACES_SAMPLER_ARG` | Sampling. |
| `RUST_LOG` | Already used. Honored by canopy-otel's EnvFilter. |

Documented in each app's `.env.example`.

## Out of scope (intentional)

- **gRPC trace-context propagation.** canopy-otel doesn't expose
  tonic interceptors and canopy-data-gateway doesn't use them either.
  forage→forest end-to-end correlation needs interceptors on both
  sides; tracked as a separate task. (The per-process traces still
  land correctly in the collector — they just don't connect across
  the gRPC hop yet.)
- **`LOG_LEVEL=json|short` formatting modes** in forest-server.
  canopy-otel always uses pretty fmt. If we need JSON in production
  logs, canopy-otel needs a feature flag — separate ask.
- **Metrics or log assertions in tests.** canopy-otel falls back to
  fmt-only when `OTEL_SERVICE_NAME` is unset; tests don't set it,
  so OTEL never spins up in CI. No special test handling needed.

## Verification

- `cargo build` clean for both apps.
- Existing test suites green.
- Manual smoke test: start a local collector
  (`docker run --rm -p 4318:4318 otel/opentelemetry-collector-contrib`),
  set `OTEL_SERVICE_NAME=forest-server` + `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318`,
  hit a gRPC endpoint, observe spans in collector stdout.
- Without `OTEL_SERVICE_NAME`, both binaries log to stdout exactly
  as today's pretty fmt output (modulo canopy-otel's hard-coded
  filter directives `notmad=debug`, `nodrift=debug`).

## Resolved decisions

1. **Use canopy-otel, don't roll our own.** Same library as the rest
   of the org's Rust services. One mental model.
2. **Toggle on `OTEL_SERVICE_NAME` presence**, not
   `OTEL_EXPORTER_OTLP_ENDPOINT`. This is canopy-otel's choice; we
   inherit it. Operators expecting "set the endpoint to enable" will
   need to also set the service name — documented in `.env.example`.
3. **HTTP/4318, not gRPC/4317.** canopy-otel's choice. Forage's old
   bespoke setup used gRPC; switching to HTTP unifies it with the
   rest of the org. Operators pointing at gRPC-only collectors need
   to expose HTTP too (most modern collectors expose both).
4. **No JSON log formatting in this task.** forest-server's existing
   `LOG_LEVEL=json` mode is dropped. Acceptable because no production
   consumer relies on the JSON mode today; if a log shipper does
   need it later, the right fix is a canopy-otel feature flag.
