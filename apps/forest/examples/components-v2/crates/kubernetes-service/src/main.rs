#[allow(dead_code)]
mod forestgen;

use forestgen::*;
use std::fmt::Write;

struct K8sCommands;
struct DeploymentHooks;
struct ObservabilityHooks;
struct SecurityHooks;

// ---------------------------------------------------------------------------
// YAML generation helpers (unchanged — pure functions)
// ---------------------------------------------------------------------------

fn generate_deployment(spec: &Spec) -> String {
    let mut y = String::new();
    writeln!(y, "apiVersion: apps/v1").unwrap();
    writeln!(y, "kind: Deployment").unwrap();
    writeln!(y, "metadata:").unwrap();
    writeln!(y, "  name: {}", spec.name).unwrap();
    writeln!(y, "  namespace: {}", spec.namespace).unwrap();
    writeln!(y, "  labels:").unwrap();
    writeln!(y, "    app.kubernetes.io/name: {}", spec.name).unwrap();
    for (k, v) in &spec.labels {
        writeln!(y, "    {k}: {v}").unwrap();
    }
    if !spec.annotations.is_empty() {
        writeln!(y, "  annotations:").unwrap();
        for (k, v) in &spec.annotations {
            writeln!(y, "    {k}: \"{v}\"").unwrap();
        }
    }
    writeln!(y, "spec:").unwrap();
    writeln!(y, "  replicas: {}", spec.replicas).unwrap();
    writeln!(y, "  selector:").unwrap();
    writeln!(y, "    matchLabels:").unwrap();
    writeln!(y, "      app.kubernetes.io/name: {}", spec.name).unwrap();
    writeln!(y, "  template:").unwrap();
    writeln!(y, "    metadata:").unwrap();
    writeln!(y, "      labels:").unwrap();
    writeln!(y, "        app.kubernetes.io/name: {}", spec.name).unwrap();
    for (k, v) in &spec.labels {
        writeln!(y, "        {k}: {v}").unwrap();
    }
    writeln!(y, "    spec:").unwrap();
    writeln!(y, "      containers:").unwrap();
    writeln!(y, "        - name: {}", spec.name).unwrap();
    writeln!(y, "          image: {}", spec.image).unwrap();
    if !spec.ports.is_empty() {
        writeln!(y, "          ports:").unwrap();
        for p in &spec.ports {
            writeln!(y, "            - name: {}", p.name).unwrap();
            writeln!(y, "              containerPort: {}", p.port).unwrap();
            let proto = match p.protocol { Protocol::Tcp => "TCP", Protocol::Udp => "UDP" };
            writeln!(y, "              protocol: {proto}").unwrap();
        }
    }
    if !spec.env_vars.is_empty() {
        writeln!(y, "          env:").unwrap();
        for ev in &spec.env_vars {
            writeln!(y, "            - name: \"{}\"", ev.key).unwrap();
            writeln!(y, "              value: \"{}\"", ev.value).unwrap();
        }
    }
    writeln!(y, "          resources:").unwrap();
    writeln!(y, "            requests:").unwrap();
    writeln!(y, "              cpu: \"{}\"", spec.resources.requests.cpu).unwrap();
    writeln!(y, "              memory: \"{}\"", spec.resources.requests.memory).unwrap();
    if let Some(limits) = &spec.resources.limits {
        writeln!(y, "            limits:").unwrap();
        writeln!(y, "              cpu: \"{}\"", limits.cpu).unwrap();
        writeln!(y, "              memory: \"{}\"", limits.memory).unwrap();
    }
    write_probe(&mut y, "livenessProbe", &spec.health_checks.liveness);
    if let Some(readiness) = &spec.health_checks.readiness {
        write_probe(&mut y, "readinessProbe", readiness);
    }
    if let Some(startup) = &spec.health_checks.startup {
        write_probe(&mut y, "startupProbe", startup);
    }
    if let Some(volumes) = &spec.volumes {
        if !volumes.is_empty() {
            writeln!(y, "          volumeMounts:").unwrap();
            for vol in volumes {
                writeln!(y, "            - name: {}", vol.name).unwrap();
                writeln!(y, "              mountPath: {}", vol.mount_path).unwrap();
            }
        }
    }
    if let Some(secrets) = &spec.secrets {
        for secret in secrets {
            if let Some(prefix) = &secret.env_prefix {
                writeln!(y, "          envFrom:").unwrap();
                writeln!(y, "            - prefix: {prefix}").unwrap();
                writeln!(y, "              secretRef:").unwrap();
                writeln!(y, "                name: {}", secret.name).unwrap();
            }
        }
    }
    if let Some(volumes) = &spec.volumes {
        if !volumes.is_empty() {
            writeln!(y, "      volumes:").unwrap();
            for vol in volumes {
                writeln!(y, "        - name: {}", vol.name).unwrap();
                match vol.volume_type {
                    VolumeType::Configmap => {
                        writeln!(y, "          configMap:").unwrap();
                        writeln!(y, "            name: {}", vol.source).unwrap();
                    }
                    VolumeType::Secret => {
                        writeln!(y, "          secret:").unwrap();
                        writeln!(y, "            secretName: {}", vol.source).unwrap();
                    }
                    VolumeType::Pvc => {
                        writeln!(y, "          persistentVolumeClaim:").unwrap();
                        writeln!(y, "            claimName: {}", vol.source).unwrap();
                    }
                    VolumeType::Emptydir => {
                        writeln!(y, "          emptyDir: {{}}").unwrap();
                    }
                }
            }
        }
    }
    y
}

