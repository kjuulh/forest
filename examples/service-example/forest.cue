project: {
	name:         "service-example"
	organisation: "rawpotion"
}

_basePath: "../../components"
_destinationTypes: {
	kubernetes: "forest/kubernetes@1"
	terraform:  "forest/terraform@1"
}

dependencies: {
	"forest/deployment": path:                      "\(_basePath)/forest/deployment"
	"forest-contrib/postgres": path:                "\(_basePath)/forest-contrib/postgres"
	"forest-contrib/rust-persistent-service": path: "\(_basePath)/forest-contrib/rust_persistent_service"
}

forest: deployment: enabled: true

"forest-contrib": "rust-persistent-service": {
	env: {
		dev: {
			destinations: [
				{destination: "infrastructure-dev.*", type: _destinationTypes.terraform},
			]
			config: {
				replicas: 2
				environment: [
					{key: "RUST_LOG", value: "debug"},
				]
			}
		}

		staging: {
			destinations: [
				{destination: "infrastructure-staging.*", type: _destinationTypes.terraform},
			]
			config: {
				replicas: 3
				environment: [
					{key: "RUST_LOG", value: "info"},
				]
			}
		}

		prod: {
			destinations: [
				{destination: "infrastructure-prod.*", type: _destinationTypes.terraform},
			]
			config: {
				replicas: 5
				environment: [
					{key: "RUST_LOG", value: "warn"},
				]
			}
		}}

	config: {
		name: "service-example"
		ports: [
			{name: "external", port: 3000, external: true},
			{name: "internal", port: 3001},
			{name: "grpc_external", port: 4000, external: true, subdomain: "grpc"},
			{name: "grpc_internal", port: 4001},
		]
		health_checks: live: http: {
			path: "/"
			port: 3001
		}
		environment: [
			{key: "RUST_LOG", value: "my_service=debug,info"},
		]
	}
}

commands: dev: ["cargo run"]
