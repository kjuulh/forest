project: name: "service-example"

#basePath: "../../components"

dependencies: {
	"non/deployment": path:                      "\(#basePath)/non/deployment"
	"non-contrib/postgres": path:                "\(#basePath)/non-contrib/postgres"
	"non-contrib/rust-persistent-service": path: "\(#basePath)/non-contrib/rust_persistent_service"
}

non: deployment: enabled: true

#destinationTypes: {
	kubernetes: "non/kubernetes@1"
	terraform:  "non/terraform@1"
}

"non-contrib": "rust-persistent-service": {
	env: {
		dev: {
			destinations: [
				{destination: "dev-k8s-*", type: #destinationTypes.kubernetes},
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
				{destination: "prod-k8s-*", type: #destinationTypes.kubernetes},
				{destination: "eu-west-1-*", type: #destinationTypes.terraform},
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
