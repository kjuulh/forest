//! Per-VM networking: tap interface + iptables NAT.
//!
//! Each VM gets its own /30 inside `10.200.0.0/16` (so up to 256 concurrent
//! VMs on a host), a dedicated tap interface, and a MASQUERADE rule that
//! NATs outbound traffic through the host's default-route interface.
//!
//! The setup is RAII: [`NetworkHandle::establish`] brings the tap + iptables
//! rules up; dropping the handle (or calling [`NetworkHandle::teardown`])
//! removes them. Requires the process to have `CAP_NET_ADMIN` — i.e. run
//! as root on the agent host.

use std::process::{Command, Output, Stdio};

use anyhow::{Context, bail};

pub const NETWORK_PREFIX: &str = "10.200";
pub const IFACE_PREFIX: &str = "hlw-";

/// Everything the VM needs to come up on the network. Constructed by
/// [`NetworkAllocator::allocate`] so the subnet index is unique across
/// concurrent VMs.
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    /// Unique subnet index in [0, 255]. Drives iface name, IPs, MAC.
    pub subnet_index: u8,
    /// Host outbound interface (e.g. "ens18"); MASQUERADE jumps through this.
    pub host_iface: String,
    /// DNS servers the guest should use. hollow-guest writes /etc/resolv.conf
    /// from this list before spawning the job.
    pub dns: Vec<String>,
}

impl NetworkConfig {
    pub fn tap_name(&self) -> String {
        format!("{IFACE_PREFIX}{}", self.subnet_index)
    }

    pub fn host_ip(&self) -> String {
        format!("{NETWORK_PREFIX}.{}.1", self.subnet_index)
    }

    pub fn guest_ip(&self) -> String {
        format!("{NETWORK_PREFIX}.{}.2", self.subnet_index)
    }

    pub fn netmask(&self) -> &'static str {
        "255.255.255.252"
    }

    pub fn subnet_cidr(&self) -> String {
        format!("{NETWORK_PREFIX}.{}.0/30", self.subnet_index)
    }

    /// MAC address: locally-administered (0x02 low nibble in first byte),
    /// subnet index baked in so two VMs never collide on a shared segment.
    pub fn guest_mac(&self) -> String {
        format!("06:00:AC:10:{:02x}:02", self.subnet_index)
    }

    /// Kernel cmdline `ip=` clause for static configuration. Matches the
    /// ip-config.txt kernel docs format:
    /// `ip=<client>:<server>:<gateway>:<netmask>:<hostname>:<device>:<autoconf>`
    pub fn kernel_ip_arg(&self) -> String {
        format!(
            "ip={}::{}:{}:hollow:eth0:off",
            self.guest_ip(),
            self.host_ip(),
            self.netmask()
        )
    }
}

/// RAII handle that owns the live tap interface and iptables rules for one
/// VM. Drop removes them.
pub struct NetworkHandle {
    config: NetworkConfig,
    /// True between `establish` and `teardown` — lets Drop know whether the
    /// kernel state is ours to remove.
    live: bool,
}

impl NetworkHandle {
    pub fn config(&self) -> &NetworkConfig {
        &self.config
    }

    /// Idempotent-ish: if a stale tap with the same name exists, clean it up
    /// first. Returns Err if any of the shell steps fail — caller should
    /// treat that as a fatal VM setup failure.
    pub fn establish(config: NetworkConfig) -> anyhow::Result<Self> {
        let tap = config.tap_name();

        // If something failed on a previous run and left state behind, clean
        // it up before reinstalling. Errors here are non-fatal (the delete
        // was speculative).
        let _ = run_ignore_err(&["ip", "link", "del", &tap]);
        let _ = remove_iptables_rules(&config);

        run("ip", &["tuntap", "add", "dev", &tap, "mode", "tap"])
            .with_context(|| format!("create tap {tap}"))?;
        run(
            "ip",
            &[
                "addr",
                "add",
                &format!("{}/30", config.host_ip()),
                "dev",
                &tap,
            ],
        )
        .with_context(|| format!("assign address to {tap}"))?;
        run("ip", &["link", "set", &tap, "up"]).with_context(|| format!("bring {tap} up"))?;

        add_iptables_rules(&config).context("install iptables rules")?;

        Ok(Self {
            config,
            live: true,
        })
    }

    pub fn teardown(&mut self) {
        if !self.live {
            return;
        }
        self.live = false;
        let _ = remove_iptables_rules(&self.config);
        let _ = run_ignore_err(&["ip", "link", "del", &self.config.tap_name()]);
    }
}

impl Drop for NetworkHandle {
    fn drop(&mut self) {
        self.teardown();
    }
}