fn write_probe(y: &mut String, name: &str, probe: &Probe) {
    writeln!(y, "          {name}:").unwrap();
    if let Some(http) = &probe.http {
        writeln!(y, "            httpGet:").unwrap();
        writeln!(y, "              path: {}", http.path).unwrap();
        writeln!(y, "              port: {}", http.port).unwrap();
    } else if let Some(tcp) = &probe.tcp {
        writeln!(y, "            tcpSocket:").unwrap();
        writeln!(y, "              port: {}", tcp.port).unwrap();
    }
    writeln!(y, "            initialDelaySeconds: {}", probe.initial_delay).unwrap();
    writeln!(y, "            periodSeconds: {}", probe.period).unwrap();
    writeln!(y, "            timeoutSeconds: {}", probe.timeout).unwrap();
    writeln!(y, "            failureThreshold: {}", probe.failure_threshold).unwrap();
}

fn generate_service(spec: &Spec) -> Option<String> {
    let external_ports: Vec<&Port> = spec.ports.iter().filter(|p| p.external).collect();
    if external_ports.is_empty() { return None; }
    let mut y = String::new();
    writeln!(y, "apiVersion: v1").unwrap();
    writeln!(y, "kind: Service").unwrap();
    writeln!(y, "metadata:").unwrap();
    writeln!(y, "  name: {}", spec.name).unwrap();
    writeln!(y, "  namespace: {}", spec.namespace).unwrap();
    writeln!(y, "  labels:").unwrap();
    writeln!(y, "    app.kubernetes.io/name: {}", spec.name).unwrap();
    writeln!(y, "spec:").unwrap();
    writeln!(y, "  type: ClusterIP").unwrap();
    writeln!(y, "  selector:").unwrap();
    writeln!(y, "    app.kubernetes.io/name: {}", spec.name).unwrap();
    writeln!(y, "  ports:").unwrap();
    for p in &external_ports {
        writeln!(y, "    - name: {}", p.name).unwrap();
        writeln!(y, "      port: {}", p.port).unwrap();
        writeln!(y, "      targetPort: {}", p.port).unwrap();
        let proto = match p.protocol { Protocol::Tcp => "TCP", Protocol::Udp => "UDP" };
        writeln!(y, "      protocol: {proto}").unwrap();
    }
    Some(y)
}

