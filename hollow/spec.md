# Hollow: Isolated Agent Runtime for Forest

> Spec version: 0.1.0 — 2026-04-24

## Table of Contents

1. [Problem Statement](#problem-statement)
2. [Industry Research](#industry-research)
3. [Architecture Overview](#architecture-overview)
4. [Isolation Strategy](#isolation-strategy)
5. [Machine Pool Management](#machine-pool-management)
6. [Job Lifecycle](#job-lifecycle)
7. [Network Architecture](#network-architecture)
8. [Security Model](#security-model)
9. [gRPC API Design](#grpc-api-design)
10. [Observability](#observability)
11. [Failure Handling](#failure-handling)
12. [Integration with Forest](#integration-with-forest)
13. [Migration Path](#migration-path)
14. [Open Questions](#open-questions)

---

## Problem Statement

Forest runners execute untrusted code: Terraform plans with arbitrary providers, custom binaries, GitOps reconciliation commands. Today, these run as direct subprocess spawns on the runner host (`tokio::process::Command` in `crates/forest-runner/src/destinations/fluxv1.rs` and `crates/forest-server/src/destinations/terraformv1.rs`) with **zero isolation** — they share the runner's filesystem, network stack, process namespace, and credentials.

This is untenable for multi-tenant operation. A malicious Terraform provider can:

- Read secrets from the runner's environment or filesystem
- Exfiltrate data over the network
- Attack other tenants' workloads on the same machine
- Persist backdoors for future runs
- Pivot to internal services (forest-server, databases)

**Hollow** is an isolated execution runtime that runs each job inside an ephemeral Firecracker microVM with NAT-only egress, integrated with forest-server via gRPC.

---

## Industry Research

### GitHub Actions (Hosted Runners)

**How it works:** Each job gets a fresh ephemeral VM on Microsoft Azure. The VM is provisioned from a pre-built image, the job executes, and the VM is destroyed. No state persists between runs.

**Isolation model:**
- Full VM isolation per job (Azure hypervisor)
- Jobs within the same workflow CAN interact (shared filesystem, Docker socket)
- Network: outbound NAT, inbound ICMP blocked, crypto-mining domains blocklisted
- Self-hosted runners have NO ephemeral guarantees — GitHub warns they should not be used for public repos
- JIT (Just-In-Time) runners mitigate self-hosted risk: ephemeral, auto-deregister after one job

**Key numbers:** ~30-60s VM boot time, $0.008/minute pricing

**Takeaways:**
- Fresh VM per job is the gold standard for hosted CI
- Self-hosted without ephemeral guarantees is a known security gap
- JIT runner pattern is relevant for our agent pool

### Depot.dev

**How it works:** Webhook-driven architecture. GitHub sends `workflow_job` webhook events to Depot's control plane, which assigns a fresh EC2 instance from a pre-provisioned standby pool. Instances are never reused across jobs.

**Key innovation — EBS warming trick:**
1. Pre-boot instances on cheap `t3.large` to warm EBS volumes (stream S3 data blocks)
2. Stop the instance (pay only for EBS storage)
3. Resize to target instance type and restart when a job arrives
4. Kernel+systemd startup drops from ~10s to <400ms on warmed volumes

**Result:** 5-second boot on standard EC2 instances (vs 40s cold).

**Isolation:** Single-tenant ephemeral EC2. Per-build mTLS certificates. Cache encrypted at rest (AES-256-GCM). Cache scoped by repository.

**Takeaways:**
- Pre-warming is a pragmatic bridge between VM boot times and microVM speeds
- Per-build scoped credentials are essential
- Never reuse execution environments across jobs

### Firecracker MicroVMs

**What it is:** A user-space VMM leveraging Linux KVM, developed by AWS for Lambda and Fargate. Written in Rust.

**Performance:**
- Boot time: <125ms
- Memory overhead: <5 MiB per microVM
- Creation rate: up to 150 microVMs/second/host
- Supports thousands of isolated instances per machine

**Minimal device model (5 devices only):** virtio-net, virtio-block, virtio-vsock, serial console, minimal keyboard controller. This extreme minimalism is a deliberate security choice — fewer devices means fewer attack vectors.

**Security layers (defense in depth):**
1. **KVM hardware virtualization** — CPU ring separation, EPT/NPT memory isolation
2. **Jailer** — chroot, Linux namespaces, cgroups, privilege dropping
3. **Seccomp-BPF** — ~40 allowed syscalls with strict argument filters
4. **Rust implementation** — memory-safe VMM, no buffer overflows
5. **Static linking** — minimal runtime dependencies

**Users:** AWS Lambda (billions of invocations), AWS Fargate, Fly.io, Koyeb.

**Takeaways:**
- Gold standard for ephemeral untrusted workload isolation
- Sub-second boot makes it viable for on-demand VM-per-job
- Rust implementation aligns with Forest's stack
- vsock provides a clean host↔guest communication channel without network exposure

### gVisor

**What it is:** An application kernel written in Go that runs in user space, intercepting all application syscalls and implementing them independently rather than passing them to the host kernel.

**Components:** Sentry (core kernel), Gofer (filesystem via 9P), runsc (OCI runtime).

**Syscall interception:**
- KVM platform: uses hardware virt extensions, best on bare metal
- Systrap (default since mid-2023): uses `SECCOMP_RET_TRAP`, works in VMs without hardware virt
- Ptrace (deprecated): highest overhead

**Security model:** No direct syscall pass-through. Go provides memory safety. Continuous fuzzing. Protects against kernel vulnerabilities triggered through syscalls (e.g., Dirty Cow). Does NOT protect against hardware side channels.

**Users:** Google Cloud Run (1st gen), GKE Sandbox, Google Cloud Functions.

**Takeaways:**
- Good for container-compatible isolation without hardware virt
- Higher per-syscall overhead than a real kernel (problematic for I/O-heavy Terraform)
- Incomplete syscall compatibility — some Terraform providers use unsupported syscalls
- Better suited as a secondary isolation layer, not primary

### Kata Containers

**What it is:** Runs each container/pod inside a lightweight VM, combining hardware virtualization isolation with standard container UX (OCI, CRI, Kubernetes).

**Key optimization:** DAX maps guest image directly into VM memory via mmap (zero-copy, demand-paged). virtio-fs shares container rootfs from host.

**Hypervisor backends:** QEMU, Cloud Hypervisor, or Firecracker.

**Takeaways:**
- VM-level isolation with container UX
- More complex than bare Firecracker for our use case
- Relevant if we want Kubernetes-native integration later

### Fly.io Machines API

**Architecture:** Firecracker microVMs on bare-metal servers globally, connected via WireGuard mesh.

**Key design decisions:**
- Machines are **closed to the public internet by default**
- 6PN (IPv6 Private Network) via WireGuard mesh within organization
- Organization-level strict isolation — platform refuses to forward packets between different 6PNs
- REST API with 17 endpoints. Only required param: `config.image`
- Market-based scheduling — requests are "bids for resources," no pending queue
- Auto-start/stop for idle machines

**Takeaways:**
- Default-closed networking is the right model
- Organization-level network isolation prevents lateral movement
- Simple API design with sensible defaults
- vsock + private network for internal communication

### Buildkite

**Architecture:** Hybrid-SaaS. Buildkite manages orchestration; customers manage agents and infrastructure.

**Agent model:** Pull-based — agents poll Buildkite's API over HTTPS (no inbound firewall ports needed). Job routing via queue tags.

**Isolation:** Customer's responsibility for self-hosted. Docker-based isolation available but "providing builds with a Docker socket gives them root system access."

**Takeaways:**
- Pull-based agent registration is operationally simpler (matches our existing `RegisterRunner` pattern)
- Isolation must be built in, not bolted on
- Queue-based routing with tags maps to our capability matching

### Hetzner Cloud (Infrastructure)

**Relevant for our machine pool.**

**Server types:** Shared CPU (CX), Dedicated CPU (CCX), ARM (CAX). Data centers in Germany, Finland, Singapore, USA.

**Pricing:** ~EUR 0.006/hour for CX22 — roughly **75x cheaper** than GitHub Actions. Hourly billing.

**CI runner ecosystem:**
- TestFlows GitHub Hetzner Runners: auto-provision/destroy VMs per job
- hcloud-github-runner: on-demand self-hosted runners
- Full REST API, Terraform provider, Go/Python libraries

**Dedicated servers (AX-series):**
- Bare metal with KVM support — required for Firecracker
- AX41: AMD Ryzen 5 3600, 64GB RAM, 2x512GB NVMe — ~EUR 46/month
- AX52: AMD Ryzen 7 5800X, 64GB RAM, 2x1TB NVMe — ~EUR 60/month
- AX102: AMD Ryzen 9 5950X, 128GB RAM, 2x3.84TB NVMe — ~EUR 130/month

**Takeaways:**
- Dedicated servers provide bare-metal KVM access for Firecracker
- 10-75x cheaper than cloud VMs or hosted CI
- Good API for future autoscaling of pool machines
- Hourly billing is wasteful for short jobs — dedicated servers with always-on agents are more cost-effective

### Comparison Matrix

| System | Isolation Tech | Boot Time | Memory Overhead | Multi-tenant Safe | Ephemeral |
|--------|---------------|-----------|-----------------|-------------------|-----------|
| GitHub Actions (hosted) | Azure VM | ~30-60s | Full VM | Yes | Yes |
| Depot | EC2 VM (warmed) | ~5s | Full VM | Yes | Yes |
| Firecracker | microVM (KVM) | <125ms | <5 MiB | Yes | Yes |
| gVisor | User-space kernel | ~instant | Process-like | Yes | N/A |
| Kata Containers | Lightweight VM | ~seconds | Moderate (DAX) | Yes | Yes |
| Fly.io Machines | Firecracker | Sub-second | <5 MiB | Yes | Yes |
| Hetzner (DIY) | Standard VM | ~30-60s | Full VM | Yes | Yes |

---

## Architecture Overview

### Component Topology

```
forest-server (scheduler)
       │
       │ gRPC (existing RunnerService protocol)
       │
       ▼
hollow-controller
       │  Orchestrates job dispatch, pool management,
       │  log forwarding, completion reporting.
       │  Acts as a standard forest-runner from
       │  forest-server's perspective.
       │
       │ gRPC (HollowAgentService — internal)
       │
       ├──────────────────┬──────────────────┐
       ▼                  ▼                  ▼
hollow-agent          hollow-agent       hollow-agent
[bare-metal host]     [bare-metal host]  [bare-metal host]
       │                  │                  │
       │ Firecracker API (Unix socket)
       │ + AF_VSOCK (guest↔host)
       │
       ├─── microVM (job A) ─── hollow-guest
       ├─── microVM (job B) ─── hollow-guest
       └─── microVM (job C) ─── hollow-guest
                │
                │ NAT-only egress
                ▼
           Internet (terraform providers, cloud APIs)
```

### Components

#### hollow-controller (`crates/hollow`)

The brain of the system. Responsibilities:

- **Registers as a forest-runner** with forest-server via the existing `RunnerService.RegisterRunner` bidirectional stream. From forest-server's perspective, it IS a runner with declared capabilities and max_concurrent capacity.
- **Receives `WorkAssignment`** messages from forest-server's scheduler, exactly like the current `forest-runner`.
- **Manages the agent pool**: tracks connected agents, their capacity, health status.
- **Schedules jobs to agents**: best-fit algorithm based on available resources.
- **Forwards logs**: receives log streams from agents, pushes to forest-server via `PushLogs`.
- **Reports completion**: calls `CompleteRelease` on forest-server when jobs finish.
- **Fetches release data**: calls `GetReleaseFiles`, `GetSpecFiles`, `GetReleaseAnnotation`, `GetProjectInfo` and forwards to agents.
- **Exposes `HollowService`** gRPC (optional): for future direct job submission with richer controls (resource requirements, network policies, egress filtering).

#### hollow-agent (`crates/hollow-agent`)

Runs on each bare-metal pool machine. Responsibilities:

- **Connects to hollow-controller** via persistent bidirectional gRPC stream (`HollowAgentService.RegisterAgent`).
- **Reports capacity**: total/available vCPUs, memory, disk. Periodic heartbeats.
- **Manages Firecracker microVMs**: creates, monitors, destroys VMs for each job.
- **Manages host networking**: creates/destroys tap interfaces, iptables NAT rules per VM.
- **Manages rootfs images**: keeps base images on local SSD, creates copy-on-write overlays per job.
- **Bridges vsock↔gRPC**: receives logs and completion status from guest over vsock, forwards to controller.
- **Enforces timeouts**: kills VMs that exceed their job timeout.
- **Cleans up**: destroys all VM resources (rootfs overlay, tap interface, cgroup, Firecracker process) after each job.

#### hollow-guest (`crates/hollow-guest`)

Minimal static binary (`x86_64-unknown-linux-musl`) that runs inside each microVM. Responsibilities:

- **Runs as PID 1** (or under a minimal init like `tini`).
- **Connects to host agent** over vsock (AF_VSOCK, CID=3).
- **Receives job definition**: command, environment variables, files.
- **Writes files** to working directory.
- **Spawns job process** with configured environment.
- **Streams stdout/stderr** back over vsock in real-time.
- **Reports exit code** and optional output artifacts on completion.
- **Sends heartbeats** over vsock so the host agent can detect guest hangs.

### Why Three Tiers?

The existing `forest-runner` is a single binary that connects to `forest-server`, receives work, and executes it directly. Hollow preserves this integration at the controller level while adding isolation below:

1. **forest-server requires zero changes** — the controller IS a runner from its perspective.
2. The scheduler (`crates/forest-server/src/scheduler.rs`) continues to call `runner_manager.try_assign()` unchanged.
3. The controller translates `WorkAssignment` into a richer `HollowJob` and dispatches to an agent.
4. The agent launches a Firecracker microVM, injects the job, manages the lifecycle.

---

## Isolation Strategy

### Primary: Firecracker MicroVMs

Each job runs inside its own Firecracker microVM with a dedicated Linux kernel. This provides:

- **Hardware-level isolation via KVM**: CPU ring separation, Extended Page Tables (EPT/NPT) for memory isolation, interrupt isolation. A guest cannot read another guest's memory even with kernel exploits.
- **Minimal attack surface**: Firecracker implements only 5 device types. No USB, PCI passthrough, GPU, or display. Fewer devices = fewer vulnerabilities.
- **Ephemeral by default**: VMs boot from a fresh rootfs overlay and are destroyed after the job. No persistent state between runs.

**Why not gVisor?** gVisor intercepts syscalls in userspace, adding overhead for I/O-heavy workloads (Terraform does significant filesystem and network I/O). It also has incomplete syscall compatibility — some Terraform providers use syscalls gVisor doesn't emulate. Firecracker delegates to a real Linux kernel inside the VM.

**Why not containers + seccomp?** Container namespaces share the host kernel. A kernel exploit breaks out of all containers on the host. For truly untrusted code (arbitrary Terraform providers, user binaries), KVM isolation is materially stronger.

### Defense-in-Depth Layers

Even with Firecracker, we apply layered security:

| Layer | Mechanism | Purpose |
|-------|-----------|---------|
| 1 | KVM hardware virtualization | Memory/CPU isolation between VMs |
| 2 | Firecracker jailer | chroot, namespaces, capabilities dropped, unprivileged UID per VM |
| 3 | Seccomp-BPF on Firecracker process | ~40 allowed host syscalls with argument filters |
| 4 | cgroup v2 per VM | CPU and memory limits, prevent host starvation |
| 5 | Ephemeral rootfs (CoW overlay) | No persistent state, clean environment per job |
| 6 | Per-VM tap + iptables | Network isolation, NAT-only egress, no lateral movement |
| 7 | vsock for host↔guest | Guest never touches host network stack |
| 8 | Scoped credentials | Secrets injected as env vars over vsock, never on disk |

### Docker / Container Workloads Inside VMs (Future CI Capability)

Docker is NOT a current requirement, but we validate the capability early to ensure the architecture supports future CI services.

Firecracker guests run a full Linux kernel with cgroups and namespaces — everything Docker needs. Standard `docker build` and `docker run` work inside the microVM because:

- Docker uses kernel namespaces and cgroups, NOT KVM. No nested virtualization required.
- `--privileged` containers inside the VM are scoped to the **guest kernel**, not the host.
- The KVM boundary remains intact regardless of what the guest does with its own kernel.

**Validation plan:** Build a `docker-test` rootfs image (see VM Image Build System) with bare Docker installed. Run a smoke test that does `docker run hello-world` inside a Firecracker VM. This validates the isolation model supports container workloads without making Docker a first-class feature yet.

**What does NOT work:**
- Nested KVM (VMs inside VMs) — Firecracker does not expose `/dev/kvm` to guests

### Host Machine Requirements

- Linux kernel >= 5.10 with KVM enabled (`/dev/kvm`)
- Hetzner AX-series dedicated servers (bare metal, KVM out of the box)
- NO nested virtualization required — agents run directly on bare metal

---

## Machine Pool Management

### Pool Model

```
hollow-controller
  │
  ├── Pool: "default" (any org)
  │     ├── Agent: ax41-01  (8 vCPU, 64GB, Falkenstein)
  │     ├── Agent: ax41-02  (8 vCPU, 64GB, Falkenstein)
  │     └── Agent: ax41-03  (8 vCPU, 64GB, Helsinki)
  │
  └── Pool: "org-acme" (dedicated to org "acme")
        └── Agent: ax52-01  (16 vCPU, 128GB, Falkenstein)
```

Pools provide:
- **Default pool**: shared across all organisations (cost-efficient)
- **Dedicated pools**: per-org for compliance or performance isolation (optional)
- **Pool affinity**: jobs routed to org-dedicated pool first, fall back to default

### Agent Registration

Each `hollow-agent` connects to `hollow-controller` via bidirectional gRPC stream. Reports:

**On registration:**
- Hostname, datacenter/region
- Total vCPUs, total memory, total disk
- Kernel version, Firecracker version
- Pool membership

**On heartbeat (every 10s):**
- Active VM count
- Available vCPUs, memory, disk
- CPU load average (1m, 5m, 15m)
- Network throughput

### Capacity Planning

A single Hetzner AX41 (6-core/12-thread, 64GB RAM) can run:
- ~6 concurrent jobs at 2 vCPU + 1 GiB each (default)
- ~3 concurrent jobs at 4 vCPU + 4 GiB each (heavy Terraform)
- ~32 concurrent lightweight jobs at 1 vCPU + 512 MiB each

Firecracker overhead is <5 MiB per VM, so memory is almost entirely available to guests.

### VM Image Build System

Each destination type (and version) gets a purpose-built rootfs image. Images are built with a reproducible pipeline and versioned alongside the destination code.

**Image structure:**

```
hollow/images/
  ├── base/                  # Shared base layer: minimal Linux, hollow-guest, busybox
  │   └── build.sh
  ├── terraform-v1/          # Terraform destination image
  │   └── build.sh           # base + terraform binary + common providers
  ├── fluxv1/                # Flux/GitOps destination image
  │   └── build.sh           # base + git + kustomize + flux CLI
  ├── docker-test/           # Validation image: base + Docker/podman (for CI capability testing)
  │   └── build.sh
  └── kernel/
      └── build.sh           # Minimal kernel config, compiled vmlinux
```

**Build approach:**
1. Start from a minimal Alpine/Ubuntu rootfs (debootstrap or alpine-minirootfs)
2. Install destination-specific tooling
3. Copy `hollow-guest` static binary to `/usr/local/bin/hollow-guest`
4. Create ext4 filesystem image
5. Output: `{destination}-{version}.ext4` + `vmlinux-{version}`

**Image versioning:** Upstream Forest destinations track only the major (breaking) version (e.g., `forest/terraform@1`). Hollow expands this with full semver for its own image lifecycle: `{destination_type}-{major}.{minor}.{patch}` (e.g., `terraform-1.3.0`). Minor/patch versions allow us to ship image improvements (new tool versions, security patches, performance fixes) without bumping the upstream breaking version.

Matching is Go-module-style: a `WorkAssignment` for `forest/terraform@1` resolves to the latest `terraform-1.x.y` image available on the agent. The agent reports available images (with full semver) on registration; the controller picks the highest compatible version.

**Image distribution:** For Phase 1 (single local machine), images live on disk. Future: push to S3/MinIO, agents pull on startup or on-demand with local cache.

### Local Development Machine

Phase 1 runs on a single local machine (2 vCPU, 16 GB RAM) with the controller and agent co-located:

```
local machine (2 vCPU, 16 GB RAM, KVM)
  ├── hollow-controller (process)
  ├── hollow-agent (process)
  └── microVMs (1 at a time, 1 vCPU + 2-4 GiB)
```

**Constraints:**
- 1 concurrent job max (controller's `max_concurrent: 1`)
- Jobs get 1 vCPU + up to 4 GiB RAM
- Boot will be slower than dedicated hardware but functional
- Controller and agent communicate over localhost gRPC (no network latency)

This is sufficient for end-to-end validation. Production will move to dedicated Hetzner bare-metal servers.

### Warm Pool Strategy

To achieve fast job starts:

1. **Pre-built images on local disk**: Each agent keeps rootfs images on local storage, built from the image pipeline above
2. **Pre-built kernel**: `vmlinux` on local disk
3. **Copy-on-write overlays**: Each job gets a CoW overlay of the base image (device-mapper thin provisioning or simple file copy for small images). Creation is <50ms.

**Future: Snapshot-based fast boot**
- Firecracker supports VM memory snapshots
- Boot a "template VM" that completes kernel init + loads hollow-guest + waits for work
- Subsequent jobs restore from snapshot in <50ms (this is how Lambda achieves sub-100ms cold starts)

### Autoscaling (Future)

Phase 1 is a single local machine. Future production adds:

- Controller tracks job queue depth and average wait time
- When queue depth > threshold for > 60s, provision new Hetzner server via API
- When agent idle for > 30 minutes with excess pool capacity, drain and deprovision
- Minimum pool size configurable per pool

---

## Job Lifecycle

### State Machine

```
PENDING ──► SCHEDULING ──► BOOTING ──► RUNNING ──► COMPLETING ──► COMPLETED
                │              │           │                         │
                ▼              ▼           ▼                         ▼
             FAILED         FAILED     TIMED_OUT                  FAILED
          (no capacity)  (boot fail)   CANCELLED
```

### Detailed Flow

#### 1. Job Submission

forest-server's scheduler picks up a QUEUED release, finds the hollow-controller via `RunnerManager.try_assign()`, and sends a `WorkAssignment` on the bidirectional stream.

The controller receives the assignment with:
- `release_token` (scoped auth for data fetching)
- `release_id`, `artifact_id`, `destination_id`
- `DestinationInfo` (name, environment, metadata, type)
- `ReleaseMode` (DEPLOY or PLAN)

#### 2. Data Prefetch

The controller fetches all data needed by the job from forest-server (using the release token):

```
GetReleaseFiles(release_token)  → deployment files
GetSpecFiles(release_token)     → spec files
GetReleaseAnnotation(release_token) → metadata
GetProjectInfo(release_token)   → org + project name
```

This happens on the controller, NOT inside the VM. The guest never contacts forest-server.

#### 3. Scheduling

The controller selects an agent using best-fit:
1. Filter agents with sufficient available resources (vCPUs, memory)
2. If org has a dedicated pool, prefer agents in that pool
3. Among matching agents, pick the one with most spare capacity
4. If no agent has capacity, queue the job (with configurable max queue time)

#### 4. Boot

The selected agent receives `RunJob` and creates a Firecracker VM:

```
1. Allocate tap interface: tap-{vm_id}
2. Configure iptables NAT rules for the tap
3. Create CoW overlay of base rootfs
4. Write Firecracker config JSON:
   - kernel: /opt/hollow/vmlinux-5.10
   - rootfs: /tmp/hollow/{vm_id}/overlay.ext4
   - vcpus: 2, memory: 1024 MiB
   - network: tap-{vm_id}, guest MAC
   - vsock: CID={vm_id_num}, uds_path=/tmp/hollow/{vm_id}/vsock.sock
5. Start Firecracker via jailer:
   jailer --id {vm_id} --exec-file /opt/hollow/firecracker \
     --uid {per_vm_uid} --gid {per_vm_gid} \
     --chroot-base-dir /tmp/hollow/jailer
6. Configure VM via Firecracker API (PUT /machine-config, PUT /boot-source, etc.)
7. Start VM (PUT /actions {"action_type": "InstanceStart"})
8. Wait for hollow-guest to connect on vsock (timeout: 10s)
```

#### 5. Artifact Injection

Once the guest agent signals readiness over vsock, the host agent pushes:

- Job metadata: destination config, environment name, release mode
- Environment variables: destination metadata (TF_VAR_*, cloud credentials), injected as key-value pairs
- Deployment files: rendered manifests / terraform configs
- Spec files: original source configs

All transferred over vsock — never touches the network.

#### 6. Execution

The guest agent:

```
1. Creates /work directory
2. Writes deployment files to /work/
3. Sets environment variables from job metadata
4. Spawns job process:
   - Terraform: terraform init && terraform plan/apply
   - Flux: kustomize build && git commit && flux reconcile
   - Custom: arbitrary binary execution
5. Pipes stdout/stderr to vsock log stream
6. Sends heartbeat every 5s over vsock
```

#### 7. Completion

The guest reports exit code over vsock. The host agent:

```
1. Captures plan output (for terraform plan mode)
2. Sends SIGTERM to Firecracker process
3. Waits 5s, then SIGKILL if needed
4. Removes rootfs overlay
5. Removes tap interface + iptables rules
6. Removes cgroup
7. Reports outcome to controller
```

The controller:

```
1. Calls CompleteRelease on forest-server:
   - outcome: SUCCESS or FAILURE
   - error_message (on failure)
   - plan_output (for plan mode)
2. Releases the agent's reserved capacity
```

#### 8. Cleanup

All VM resources are destroyed. Nothing remains on the host from the job:
- Rootfs overlay: deleted
- Tap interface: removed
- iptables rules: removed
- cgroup: removed
- Firecracker process: killed
- vsock socket: removed

### Timeouts

| Timeout | Default | Configurable | Enforced By |
|---------|---------|-------------|-------------|
| VM boot | 30s | No | Agent |
| Guest readiness | 10s | No | Agent |
| Job execution | 30 minutes | Per destination | Agent |
| Max job execution | 2 hours | Global | Agent |
| Guest heartbeat interval | 5s | No | Guest |
| Guest heartbeat loss | 3 missed (15s) | No | Agent |
| Agent heartbeat interval | 10s | No | Agent |
| Agent heartbeat loss | 3 missed (30s) | No | Controller |

---

## Network Architecture

### Per-VM Network Setup

Each Firecracker VM gets:

1. **Dedicated tap interface** on the host: `tap-{vm_id}`
2. **Private /30 subnet**: `10.200.{n}.1/30` (host), `10.200.{n}.2/30` (guest)
3. **NAT via iptables MASQUERADE**: outbound internet via host's default route
4. **DNS relay**: guest resolves via host-side DNS forwarder at `10.200.{n}.1:53`

### iptables Rules (Per VM)

```bash
# Allow established/related traffic (return packets for outbound connections)
-A FORWARD -i tap-{vm_id} -m state --state ESTABLISHED,RELATED -j ACCEPT

# Allow outbound to internet (NAT egress)
-A FORWARD -i tap-{vm_id} -o {host_iface} -j ACCEPT

# BLOCK all traffic between tap interfaces (prevent lateral movement between VMs)
-A FORWARD -i tap-{vm_id} -o tap-+ -j DROP

# BLOCK access to host network services (except DNS)
-A INPUT -i tap-{vm_id} -p udp --dport 53 -d 10.200.{n}.1 -j ACCEPT
-A INPUT -i tap-{vm_id} -j DROP

# NAT outbound traffic
-t nat -A POSTROUTING -s 10.200.{n}.0/30 -o {host_iface} -j MASQUERADE
```

### What This Achieves

| Traffic | Allowed? | Why |
|---------|----------|-----|
| VM → Internet | Yes (NAT) | Terraform needs provider downloads, cloud API calls |
| VM → VM | No | Prevents lateral movement between tenants |
| VM → Host services | No (except DNS) | Prevents attacking agent, forest-server, databases |
| Internet → VM | No | No inbound, no public IP |
| VM → forest-server | No | Guest communicates only via vsock to host agent |

### Guest ↔ Host Communication: vsock

All communication between the guest and host uses **AF_VSOCK** (virtio-vsock), NOT the network:

- **Logs**: Guest streams stdout/stderr over vsock to host agent
- **Artifacts**: Host pushes files to guest over vsock
- **Heartbeat**: Guest sends periodic heartbeats over vsock
- **Completion**: Guest reports exit code over vsock

This means the guest never needs network access to forest-server and never holds a release token.

### Terraform Network Requirements

Terraform jobs need outbound internet for:

| Purpose | Destination | How |
|---------|-------------|-----|
| Provider downloads | registry.terraform.io, GitHub releases | NAT egress |
| Cloud API calls | AWS, GCP, Azure endpoints | NAT egress |
| State backend | forest-server terraform state API | NAT egress (or vsock proxy) |

The state backend URL is injected via `TF_HTTP_ADDRESS` / `TF_HTTP_LOCK_ADDRESS` / `TF_HTTP_UNLOCK_ADDRESS` environment variables. The existing `crates/forest-server/src/destinations/terraformv1.rs` already uses this pattern.

**Optional optimization (Phase 2):** Terraform provider mirror running on the host or shared pool machine, exposed to VMs at a known internal IP. Reduces external downloads and improves cold-start time. The existing `FOREST_TERRAFORM_PROVIDER_MIRROR_URL` environment variable already supports this.

---

## Security Model

### Credential Injection

Secrets flow: `forest-server → controller → agent → guest (vsock)`

**Rules:**
- Secrets are passed as environment variables to the job process
- Secrets are NEVER written to the rootfs image
- Secrets are NEVER logged (log streaming filters known secret keys)
- Secrets are NEVER persisted to disk on the host after VM destruction
- The guest has no access to the controller's or agent's credentials

**Credential types:**
- Cloud provider credentials (AWS_ACCESS_KEY_ID, etc.) — injected via destination metadata
- Git tokens (for GitOps destinations) — injected via destination metadata
- Terraform state backend auth — injected via TF_HTTP_USERNAME/PASSWORD environment variables
- Release token — NOT passed to the guest (controller holds it)

### Filesystem Isolation

- Each VM boots from a **read-only base rootfs** with a **writable CoW overlay**
- The overlay is destroyed when the VM exits
- No volume mounts between VMs
- No access to host filesystem from the guest
- The guest has no mechanism to access the host agent's filesystem (KVM + jailer chroot)

### Resource Limits

| Resource | Default | Range | Enforced By |
|----------|---------|-------|-------------|
| vCPUs | 2 | 1-8 | Firecracker config |
| Memory | 1 GiB | 256 MiB - 8 GiB | Firecracker config |
| Rootfs disk | 4 GiB | 1 GiB - 20 GiB | Rootfs image size |
| Network bandwidth | 100 Mbit/s | 10-1000 Mbit/s | Firecracker rate limiter |
| Network PPS | 10,000 | 1,000-100,000 | Firecracker rate limiter |
| Job timeout | 30 min | 1 min - 2 hr | Agent kills VM |

### Organisation-Level Isolation

- Each job runs in its own VM — guaranteed by ephemeral model
- No shared filesystem or network namespace between organisations
- VMs cannot communicate with each other (iptables blocks tap↔tap)
- Optional: dedicated machine pools per organisation
- Logs and artifacts scoped by release token (org-scoped)

### Audit Trail

All VM lifecycle events are logged with structured fields:

```
{
  "event": "hollow.job.vm.boot",
  "org_id": "uuid",
  "release_id": "uuid",
  "job_id": "uuid",
  "vm_id": "vm-abc123",
  "agent": "ax41-01",
  "vcpus": 2,
  "memory_mib": 1024,
  "image": "terraform-1.8",
  "timestamp": "2026-04-24T12:00:00Z"
}
```

---

## gRPC API Design

### HollowService (controller → forest-server integration)

This is the **optional** richer API exposed by the controller. For Phase 1, the controller integrates purely via the existing `RunnerService` protocol. `HollowService` is added when forest-server needs direct Hollow capabilities.

```protobuf
syntax = "proto3";
package forest.v1;

service HollowService {
  // Submit a job for isolated execution.
  rpc SubmitJob(SubmitJobRequest) returns (SubmitJobResponse);

  // Cancel a running or queued job.
  rpc CancelJob(CancelJobRequest) returns (CancelJobResponse);

  // Get current job status.
  rpc GetJobStatus(GetJobStatusRequest) returns (GetJobStatusResponse);

  // Stream real-time logs from a running job.
  rpc StreamJobLogs(StreamJobLogsRequest) returns (stream JobLogLine);

  // List agents in the pool with status and capacity.
  rpc ListAgents(ListAgentsRequest) returns (ListAgentsResponse);

  // Get aggregate pool utilization metrics.
  rpc GetPoolMetrics(GetPoolMetricsRequest) returns (GetPoolMetricsResponse);
}

message SubmitJobRequest {
  string release_token = 1;
  string release_id = 2;
  string organisation = 3;
  JobSpec spec = 4;
}

message JobSpec {
  string image = 1;                        // e.g. "terraform-1.8", "base-ubuntu-22.04"
  repeated string command = 2;             // e.g. ["terraform", "plan"]
  map<string, string> environment = 3;     // env vars (includes secrets)
  repeated JobFile files = 4;              // files to inject into /work
  ResourceRequirements resources = 5;
  uint32 timeout_seconds = 6;              // max execution time
  NetworkPolicy network = 7;
}

message JobFile {
  string path = 1;
  bytes content = 2;
  uint32 mode = 3;  // octal, default 0644
}

message ResourceRequirements {
  uint32 vcpus = 1;       // default: 2
  uint32 memory_mib = 2;  // default: 1024
  uint32 disk_mib = 3;    // default: 4096
}

message NetworkPolicy {
  bool egress_enabled = 1;                 // default: true (NAT egress)
  repeated string allowed_egress_cidrs = 2; // optional: restrict to specific CIDRs
}

enum JobStatus {
  JOB_STATUS_UNSPECIFIED = 0;
  JOB_STATUS_PENDING = 1;
  JOB_STATUS_SCHEDULING = 2;
  JOB_STATUS_BOOTING = 3;
  JOB_STATUS_RUNNING = 4;
  JOB_STATUS_COMPLETING = 5;
  JOB_STATUS_COMPLETED = 6;
  JOB_STATUS_FAILED = 7;
  JOB_STATUS_TIMED_OUT = 8;
  JOB_STATUS_CANCELLED = 9;
}

message SubmitJobResponse { string job_id = 1; }
message CancelJobRequest { string job_id = 1; }
message CancelJobResponse {}

message GetJobStatusRequest { string job_id = 1; }
message GetJobStatusResponse {
  string job_id = 1;
  JobStatus status = 2;
  string agent_hostname = 3;
  string error_message = 4;
  optional string plan_output = 5;
}

message StreamJobLogsRequest {
  string job_id = 1;
  bool from_beginning = 2;
}

message JobLogLine {
  string channel = 1;  // "stdout" or "stderr"
  string line = 2;
  uint64 timestamp = 3;
}

message ListAgentsRequest {}
message ListAgentsResponse { repeated AgentInfo agents = 1; }

message AgentInfo {
  string agent_id = 1;
  string hostname = 2;
  uint32 total_vcpus = 3;
  uint32 available_vcpus = 4;
  uint32 total_memory_mib = 5;
  uint32 available_memory_mib = 6;
  uint32 active_vms = 7;
  bool healthy = 8;
}

message GetPoolMetricsRequest {}
message GetPoolMetricsResponse {
  uint32 total_agents = 1;
  uint32 healthy_agents = 2;
  uint32 active_jobs = 3;
  uint32 pending_jobs = 4;
  double cpu_utilization = 5;
  double memory_utilization = 6;
}
```

### HollowAgentService (controller ↔ agent internal)

```protobuf
syntax = "proto3";
package hollow.internal;

service HollowAgentService {
  // Bidirectional stream for agent registration and job dispatch.
  rpc RegisterAgent(stream AgentMessage) returns (stream ControllerMessage);
}

message AgentMessage {
  oneof message {
    AgentRegister register = 1;
    AgentHeartbeat heartbeat = 2;
    JobUpdate job_update = 3;
    JobLogBatch log_batch = 4;
  }
}

message AgentRegister {
  string agent_id = 1;
  string hostname = 2;
  string pool = 3;
  uint32 total_vcpus = 4;
  uint32 total_memory_mib = 5;
  uint32 total_disk_mib = 6;
  string kernel_version = 7;
  string firecracker_version = 8;
  repeated string available_images = 9;
}

message AgentHeartbeat {
  uint32 active_vms = 1;
  uint32 available_vcpus = 2;
  uint32 available_memory_mib = 3;
  uint32 available_disk_mib = 4;
  double load_1m = 5;
  double load_5m = 6;
}

message JobUpdate {
  string job_id = 1;
  JobStatus status = 2;
  string error_message = 3;
  optional string plan_output = 4;
  int32 exit_code = 5;
}

message JobLogBatch {
  string job_id = 1;
  repeated LogLine lines = 2;
}

message LogLine {
  string channel = 1;
  string line = 2;
  uint64 timestamp = 3;
}

message ControllerMessage {
  oneof message {
    AgentRegisterAck register_ack = 1;
    RunJob run_job = 2;
    CancelJob cancel_job = 3;
  }
}

message AgentRegisterAck {
  string agent_id = 1;
  bool accepted = 2;
  string reason = 3;
}

message RunJob {
  string job_id = 1;
  string image = 2;
  repeated string command = 3;
  map<string, string> environment = 4;
  repeated JobFile files = 5;
  uint32 vcpus = 6;
  uint32 memory_mib = 7;
  uint32 disk_mib = 8;
  uint32 timeout_seconds = 9;
  bool egress_enabled = 10;
}

message CancelJob {
  string job_id = 1;
}
```

### vsock Protocol (agent ↔ guest)

Simple length-prefixed binary protocol over AF_VSOCK:

```
┌─────────┬──────────┬─────────────┐
│ type(u8) │ len(u32) │ payload     │
└─────────┴──────────┴─────────────┘
```

Message types:

| Type | Direction | Payload |
|------|-----------|---------|
| 0x01 | Host→Guest | JobDefinition (JSON): command, env, files |
| 0x02 | Guest→Host | Ready signal |
| 0x03 | Guest→Host | LogLine (JSON): channel, line, timestamp |
| 0x04 | Guest→Host | Heartbeat |
| 0x05 | Guest→Host | Completion (JSON): exit_code, plan_output |
| 0x06 | Host→Guest | Cancel signal |

---

## Observability

### Log Streaming Pipeline

```
job process stdout/stderr
  │ (pipe)
  ▼
hollow-guest (captures lines)
  │ (vsock, type 0x03)
  ▼
hollow-agent (buffers + forwards)
  │ (gRPC AgentMessage.log_batch)
  ▼
hollow-controller (routes to forest-server)
  │ (gRPC PushLogs)
  ▼
forest-server (stores in release_logs, streams to UI)
```

End-to-end latency target: <200ms from job process emit to UI display.

### Metrics (Prometheus)

```
# Controller metrics
hollow_jobs_total{status, org}                   # counter
hollow_job_duration_seconds{org}                  # histogram
hollow_job_boot_duration_seconds                  # histogram
hollow_job_queue_depth{org}                       # gauge
hollow_job_queue_wait_seconds{org}                # histogram

# Agent metrics (per agent)
hollow_agent_vms_active{agent}                    # gauge
hollow_agent_cpu_available{agent}                 # gauge
hollow_agent_memory_available_mib{agent}          # gauge
hollow_agent_vm_boot_duration_seconds{agent}      # histogram

# Pool metrics
hollow_pool_agents_total{pool, status}            # gauge
hollow_pool_cpu_utilization{pool}                 # gauge
hollow_pool_memory_utilization{pool}              # gauge
```

### Structured Logging

All components use `tracing` with structured fields (consistent with Forest's existing patterns):

```rust
tracing::info!(
    job_id = %job_id,
    release_id = %release_id,
    org = %org,
    agent = %agent_hostname,
    vm_id = %vm_id,
    event = "hollow.job.vm.boot",
    "microVM booted successfully"
);
```

### Audit Events

| Event | When | Key Fields |
|-------|------|------------|
| `hollow.job.submitted` | Job enters queue | job_id, org, release_id |
| `hollow.job.scheduled` | Agent assigned | job_id, agent |
| `hollow.job.vm.boot` | VM started | job_id, vm_id, image, vcpus, memory |
| `hollow.job.vm.ready` | Guest connected | job_id, vm_id, boot_duration_ms |
| `hollow.job.started` | Job process spawned | job_id, command |
| `hollow.job.completed` | Success | job_id, exit_code, duration_s |
| `hollow.job.failed` | Failure | job_id, exit_code, error |
| `hollow.job.timeout` | Killed by timeout | job_id, timeout_s |
| `hollow.job.cancelled` | Cancelled by user/system | job_id, reason |
| `hollow.job.vm.destroyed` | Resources cleaned up | job_id, vm_id |

---

## Failure Handling

### Agent (Machine) Death

**Scenario:** Host machine loses power, kernel panics, or network partitions.

1. Agent stops sending heartbeats to controller
2. After 3 missed heartbeats (30s), controller marks agent as unhealthy
3. All jobs on that agent transition to FAILED with error "agent unreachable"
4. Controller calls `CompleteRelease` with FAILURE for each affected release
5. Failed releases become eligible for re-scheduling by forest-server (if retry policy configured)
6. Controller removes agent from active pool

This mirrors the existing pattern in `crates/forest-server/src/grpc/runner.rs` where runner disconnect triggers token revocation.

### Job Hangs

**Scenario:** Job process enters infinite loop or deadlock.

1. Guest agent continues sending heartbeats (this is NOT a heartbeat failure)
2. Job timeout fires on host agent (default 30 minutes)
3. Host agent kills Firecracker process (SIGTERM → 5s → SIGKILL)
4. Job reported as TIMED_OUT
5. All VM resources cleaned up

### VM Boot Failure

**Scenario:** Firecracker fails to start (resource exhaustion, bad image, KVM error).

1. Agent reports `JobUpdate` with status FAILED and Firecracker error message
2. Controller marks job as failed
3. If transient (e.g., temporary resource exhaustion): retry on another agent (max 1 retry)
4. If persistent (e.g., bad rootfs): fail permanently

### Guest Agent Crash

**Scenario:** `hollow-guest` binary crashes inside the VM.

1. vsock connection drops
2. Host agent detects vsock disconnection within 5s
3. Host agent kills Firecracker process
4. Job reported as FAILED with "guest agent lost"

### Controller Crash / HA

The controller uses Forest's **event-sourced aggregate model** with **application-defined sharding** for horizontal scaling. Multiple controller instances can run concurrently, each owning a shard of the job space (e.g., sharded by organisation or job ID hash).

**Event-sourced state:**
- Job lifecycle events (submitted, scheduled, booted, completed, failed) are persisted to the event store
- Controller state is rebuilt from events on restart — no in-memory-only state is authoritative
- Agent pool membership is ephemeral (rebuilt from agent reconnections), but job state survives restarts

**Sharding model:**
- Each controller instance owns a partition of jobs (application-defined shard key, e.g., `org_id % shard_count`)
- Agents connect to all controller instances; the controller that owns a job's shard dispatches it
- Shard rebalancing on controller add/remove follows the same patterns as other Forest aggregates

**Crash recovery:**
1. Agents detect controller disconnect, reconnect with exponential backoff to remaining instances
2. Running VMs on agents continue executing (independent processes)
3. Surviving controller instances pick up orphaned shards
4. On restart, the recovering controller replays events to rebuild state, agents reconnect
5. forest-server detects the runner disconnect for the crashed instance, revokes affected release tokens (existing behavior)
6. In-flight releases on orphaned shards are re-queued by forest-server's scheduler

### Network Partition (Controller ↔ Agent)

1. Agent continues running active VMs for their remaining timeout
2. Logs are buffered locally on agent (bounded buffer, drops oldest on overflow)
3. On reconnection, agent reports current state
4. If partition lasts longer than job timeout, agent kills VMs locally

---

## Integration with Forest

### How Hollow Fits the Existing Runner Model

The key design insight: **the hollow-controller is a forest-runner**. It implements the client side of `RunnerService.RegisterRunner` exactly as `crates/forest-runner/src/service.rs` does.

```
forest-server sees:
  Runner "hollow-prod" | capabilities: [forest/terraform@1, forest/fluxv1@1] | max_concurrent: 20

The scheduler doesn't know or care that this "runner" dispatches to
Firecracker microVMs on a fleet of bare-metal machines. It just sees
a runner with capacity.
```

### What the Controller Replicates from forest-runner

The controller mirrors the executor flow from `crates/forest-runner/src/executor.rs`:

1. Receives `WorkAssignment` on the bidirectional stream
2. Opens log stream (`session.open_log_stream()`)
3. Fetches release files, spec files, annotation, project info
4. **Instead of** calling `handler.prepare()` + `handler.release()` locally:
   - Packages everything into a `RunJob` message
   - Dispatches to an agent
   - Streams logs back as they arrive
5. On completion, calls `session.complete_release()` with outcome

### Zero forest-server Changes Required (Phase 1)

- Scheduler: unchanged — calls `runner_manager.try_assign()` as usual
- RunnerManager: unchanged — tracks the controller as a connected runner
- Release event store: unchanged — releases follow the same state machine
- Token registry: unchanged — release tokens work identically
- PushLogs: unchanged — controller pushes logs on behalf of the VM
- CompleteRelease: unchanged — controller reports outcomes

### Future forest-server Integration (Phase 2+)

- `HollowService` for direct job submission (bypassing the runner protocol for richer control)
- Job status API in forest-server UI
- Pool management dashboard
- Network policy configuration per destination

---

## Migration Path

### Phase 1: Hollow as an Additional Runner

**Goal:** Deploy alongside existing runners. No forest-server changes.

1. Create new crates: `hollow`, `hollow-agent`, `hollow-guest`
2. Build rootfs images (Ubuntu + Terraform, Ubuntu + Git/Flux)
3. Deploy controller alongside forest-server
4. Deploy agents on 1-2 Hetzner dedicated servers
5. Controller registers with `capabilities: [forest/terraform@1]` only
6. Scheduler automatically routes terraform jobs to Hollow
7. Existing runners continue handling flux and other destinations

**Validation:** Compare terraform plan/apply outputs between old runner and Hollow. Verify identical behavior.

### Phase 2: All Destinations

1. ✅ MVP — `Dockerfile.fluxv1` ships git + openssh-client + flux CLI +
   kustomize CLI; controller has `fluxv1` arm; integration test proves the
   image boots and the toolchain is reachable.
2. ✅ Real fluxv1 git workflow — `forest-flux-deploy` baked into the image
   does clone → write manifests at
   `releases/<env>/<dest>/<cluster>/<ns>/<project>/` → commit → push.
   Configuration via destination metadata env vars; SSH key ships through
   the secret channel as a Secret targeting `/root/.ssh/id_forest`. Acceptance
   test exercises the full path against a `file://` bare repo and asserts
   on the `FLUX_PUSHED` sentinel.
3. ✅ Secret-shipping channel — `RunJob.secrets` carries name/target_path/
   mode/content; agent and guest redact `content` everywhere; written to
   the target path with the requested mode before the job's command runs.
4. Add forage/component destination support — `forage` is gRPC-based, no
   shell command surface, so this likely stays on the legacy in-process
   runner.
5. Increase controller's `max_concurrent`, decrease old runner's `max_concurrent`
6. Gradually shift all traffic to Hollow

### Phase 3: Deprecate Old Runners

1. Set `FOREST_DISABLE_IN_PROCESS=true` on forest-server
2. Remove old `forest-runner` processes
3. All execution goes through Hollow

### Phase 4: Advanced Features

1. Snapshot-based fast boot (<50ms cold start)
2. Machine pool autoscaling via Hetzner API
3. Per-org dedicated pools
4. ✅ Network egress filtering (per-destination CIDR allowlists) — sourced
   from `destination.metadata.allowed_egress_cidrs` (comma-separated). When
   set, the VM may only reach those CIDRs; everything else is dropped at
   the FORWARD chain. IMDS / RFC1918 blocks remain unconditional.
5. Terraform provider mirror cache
6. Job result caching
7. `HollowService` gRPC API for forest-server integration
8. Pool management dashboard

---

## Resolved Decisions

| # | Question | Decision |
|---|----------|----------|
| 1 | Rootfs image management | Custom-built images per destination type/version (see VM Image Build System) |
| 2 | vsock vs virtio-net | vsock. Revisit only if performance is a problem. |
| 3 | State persistence between stages | No. Each run is from scratch. Future: consider job hibernate/wait for warm workspaces. |
| 4 | ARM support | Deferred. API supports `arch` field for future use, implementation later. |
| 5 | GPU workloads | Not needed. |
| 6 | Agent binary distribution | Handled by existing provisioning scheme, not Hollow's concern. |
| 7 | Docker/CI | Not a current feature. Validate with a bare Docker smoke test image only. |
| 8 | Cost model / initial machine | Single local machine (2 vCPU, 16 GB RAM) for development. |
| 9 | Controller HA | Event-sourced aggregates with application-defined sharding. |

## Resolved Decisions (Continued)

| # | Question | Decision |
|---|----------|----------|
| 10 | Kernel | Stock distribution kernel. No custom kernel maintenance burden. |
| 11 | Image build tooling | Docker build + export to ext4. Structured, reproducible, universally known. See Image Build Pipeline below. |
| 12 | Dev iteration loop | Target <10s from code change to boot + registration. See Development Workflow below. |

### Image Build Pipeline

Images are built as Docker images, then exported to raw ext4 for Firecracker. This gives us Dockerfiles (structured, layered, reproducible) without inventing new tooling.

```
hollow/images/
  ├── Dockerfile.base          # Shared base: Alpine + hollow-guest + init
  ├── Dockerfile.terraform-v1  # FROM base, adds terraform binary
  ├── Dockerfile.fluxv1        # FROM base, adds git + kustomize + flux CLI
  ├── Dockerfile.docker-test   # FROM base, adds Docker (validation only)
  ├── Makefile                 # Build targets per image
  └── scripts/
      └── pack-ext4.sh         # docker export → ext4 image
```

**Base image (`Dockerfile.base`):**
```dockerfile
FROM alpine:3.21
RUN apk add --no-cache openrc busybox-initscripts ca-certificates curl
COPY hollow-guest /usr/local/bin/hollow-guest
RUN chmod +x /usr/local/bin/hollow-guest
# hollow-guest runs as PID 1 or as an openrc service
```

**Variant image (`Dockerfile.terraform-v1`):**
```dockerfile
FROM hollow-base:latest
ARG TERRAFORM_VERSION=1.8.5
RUN curl -fsSL https://releases.hashicorp.com/terraform/${TERRAFORM_VERSION}/terraform_${TERRAFORM_VERSION}_linux_amd64.zip \
    | unzip -d /usr/local/bin/ -
```

**Export to ext4 (`pack-ext4.sh`):**
```bash
#!/bin/bash
# Usage: pack-ext4.sh <image-tag> <output.ext4> <size-mb>
set -euo pipefail
IMAGE=$1; OUTPUT=$2; SIZE_MB=${3:-2048}

CID=$(docker create "$IMAGE")
trap "docker rm $CID" EXIT

dd if=/dev/zero of="$OUTPUT" bs=1M count="$SIZE_MB"
mkfs.ext4 -F "$OUTPUT"

MOUNT=$(mktemp -d)
sudo mount -o loop "$OUTPUT" "$MOUNT"
trap "sudo umount $MOUNT; rmdir $MOUNT; docker rm $CID" EXIT

docker export "$CID" | sudo tar -xf - -C "$MOUNT"
sudo umount "$MOUNT"
rmdir "$MOUNT"
```

**Reproducibility:** Pin base image digests (`FROM alpine:3.21@sha256:...`), pin tool versions via build args, use `--no-cache` in CI for clean builds. Good enough — not Nix-level, but practical.

**Makefile targets:**
```makefile
.PHONY: base terraform-v1 fluxv1 all

base:
	docker build -t hollow-base -f Dockerfile.base .

terraform-v1: base
	docker build -t hollow-terraform-v1 -f Dockerfile.terraform-v1 .
	./scripts/pack-ext4.sh hollow-terraform-v1 out/terraform-v1.ext4 2048

fluxv1: base
	docker build -t hollow-fluxv1 -f Dockerfile.fluxv1 .
	./scripts/pack-ext4.sh hollow-fluxv1 out/fluxv1.ext4 2048

all: terraform-v1 fluxv1
```

### Development Workflow

Target: **<10s from code change to boot + registration**.

**Dev setup:** Controller and agent run as two local processes on the dev machine (which has KVM). Images and kernel are pre-built once (not part of the iteration loop).

**Iteration loop:**

```
1. Edit code                              (0s)
2. cargo build -p hollow-agent            (~3-5s incremental)
   OR cargo build -p hollow               (~3-5s incremental)
   OR cargo build -p hollow-guest         (~1-2s, small binary)
3. Restart agent/controller process       (<100ms)
4. Agent reconnects + registers           (<500ms)
5. Submit test job                        (<100ms)
6. Firecracker boots VM                   (<500ms)
7. Guest connects over vsock              (<200ms)
                                    TOTAL: ~5-7s
```

**Key enablers:**
- **Incremental compilation**: Small Rust crates compile fast. Keep `hollow`, `hollow-agent`, `hollow-guest` as separate crates with minimal dependencies.
- **Pre-built images**: `terraform-v1.ext4` and `vmlinux` sit on disk. Only rebuild when changing image contents.
- **hollow-guest is static musl**: Fast compile (~1-2s), no dynamic linking issues. Copy into the ext4 image once, or mount-inject at VM boot time for faster iteration (avoid rebuilding the whole ext4 on guest changes).
- **Hot restart**: Agent and controller are just processes — kill and restart. No container rebuild, no deployment step.
- **`mise run dev`**: Single command that starts controller + agent + watches for rebuilds. Uses `cargo-watch` or similar for auto-rebuild on save.

**Guest binary iteration shortcut:** During development, instead of rebuilding the ext4 image every time `hollow-guest` changes, the agent can inject the guest binary via a secondary virtio-block device or pass it over vsock at boot. This keeps the ext4 image stable and only swaps the guest binary.

**Firecracker-only:** Hollow has no subprocess or host-execution fallback. All jobs run inside a Firecracker microVM — that is the isolation contract, and the only execution mode the agent supports. Tests run against a real KVM-capable Linux host (see `hollow-test-harness`).

## Command Dispatch Abstraction

> This section sketches the evolution of the "what to run in the VM" model.
> Today it's hardcoded; soon we'll need to accept user-defined commands.

### Today

`hollow-controller` maps a destination name to a fixed shell command inside
`build_command_for_destination` (see `hollow/crates/hollow-controller/src/dispatcher.rs`):

| Destination | Command |
|-------------|---------|
| `terraform` | `terraform init && terraform {plan,apply}` (binary is OpenTofu under a `terraform → tofu` symlink in the rootfs) |
| `echo`      | `sh -c $metadata.command` (test-only) |

This matches the legacy `forest-runner` where each destination is a trait
implementation that owns its own process spawn and file plumbing
(`TerraformV1`, `ForageV1`, `FluxV1`). It's fine for first-party destinations
that Forest knows the shape of.

### The gap: custom CI

Users want to run arbitrary pre/post-deploy steps that Forest doesn't know
about in advance — lint, tests, smoke probes, database migrations, custom
deploy scripts. The legacy runner has no slot for these; neither does the
controller today.

### Target model

Introduce a `ci-script` / `exec` destination type whose contract is:

1. **Release artifacts** carry a `forest.yaml` (or similar) describing steps:

   ```yaml
   steps:
     - name: lint
       run: "./ci/lint.sh"
     - name: test
       run: "cargo test"
       env:
         RUST_LOG: info
     - name: deploy
       run: "./ci/deploy.sh"
   ```

2. **Controller** reads `forest.yaml` from the deployment files, translates
   it to a sequence of `RunJob`s — one per step OR a single job that runs
   all steps and streams results step-by-step. The first version runs all
   steps in one VM to avoid VM-boot overhead per step.

3. **Image selection** is configurable per step: `image: "ci-node-20"`,
   `image: "ci-rust-stable"`, etc. Forest ships a small catalogue;
   organisations can register their own.

4. **Output contract**: each step's stdout/stderr streams back with a
   `step=<name>` tag in the log channel, so the UI can present per-step
   collapsible logs and mark pass/fail independently.

### Migration

No existing destinations need to move. The `ci-script` destination lives
alongside `terraform`/`flux`; it's a separate code path in the dispatcher
with its own arm in `build_command_for_destination` that reads the
pipeline definition from metadata or release files.

Later, the first-party destinations can become sugar over the `ci-script`
substrate (e.g. `terraform` is a pre-baked pipeline of `init` → `plan` →
`apply` steps). That's a refactor, not a prerequisite.

### Where this lives

- `forest-server` — stores the pipeline definition as part of the release
  artifact; no change to `release_files` needed.
- `hollow-controller` — one arm in `build_command_for_destination` that
  parses `forest.yaml` from deployment files and dispatches accordingly.
- `hollow-guest` — no change initially (one command executed per VM). When
  multi-step-per-VM arrives, the guest learns a new vsock message type
  for `ExecuteStep(name, cmd, env)` that the host agent sends in sequence.

## Deferred Hardening

The post-Phase-1 security audit (see `git log --grep "audit"`) closed five
issues: read-only rootfs + tmpfs scratch, cgroup v2 caps, iptables egress
lockdown (INPUT chain + IMDS + RFC1918 blocks), console-capture
default-off, and SHA256 pinning of every external artifact. Two items are
deliberately deferred:

- **Agent privilege split.** Today the agent runs as root because it owns
  `ip tuntap` / `iptables` / chroot setup. A "small privileged helper +
  unprivileged daemon" split is a textbook hardening pattern, but it
  defends only against the case where Firecracker's KVM boundary has
  *already* been broken. KVM+Firecracker has had ~1–2 critical CVEs/year
  historically, all patched within days; relying on that boundary plus
  fast patching is a defensible trade-off versus the maintenance cost of
  a split-process agent + IPC contract. Revisit if a real customer
  threat model demands it.

- **Agent-side rootfs digest verification.** Build-time pinning means
  the bytes on the operator's disk match what we built; we don't yet
  verify those bytes haven't been swapped on-disk between build and
  launch. A signed `image_name → sha256` manifest checked at mount time
  closes the gap. Smaller lift than the privilege split; do this when we
  get a real image-distribution story (S3/MinIO/registry) instead of the
  current rsync-from-dev pattern.

## Open Questions

None currently. All architectural decisions are resolved for Phase 1.

---

## Appendix: Customer Workload Scenarios

Hollow should support a wide range of release management workloads. Below are concrete customer scenarios across different business types, the workloads they generate, and what that demands from the runtime.

### Scenario 1: SaaS Startup (10-50 engineers)

**Profile:** B2B SaaS, microservices on Kubernetes, Terraform for cloud infra, GitHub for source.

**Workloads:**
- **Terraform plan/apply** for AWS/GCP infrastructure (VPCs, RDS, EKS clusters, IAM). Needs cloud provider credentials, network egress to cloud APIs, state locking.
- **Kubernetes manifest deployment** via Kustomize/Helm rendered manifests pushed through GitOps (Flux). Needs git access, cluster kubeconfig.
- **Database migrations** run as pre-deploy steps. Needs database network access (private VPC peering or egress to managed DB endpoints).
- **Smoke tests** post-deploy — curl health endpoints, run a small integration suite. Needs egress to the deployed service.

**Hollow requirements:** Multi-step pipelines (plan → approve → deploy → test), cloud credential injection, moderate compute (2 vCPU, 1-2 GiB), 5-15 min job duration, frequent runs (10-50/day).

### Scenario 2: E-commerce / Retail Platform

**Profile:** Monolith + some services, multiple environments (staging, canary, production per region), strict change management.

**Workloads:**
- **Multi-region deployments** — same artifact rolled out to EU, US, APAC sequentially with health gates between regions.
- **Terraform for CDN/edge config** — CloudFront distributions, DNS records, WAF rules. Fast, small plans.
- **Feature flag rollouts** — configuration changes pushed to a feature flag service. Tiny jobs, but frequent.
- **Scheduled deployments** — deploy at 2am in each region's timezone to minimize customer impact.
- **Rollback automation** — triggered by monitoring alerts, needs to re-deploy previous known-good artifact fast.

**Hollow requirements:** Pipeline DAG with regional fan-out, scheduling/cron triggers, fast job start (<5s for rollbacks), small compute per job, high reliability (rollback must not fail because the runtime is busy).

### Scenario 3: Fintech / Regulated Industry

**Profile:** Strict compliance (SOC2, PCI-DSS, HIPAA), audit trail for every change, segregation of duties.

**Workloads:**
- **Terraform with approval gates** — every infra change requires plan review + approval from a different person before apply.
- **Compliance scanning** as a pipeline stage — run policy checks (OPA/Rego, Checkov, tfsec) on the plan output before allowing apply.
- **Secrets from external vaults** — credentials fetched from HashiCorp Vault or AWS Secrets Manager at runtime, never stored in Forest.
- **Audit logging** — every job execution, who triggered it, what changed, full log retention.
- **Network egress restrictions** — jobs should only reach specific approved endpoints (cloud APIs, internal services). No arbitrary internet access.

**Hollow requirements:** Per-job network egress CIDR allowlists, integration with external secret managers (env var injection from vault), complete audit trail (already in spec), approval workflows (already in Forest), policy stage support.

### Scenario 4: Platform Team / Internal Developer Platform

**Profile:** Central platform team serving 20+ product teams, each with their own apps/services, self-service deployment.

**Workloads:**
- **Tenant-isolated deployments** — each product team deploys independently, cannot see or affect other teams' releases.
- **Custom deployment scripts** — some teams use Terraform, some use Pulumi, some have bespoke deploy scripts (shell, Python). Need to run arbitrary binaries.
- **Shared infrastructure management** — platform team manages shared resources (databases, message queues, networking) via Terraform, with tighter controls.
- **Self-service onboarding** — new teams register, get a project, configure destinations, start deploying. Minimal platform team involvement.
- **Resource quotas** — prevent any single team from consuming all runner capacity.

**Hollow requirements:** Arbitrary binary execution (not just terraform/flux), per-org resource quotas (vCPU/memory limits, max concurrent jobs), org-level isolation (already in spec), custom rootfs images per team (bring-your-own-tools image).

### Scenario 5: IoT / Edge / Embedded Company

**Profile:** Manages a fleet of edge devices or embedded systems, firmware and config updates pushed centrally.

**Workloads:**
- **Firmware build + signing** — compile firmware, sign with a hardware security module (HSM) or signing service, produce an artifact.
- **Config generation** — generate per-device or per-site configuration from templates + a device registry.
- **Staged rollout** — push firmware to 1% of devices, monitor error rates, then 10%, then 100%. Pipeline with monitoring gates.
- **Terraform for cloud backend** — the cloud services that edge devices call home to.

**Hollow requirements:** Longer job durations (firmware builds: 15-60 min), larger compute for compilation (4-8 vCPU, 4-8 GiB), artifact output capture (built firmware binary returned as a release artifact), staged rollout with monitoring feedback.

### Scenario 6: Agency / Consultancy (Multi-Client)

**Profile:** Manages infrastructure and deployments for 10-50 client organisations, each with separate cloud accounts and environments.

**Workloads:**
- **Per-client Terraform** — each client has their own AWS account, their own state, their own variables. Strict isolation between clients.
- **Shared tooling, isolated execution** — same terraform version and modules, but completely separate credentials and state.
- **Bulk operations** — "upgrade this module across all 30 clients" — fan-out of plan/apply across many orgs.
- **Client-facing audit** — per-client deployment history exportable for their compliance needs.

**Hollow requirements:** Strong multi-tenant isolation (already core to Hollow), per-org credential scoping, fan-out/bulk job submission, per-org audit log export.

### Scenario 7: Data / ML Platform

**Profile:** Data pipelines, ML model training, model deployment to inference endpoints.

**Workloads:**
- **Terraform for data infra** — Snowflake, BigQuery, Databricks workspaces, S3 buckets, IAM.
- **dbt run/test** as a deployment step — run data transformations as part of a release pipeline.
- **Model deployment** — push a model artifact to a serving endpoint (SageMaker, Vertex AI, custom K8s).
- **Integration tests** — run queries against the data warehouse post-deploy to validate transformations.

**Hollow requirements:** dbt / Python runtime in custom rootfs images, moderate compute, database egress (Snowflake, BigQuery endpoints), longer timeouts for data pipeline runs (30-60 min).

### Summary: What This Means for Hollow

| Capability | Scenarios | Priority |
|-----------|-----------|----------|
| Terraform plan/apply in isolation | All | Phase 1 |
| GitOps (Flux/Kustomize) in isolation | 1, 2 | Phase 2 |
| Arbitrary binary execution | 4, 5, 7 | Phase 2 |
| Multi-step pipeline (plan → approve → deploy) | 1, 2, 3 | Already in Forest |
| Per-org isolation | 4, 6 | Core to Hollow |
| Network egress filtering (CIDR allowlists) | 3 | Phase 4 |
| Custom rootfs images (bring-your-own-tools) | 4, 7 | Phase 3+ |
| Resource quotas per org | 4 | Phase 3+ |
| Fast job start for rollbacks | 2 | Snapshot boot (Phase 4) |
| Large compute jobs (4-8 vCPU) | 5, 7 | Supported in API, needs bigger machines |
| Artifact output capture | 5 | Phase 2+ |
| External secret manager integration | 3 | Phase 2+ |

The architecture as designed handles all of these scenarios. Phase 1 (Terraform isolation on a single machine) validates the core runtime. Subsequent phases expand workload types, add policy controls, and scale the machine pool.