/// CIDRs that MUST NOT be reachable from a VM. The intent is "public internet
/// only": no host LAN, no other tenants, no cloud instance metadata.
///
/// IMPORTANT: 10.0.0.0/8 covers our own per-VM /30 subnets, so this also
/// implicitly blocks VM→VM traffic via the routing path. The dedicated
/// tap-to-tap rule below is still installed for defence in depth (and to
/// make the intent explicit when reading `iptables -L`).
const BLOCKED_DESTS: &[&str] = &[
    "169.254.0.0/16", // link-local — cloud IMDS lives at 169.254.169.254
    "10.0.0.0/8",     // RFC1918
    "172.16.0.0/12",  // RFC1918
    "192.168.0.0/16", // RFC1918
];

fn add_iptables_rules(cfg: &NetworkConfig) -> anyhow::Result<()> {
    let tap = cfg.tap_name();
    let any_tap = format!("{IFACE_PREFIX}+");

    // Order matters: every -I X inserts at position X, shifting prior rules
    // down. We install all DROP rules with `-I FORWARD 1` so they end up
    // ahead of the ACCEPTs in the chain. iptables walks rules top-to-bottom,
    // so DROPs evaluated first means a denied packet never reaches MASQUERADE.

    // Block lateral movement between tenants.
    run(
        "iptables",
        &[
            "-I", "FORWARD", "1",
            "-i", &tap,
            "-o", &any_tap,
            "-j", "DROP",
        ],
    )?;

    // Block VM→host LAN, IMDS, anything else private. -d operates on the
    // routed destination IP, so a VM that pokes 192.168.0.1 (the host's
    // gateway, perhaps) is dropped before MASQUERADE rewrites the source.
    for cidr in BLOCKED_DESTS {
        run(
            "iptables",
            &[
                "-I", "FORWARD", "1",
                "-i", &tap,
                "-d", cidr,
                "-j", "DROP",
            ],
        )?;
    }

    // Block VM→host services. The VM's gateway is the host tap IP, so any
    // packet destined for `10.200.N.1` lands in INPUT after routing. INPUT's
    // default policy is usually ACCEPT, which would expose every host
    // service bound to 0.0.0.0 (Postgres, dockerd, the agent itself…).
    run(
        "iptables",
        &[
            "-I", "INPUT", "1",
            "-i", &tap,
            "-j", "DROP",
        ],
    )?;

    // Allow the VM's outbound traffic to the host's uplink (anything that
    // wasn't matched by the DROPs above is, by construction, public).
    run(
        "iptables",
        &[
            "-A", "FORWARD",
            "-i", &tap,
            "-o", &cfg.host_iface,
            "-j", "ACCEPT",
        ],
    )?;
    // And return packets for established connections.
    run(
        "iptables",
        &[
            "-A", "FORWARD",
            "-i", &cfg.host_iface,
            "-o", &tap,
            "-m", "state", "--state", "ESTABLISHED,RELATED",
            "-j", "ACCEPT",
        ],
    )?;

    // NAT outbound traffic from the VM's /30 behind the host's uplink IP.
    run(
        "iptables",
        &[
            "-t", "nat",
            "-A", "POSTROUTING",
            "-s", &cfg.subnet_cidr(),
            "-o", &cfg.host_iface,
            "-j", "MASQUERADE",
        ],
    )?;
    Ok(())
}

fn remove_iptables_rules(cfg: &NetworkConfig) -> anyhow::Result<()> {
    // Each rule gets `-D` (delete). Deletion by rule-spec must match exactly.
    // Errors are swallowed so a partial-teardown path still removes as much
    // as it can.
    let tap = cfg.tap_name();
    let any_tap = format!("{IFACE_PREFIX}+");

    let _ = run_ignore_err(&[
        "iptables", "-D", "FORWARD",
        "-i", &tap, "-o", &any_tap, "-j", "DROP",
    ]);
    for cidr in BLOCKED_DESTS {
        let _ = run_ignore_err(&[
            "iptables", "-D", "FORWARD",
            "-i", &tap, "-d", cidr, "-j", "DROP",
        ]);
    }
    let _ = run_ignore_err(&[
        "iptables", "-D", "INPUT",
        "-i", &tap, "-j", "DROP",
    ]);
    let _ = run_ignore_err(&[
        "iptables", "-D", "FORWARD",
        "-i", &tap, "-o", &cfg.host_iface, "-j", "ACCEPT",
    ]);
    let _ = run_ignore_err(&[
        "iptables", "-D", "FORWARD",
        "-i", &cfg.host_iface, "-o", &tap,
        "-m", "state", "--state", "ESTABLISHED,RELATED",
        "-j", "ACCEPT",
    ]);
    let _ = run_ignore_err(&[
        "iptables", "-t", "nat", "-D", "POSTROUTING",
        "-s", &cfg.subnet_cidr(),
        "-o", &cfg.host_iface,
        "-j", "MASQUERADE",
    ]);
    Ok(())
}