fn generate_ingress(spec: &Spec) -> Option<String> {
    let ingress = spec.ingress.as_ref()?;
    let http_port = spec.ports.iter().find(|p| p.external)?;
    let mut y = String::new();
    writeln!(y, "apiVersion: networking.k8s.io/v1").unwrap();
    writeln!(y, "kind: Ingress").unwrap();
    writeln!(y, "metadata:").unwrap();
    writeln!(y, "  name: {}", spec.name).unwrap();
    writeln!(y, "  namespace: {}", spec.namespace).unwrap();
    if !ingress.annotations.is_empty() {
        writeln!(y, "  annotations:").unwrap();
        for (k, v) in &ingress.annotations { writeln!(y, "    {k}: \"{v}\"").unwrap(); }
    }
    writeln!(y, "spec:").unwrap();
    if ingress.tls {
        writeln!(y, "  tls:").unwrap();
        writeln!(y, "    - hosts:").unwrap();
        writeln!(y, "        - {}", ingress.host).unwrap();
        writeln!(y, "      secretName: {}-tls", spec.name).unwrap();
    }
    writeln!(y, "  rules:").unwrap();
    writeln!(y, "    - host: {}", ingress.host).unwrap();
    writeln!(y, "      http:").unwrap();
    writeln!(y, "        paths:").unwrap();
    writeln!(y, "          - path: {}", ingress.path).unwrap();
    writeln!(y, "            pathType: Prefix").unwrap();
    writeln!(y, "            backend:").unwrap();
    writeln!(y, "              service:").unwrap();
    writeln!(y, "                name: {}", spec.name).unwrap();
    writeln!(y, "                port:").unwrap();
    writeln!(y, "                  number: {}", http_port.port).unwrap();
    Some(y)
}

fn generate_hpa(spec: &Spec) -> Option<String> {
    let auto = spec.autoscaling.as_ref()?;
    let mut y = String::new();
    writeln!(y, "apiVersion: autoscaling/v2").unwrap();
    writeln!(y, "kind: HorizontalPodAutoscaler").unwrap();
    writeln!(y, "metadata:").unwrap();
    writeln!(y, "  name: {}", spec.name).unwrap();
    writeln!(y, "  namespace: {}", spec.namespace).unwrap();
    writeln!(y, "spec:").unwrap();
    writeln!(y, "  scaleTargetRef:").unwrap();
    writeln!(y, "    apiVersion: apps/v1").unwrap();
    writeln!(y, "    kind: Deployment").unwrap();
    writeln!(y, "    name: {}", spec.name).unwrap();
    writeln!(y, "  minReplicas: {}", auto.min_replicas).unwrap();
    writeln!(y, "  maxReplicas: {}", auto.max_replicas).unwrap();
    writeln!(y, "  metrics:").unwrap();
    writeln!(y, "    - type: Resource").unwrap();
    writeln!(y, "      resource:").unwrap();
    writeln!(y, "        name: cpu").unwrap();
    writeln!(y, "        target:").unwrap();
    writeln!(y, "          type: Utilization").unwrap();
    writeln!(y, "          averageUtilization: {}", auto.target_cpu).unwrap();
    Some(y)
}

fn generate_network_policy(spec: &Spec) -> String {
    let mut y = String::new();
    writeln!(y, "apiVersion: networking.k8s.io/v1").unwrap();
    writeln!(y, "kind: NetworkPolicy").unwrap();
    writeln!(y, "metadata:").unwrap();
    writeln!(y, "  name: {}-default", spec.name).unwrap();
    writeln!(y, "  namespace: {}", spec.namespace).unwrap();
    writeln!(y, "spec:").unwrap();
    writeln!(y, "  podSelector:").unwrap();
    writeln!(y, "    matchLabels:").unwrap();
    writeln!(y, "      app.kubernetes.io/name: {}", spec.name).unwrap();
    writeln!(y, "  policyTypes:").unwrap();
    writeln!(y, "    - Ingress").unwrap();
    writeln!(y, "  ingress:").unwrap();
    writeln!(y, "    - ports:").unwrap();
    for p in &spec.ports {
        let proto = match p.protocol { Protocol::Tcp => "TCP", Protocol::Udp => "UDP" };
        writeln!(y, "        - port: {}", p.port).unwrap();
        writeln!(y, "          protocol: {proto}").unwrap();
    }
    y
}

