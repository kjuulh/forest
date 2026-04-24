# Hollow — Handoff Document

## What Is Hollow

Hollow is an isolated agent runtime for Forest that runs untrusted code (Terraform, binaries, CI jobs) inside ephemeral Firecracker microVMs. It sits between forest-server and actual job execution, replacing the current unprotected subprocess spawning with KVM-isolated VMs.

## Current State (Phase 1 — Dev Mode)

The codebase is fully scaffolded, compiles cleanly (`cargo clippy -- -D warnings`), and has passing protocol tests. It runs in **dev mode** where the agent spawns `hollow-guest` as a local subprocess communicating over a Unix socket, instead of a Firecracker microVM over vsock. This lets you test the full flow without KVM.

### Architecture

```
forest-server (scheduler, existing)
    │
    │ gRPC (existing RunnerService protocol)
    ▼
hollow-controller          ← registers as a forest-runner, dispatches to agents
    │
    │ gRPC (HollowAgentService, new proto)
    ▼
hollow-agent               ← manages VM lifecycle per job (Unix socket in dev, Firecracker in prod)
    │
    │ vsock protocol (hollow-vsock crate, Unix socket in dev)
    ▼
hollow-guest               ← minimal binary inside VM, executes the job command
```

### Crates

| Crate | Purpose |
|-------|---------|
| `hollow-grpc-interface` | Generated proto code from `proto/hollow/v1/agent.proto` |
| `hollow-vsock` | Shared wire protocol (framing, message types) used by agent + guest |
| `hollow-guest` | Binary running inside VM — receives job, spawns process, streams logs |
| `hollow-agent` | Runs on pool machine — connects to controller, manages VMs |
| `hollow-controller` | Orchestrator — registers with forest-server, dispatches to agents |

### Key Design Patterns

- **State + *State traits**: Dependencies are constructed via `AgentPoolState`, `JobTrackerState`, `DispatcherState`, `AgentGrpcServerState` extension traits on `State`. Singletons use `OnceLock`.
- **Inner Arc pattern**: `AgentPool` and `JobTracker` are cheaply `Clone` via `inner: Arc<Mutex<...>>`.
- **notmad Components**: Controller runs `MetricsServer` + `AgentGrpcServer` + `Dispatcher` as notmad components. Agent runs `AgentService`. Both handle graceful shutdown via `CancellationToken`.
- **JobTracker**: Bridges the gRPC server (receives agent events) and the dispatcher (holds the forest-server session). Events flow: agent → gRPC server → JobTracker channel → dispatcher → forest-server PushLogs/CompleteRelease.

### What Works End-to-End

1. Controller registers with forest-server as a runner
2. Agent connects to controller, sends registration + heartbeats
3. forest-server scheduler dispatches `WorkAssignment` to controller
4. Controller prefetches all release data (files, annotation, project info)
5. Controller builds `RunJob` with destination-specific command (terraform init + plan/apply)
6. Controller dispatches to agent via gRPC
7. Agent creates Unix socket, spawns `hollow-guest` subprocess
8. Guest connects, receives job definition, executes command, streams logs
9. Logs flow: guest → agent → controller → forest-server `PushLogs`
10. Completion flows: guest → agent → controller → forest-server `CompleteRelease`
11. Agent cleans up guest process + socket

### What's NOT Done Yet

- **Firecracker VM launch**: Agent uses subprocess + Unix socket. Production needs Firecracker API + vsock. The two TODOs in `vm.rs` and `guest/main.rs` mark these transition points.
- **Real vsock**: Guest uses `HOLLOW_VSOCK_PATH` Unix socket env var. Production needs `AF_VSOCK` with CID=2, port=1024.
- **Rootfs image building**: Dockerfiles + pack script exist (`hollow/images/`) but haven't been tested end-to-end.
- **Network isolation**: iptables NAT rules per VM (spec'd in `spec.md` section 7).
- **Event-sourced job state**: Controller currently tracks jobs in-memory only. Production needs event-sourced persistence with application-defined sharding per the spec.
- **Log forwarding completeness**: Logs are forwarded but forest-server integration hasn't been tested end-to-end.

### How to Test Locally

```bash
# Terminal 1: forest-server with in-process execution disabled
mise run develop:hollow    # alias: mise run dh

# Terminal 2: hollow controller
cd hollow && mise run controller

# Terminal 3: hollow agent
cd hollow && mise run agent

# Then trigger a terraform release through forest — it will route to hollow
```

### Key Files

| File | What to know |
|------|-------------|
| `hollow/spec.md` | Full design spec — architecture, isolation, networking, security, API, migration |
| `hollow/proto/hollow/v1/agent.proto` | Controller↔agent gRPC protocol |
| `hollow/crates/hollow-vsock/src/protocol.rs` | Message types for vsock wire protocol |
| `hollow/crates/hollow-vsock/src/transport.rs` | Framed read/write over any AsyncRead/AsyncWrite |
| `hollow/crates/hollow-agent/src/vm.rs` | VM lifecycle — where subprocess mode lives, where Firecracker goes |
| `hollow/crates/hollow-controller/src/dispatcher.rs` | WorkAssignment → RunJob translation, log/completion forwarding |
| `hollow/crates/hollow-controller/src/state.rs` | State + *State trait pattern |
| `hollow/crates/hollow-controller/src/job_tracker.rs` | Channel bridge between gRPC server and dispatcher |

### Resource Constants

`DEFAULT_VCPUS_PER_JOB = 1`, `DEFAULT_MEMORY_MIB_PER_JOB = 1024`, `DEFAULT_TIMEOUT_SECONDS = 1800` — defined in `dispatcher.rs` and shared with `agent/service.rs`. These are used for capacity tracking in heartbeats and dispatch decisions in `agent_pool.rs`.

### Prometheus Metrics (controller, :4051)

- `hollow_jobs_dispatched_total` — counter
- `hollow_jobs_completed_total` — counter
- `hollow_jobs_failed_total` — counter
- `hollow_agents_connected` — gauge
- `hollow_jobs_active` — gauge

### Next Steps

1. **End-to-end test** with actual forest-server + terraform destination
2. **Set up the test VM** (2 vCPU, 16 GB RAM, KVM-enabled) and deploy agent
3. **Implement Firecracker launch** in `vm.rs` — replace subprocess with Firecracker API
4. **Build rootfs images** using the image pipeline in `hollow/images/`
5. **Wire up real vsock** in guest binary
6. **Add network isolation** (per-VM tap + iptables NAT)