/// Detect the host's default-route outbound interface. Falls back to a
/// sensible default if parsing fails.
pub fn detect_host_iface() -> anyhow::Result<String> {
    let out = Command::new("ip")
        .args(["-4", "route", "show", "default"])
        .output()
        .context("spawn `ip route show default`")?;
    if !out.status.success() {
        bail!(
            "ip route show default exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Line looks like: "default via 192.168.1.1 dev ens18 proto dhcp src ..."
    let tokens: Vec<&str> = stdout.split_whitespace().collect();
    for w in tokens.windows(2) {
        if w[0] == "dev" {
            return Ok(w[1].to_string());
        }
    }
    bail!("no `dev <iface>` in default route: {stdout}");
}

fn run(bin: &str, args: &[&str]) -> anyhow::Result<()> {
    let out = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .with_context(|| format!("spawn {bin}"))?;
    check(bin, args, &out)
}

fn run_ignore_err(argv: &[&str]) -> anyhow::Result<()> {
    let bin = argv[0];
    let rest = &argv[1..];
    let _ = Command::new(bin)
        .args(rest)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .output();
    Ok(())
}

fn check(bin: &str, args: &[&str], out: &Output) -> anyhow::Result<()> {
    if !out.status.success() {
        bail!(
            "{bin} {:?} failed: status={}, stderr={}",
            args,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

// -- Allocator --------------------------------------------------------------

/// Process-wide allocator that hands out unique subnet indexes in [0, 255].
/// Drop of the returned `AllocatedSubnet` releases the slot.
///
/// Persistent tracking (across process restarts) is out of scope — the agent
/// is expected to clean up stale taps on startup using `scan_stale_taps`.
pub struct NetworkAllocator {
    inner: std::sync::Arc<std::sync::Mutex<AllocState>>,
}

struct AllocState {
    in_use: [bool; 256],
    next_hint: u16,
}

impl Default for NetworkAllocator {
    fn default() -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(AllocState {
                in_use: [false; 256],
                next_hint: 0,
            })),
        }
    }
}

impl Clone for NetworkAllocator {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl NetworkAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allocate(&self) -> anyhow::Result<AllocatedSubnet> {
        let mut state = self.inner.lock().expect("net alloc lock poisoned");
        let start = state.next_hint as usize;
        for offset in 0..256 {
            let idx = (start + offset) % 256;
            if !state.in_use[idx] {
                state.in_use[idx] = true;
                state.next_hint = ((idx + 1) % 256) as u16;
                return Ok(AllocatedSubnet {
                    index: idx as u8,
                    allocator: self.inner.clone(),
                });
            }
        }
        bail!("no free /30 subnet — 256 concurrent VMs in flight")
    }
}

/// Owns a subnet index; dropping returns it to the allocator.
pub struct AllocatedSubnet {
    pub index: u8,
    allocator: std::sync::Arc<std::sync::Mutex<AllocState>>,
}

impl Drop for AllocatedSubnet {
    fn drop(&mut self) {
        if let Ok(mut s) = self.allocator.lock() {
            s.in_use[self.index as usize] = false;
        }
    }
}

/// Remove any `hlw-*` tap interfaces that are still around from a previous
/// agent process. Safe to call at agent startup — it's best-effort and won't
/// touch anything that isn't a tap with the right prefix.
pub fn clean_stale_taps() -> anyhow::Result<()> {
    let out = Command::new("ip")
        .args(["-o", "link", "show", "type", "tuntap"])
        .output()
        .context("spawn `ip link show type tuntap`")?;
    if !out.status.success() {
        // Not fatal — some hosts may not have any taps.
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        // Format: "N: name@ifstate: <flags>"; we just need the name.
        let Some(colon1) = line.find(':') else { continue };
        let after_num = &line[colon1 + 1..].trim_start();
        let Some(colon2) = after_num.find(':') else { continue };
        let name = after_num[..colon2].trim();
        // Names may appear as "hlw-5@NONE" — strip the alias.
        let name = name.split('@').next().unwrap_or(name);
        if name.starts_with(IFACE_PREFIX) {
            let _ = run_ignore_err(&["ip", "link", "del", name]);
        }
    }
    Ok(())
}