fn generate_service_monitor(spec: &Spec) -> Option<String> {
    let metrics_port = spec.ports.iter().find(|p| p.name == "metrics")?;
    let mut y = String::new();
    writeln!(y, "apiVersion: monitoring.coreos.com/v1").unwrap();
    writeln!(y, "kind: ServiceMonitor").unwrap();
    writeln!(y, "metadata:").unwrap();
    writeln!(y, "  name: {}", spec.name).unwrap();
    writeln!(y, "  namespace: {}", spec.namespace).unwrap();
    writeln!(y, "  labels:").unwrap();
    writeln!(y, "    app.kubernetes.io/name: {}", spec.name).unwrap();
    writeln!(y, "spec:").unwrap();
    writeln!(y, "  selector:").unwrap();
    writeln!(y, "    matchLabels:").unwrap();
    writeln!(y, "      app.kubernetes.io/name: {}", spec.name).unwrap();
    writeln!(y, "  endpoints:").unwrap();
    writeln!(y, "    - port: {}", metrics_port.name).unwrap();
    writeln!(y, "      interval: 15s").unwrap();
    writeln!(y, "      path: /metrics").unwrap();
    Some(y)
}

// ---------------------------------------------------------------------------
// Command implementations (now async)
// ---------------------------------------------------------------------------

impl CommandHandler for K8sCommands {
    async fn prepare(&self, spec: &Spec, _input: PrepareInput) -> Result<PrepareOutput, forest_sdk::Error> {
        let mut manifests = Vec::new();
        manifests.push(generate_deployment(spec));
        if let Some(svc) = generate_service(spec) { manifests.push(svc); }
        if let Some(ing) = generate_ingress(spec) { manifests.push(ing); }
        if let Some(hpa) = generate_hpa(spec) { manifests.push(hpa); }
        Ok(PrepareOutput { manifests })
    }

    async fn status(&self, spec: &Spec, _input: StatusInput) -> Result<StatusOutput, forest_sdk::Error> {
        Ok(StatusOutput { ready: spec.replicas, desired: spec.replicas, healthy: true, age: "3d12h".to_string() })
    }

    async fn validate(&self, spec: &Spec, _input: ValidateInput) -> Result<ValidateOutput, forest_sdk::Error> {
        let mut errors = Vec::new();
        if spec.name.is_empty() { errors.push("name must not be empty".to_string()); }
        if spec.image.is_empty() { errors.push("image must not be empty".to_string()); }
        if spec.ports.is_empty() { errors.push("at least one port is required".to_string()); }
        if let Some(auto) = &spec.autoscaling {
            if auto.max_replicas < auto.min_replicas { errors.push("autoscaling.max_replicas must be >= min_replicas".to_string()); }
        }
        if let Some(ingress) = &spec.ingress {
            if ingress.host.is_empty() { errors.push("ingress.host must not be empty".to_string()); }
            if !spec.ports.iter().any(|p| p.external) { errors.push("ingress requires at least one external port".to_string()); }
        }
        Ok(ValidateOutput { valid: errors.is_empty(), errors })
    }

    async fn diff(&self, spec: &Spec, _input: DiffInput) -> Result<DiffOutput, forest_sdk::Error> {
        Ok(DiffOutput { changes: vec![Change { resource: format!("Deployment/{}", spec.name), kind: Kind::Modify, diff: format!("replicas: ? -> {}", spec.replicas) }] })
    }

