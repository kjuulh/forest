project: {
	name:         "rust-podinfo"
	organisation: "rawpotion"
}

_basePath:      "../../components"
_componentPath: "../rust-service-component"

_destinationTypes: {
	kubernetes: "forest/kubernetes@1"
	flux:       "forest/flux@1"
}

dependencies: {
	"forest/deployment": path:           "\(_basePath)/forest/deployment"
	"forest-contrib/rust-service": path: _componentPath
}

forest: deployment: enabled: true

"forest-contrib": "rust-service": {
	env: {
		dev: {
			destinations: [
				{destination: "k8s-dev.*", type: _destinationTypes.kubernetes},
				{destination: "flux-dev.*", type: _destinationTypes.flux},
			]
			config: {
				replicas: 1
				resources: {
					requests: {cpu: "100m", memory: "128Mi"}
					limits: {cpu: "250m", memory: "256Mi"}
				}
				environment: [
					{key: "RUST_LOG", value: "rust_podinfo=debug,info"},
					{key: "PODINFO_ENV", value: "dev"},
				]
			}
		}

		staging: {
			destinations: [
				{destination: "k8s-staging.*", type: _destinationTypes.kubernetes},
				{destination: "flux-staging.*", type: _destinationTypes.flux},
			]
			config: {
				replicas: 2
				resources: {
					requests: {cpu: "250m", memory: "256Mi"}
					limits: {cpu: "500m", memory: "512Mi"}
				}
				environment: [
					{key: "RUST_LOG", value: "rust_podinfo=info"},
					{key: "PODINFO_ENV", value: "staging"},
				]
			}
		}

		prod: {
			destinations: [
				{destination: "k8s-prod.*", type: _destinationTypes.kubernetes},
				{destination: "flux-prod.*", type: _destinationTypes.flux},
			]
			config: {
				replicas: 3
				resources: {
					requests: {cpu: "500m", memory: "512Mi"}
					limits: {cpu: "1000m", memory: "1024Mi"}
				}
				environment: [
					{key: "RUST_LOG", value: "rust_podinfo=warn,info"},
					{key: "PODINFO_ENV", value: "prod"},
				]
			}
		}
	}

	config: {
		name:  "rust-podinfo"
		image: "k3d-forest-test-registry:5000/rust-podinfo:test"
		ports: [
			{name: "http", port: 8080, external: true},
			{name: "internal", port: 8081},
		]
		health_checks: {
			liveness: {
				path: "/healthz"
				port: 8081
			}
			readiness: {
				path: "/readyz"
				port: 8081
			}
		}
		environment: []
	}
}

commands: {
	dev: ["cargo run -p rust-podinfo"]
	compile: ["cargo build -p rust-podinfo --release"]
}
