// consumer-example.cue
//
// This file is NOT part of the component itself. It shows how a consuming
// project would reference the kubernetes-service component in its own
// forest.cue to deploy a real application.
//
// In a real project this would live at: my-api/forest.cue

package consumer_example

project: {
	name:         "my-api"
	organisation: "rawpotion"
}

_destinationTypes: {
	kubernetes: "forest/kubernetes@1"
}

dependencies: {
	"forest-contrib/kubernetes-service": version: "0.1.0"
}

"forest-contrib": "kubernetes-service": {
	// --- Per-environment overrides ---
	env: {
		dev: {
			destinations: [
				{destination: "k8s-dev.*", type: _destinationTypes.kubernetes},
			]
			config: {
				replicas: 1
				resources: requests: {cpu: "100m", memory: "128Mi"}
				env_vars: [
					{key: "LOG_LEVEL", value: "debug"},
					{key: "OTEL_EXPORTER", value: "stdout"},
				]
			}
		}

		staging: {
			destinations: [
				{destination: "k8s-staging.*", type: _destinationTypes.kubernetes},
			]
			config: {
				replicas: 2
				resources: {
					requests: {cpu: "250m", memory: "256Mi"}
					limits: {cpu: "500m", memory: "512Mi"}
				}
				env_vars: [
					{key: "LOG_LEVEL", value: "info"},
					{key: "OTEL_EXPORTER", value: "otlp"},
				]
				autoscaling: {
					min_replicas: 2
					max_replicas: 5
					target_cpu:   80
				}
			}
		}

		prod: {
			destinations: [
				{destination: "k8s-prod.*", type: _destinationTypes.kubernetes},
			]
			config: {
				replicas: 5
				resources: {
					requests: {cpu: "500m", memory: "512Mi"}
					limits: {cpu: "1000m", memory: "1Gi"}
				}
				env_vars: [
					{key: "LOG_LEVEL", value: "warn"},
					{key: "OTEL_EXPORTER", value: "otlp"},
				]
				autoscaling: {
					min_replicas: 3
					max_replicas: 20
					target_cpu:   70
				}
				ingress: {
					host: "api.example.com"
					tls:  true
					annotations: {
						"nginx.ingress.kubernetes.io/rate-limit": "100"
					}
				}
				volumes: [
					{name: "config", volume_type: "configmap", source: "my-api-config", mount_path: "/etc/my-api"},
				]
				secrets: [
					{name: "db-credentials", env_prefix: "DB_"},
					{name: "tls-cert", mount_path: "/etc/tls"},
				]
				service_mesh: {
					enabled: true
					mtls:    true
				}
			}
		}
	}

	// --- Base config (shared across all environments) ---
	config: {
		name:      "my-api"
		namespace: "services"
		image:     "registry.example.com/my-api:latest"
		ports: [
			{name: "http", port: 8080, external: true},
			{name: "metrics", port: 9090},
			{name: "health", port: 8081},
		]
		health_checks: {
			liveness: {
				http: {path: "/healthz", port: 8081}
				initial_delay: 15
				period:        10
			}
			readiness: {
				http: {path: "/readyz", port: 8081}
				initial_delay: 5
				period:        5
			}
		}
		labels: {
			team:    "platform"
			tier:    "backend"
			product: "core-api"
		}
		annotations: {
			"prometheus.io/scrape": "true"
			"prometheus.io/port":   "9090"
		}
	}
}

commands: {
	dev:     ["cargo run -p my-api"]
	compile: ["cargo build -p my-api --release"]
	test:    ["cargo test -p my-api"]
}