    async fn logs(&self, spec: &Spec, input: LogsInput) -> Result<LogsOutput, forest_sdk::Error> {
        eprintln!("Would tail {} lines from {}/{} container={}", input.lines, spec.namespace, spec.name, if input.container.is_empty() { &spec.name } else { &input.container });
        Ok(LogsOutput {})
    }
}

// ---------------------------------------------------------------------------
// Hook implementations (now async)
// ---------------------------------------------------------------------------

impl ForestDeploymentHookHandler for DeploymentHooks {
    async fn prepare(&self, spec: &Spec, _input: ForestDeploymentPrepareInput) -> Result<ForestDeploymentPrepareOutput, forest_sdk::Error> {
        let mut manifests = Vec::new();
        manifests.push(generate_deployment(spec));
        if let Some(svc) = generate_service(spec) { manifests.push(svc); }
        if let Some(ing) = generate_ingress(spec) { manifests.push(ing); }
        if let Some(hpa) = generate_hpa(spec) { manifests.push(hpa); }
        eprintln!("deployment/prepare: generated {} manifests for '{}'", manifests.len(), spec.name);
        Ok(ForestDeploymentPrepareOutput { manifests })
    }

    async fn release(&self, spec: &Spec, input: ForestDeploymentReleaseInput) -> Result<ForestDeploymentReleaseOutput, forest_sdk::Error> {
        eprintln!("deployment/release: applying '{}' (release_id={})", spec.name, input.release_id);
        Ok(ForestDeploymentReleaseOutput {})
    }

    async fn rollback(&self, spec: &Spec, input: ForestDeploymentRollbackInput) -> Result<(), forest_sdk::Error> {
        let target = if input.target_revision.is_empty() { "previous".to_string() } else { format!("revision {}", input.target_revision) };
        eprintln!("deployment/rollback: rolling back '{}' to {} (release_id={})", spec.name, target, input.release_id);
        Ok(())
    }
}

impl ForestObservabilityHookHandler for ObservabilityHooks {
    async fn configure_monitoring(&self, spec: &Spec, _input: ForestObservabilityConfigureMonitoringInput) -> Result<ForestObservabilityConfigureMonitoringOutput, forest_sdk::Error> {
        if let Some(monitor_yaml) = generate_service_monitor(spec) {
            eprintln!("observability/configure_monitoring: generated ServiceMonitor ({} bytes)", monitor_yaml.len());
        }
        Ok(ForestObservabilityConfigureMonitoringOutput {})
    }

    async fn configure_logging(&self, spec: &Spec, input: ForestObservabilityConfigureLoggingInput) -> Result<ForestObservabilityConfigureLoggingOutput, forest_sdk::Error> {
        eprintln!("observability/configure_logging: log_level={} for '{}'", input.log_level, spec.name);
        Ok(ForestObservabilityConfigureLoggingOutput {})
    }
}

impl ForestSecurityHookHandler for SecurityHooks {
    async fn scan_image(&self, spec: &Spec, _input: ForestSecurityScanImageInput) -> Result<ForestSecurityScanImageOutput, forest_sdk::Error> {
        eprintln!("security/scan_image: scanning '{}'", spec.image);
        Ok(ForestSecurityScanImageOutput { vulnerabilities: 3, critical: 0, passed: true })
    }

    async fn apply_policies(&self, spec: &Spec, _input: ForestSecurityApplyPoliciesInput) -> Result<ForestSecurityApplyPoliciesOutput, forest_sdk::Error> {
        let policy = generate_network_policy(spec);
        eprintln!("security/apply_policies: generated NetworkPolicy ({} bytes)", policy.len());
        Ok(ForestSecurityApplyPoliciesOutput {})
    }
}

fn main() {
    let router = ComponentRouter::new(K8sCommands, DeploymentHooks, ObservabilityHooks, SecurityHooks);
    forest_sdk::run_once(&router);
}
