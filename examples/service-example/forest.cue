project: name: "service-example"

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
				{destination: "k8s.*", type: _destinationTypes.kubernetes},
				{destination: "eu-west-1.*", type: _destinationTypes.terraform},
			]
			config: {
				replicas: 3
				environment: [
					{key: "RUST_LOG", value: "debug"},
				]
			}
		}

		prod: {
			destinations: [
				{destination: "k8s.*", type: _destinationTypes.kubernetes},
				{destination: "eu-west-1.*", type: _destinationTypes.terraform},
			]
			config: {
				replicas: 10
				environment: [{key: "RUST_LOG", value: "info"}]
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
