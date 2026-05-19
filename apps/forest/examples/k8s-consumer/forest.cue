project: {
	name:         "my-api"
	organisation: "rawpotion"
}

_componentPath: "../components-v2"

dependencies: {
	"forest-contrib/kubernetes-service": path: _componentPath
}

forest: deployment: enabled: true

"forest-contrib": "kubernetes-service": {
	env: {
		dev: {
			destinations: [
				{destination: "k8s-dev-1", type: "forest/kubernetes@1"},
			]
			config: {
				replicas:    1
				environment: "dev"
				resources: requests: {cpu: "100m", memory: "128Mi"}
				env_vars: [
					{key: "LOG_LEVEL", value: "debug"},
				]
			}
		}
	}

	config: {
		name:        "my-api"
		namespace:   "services"
		image:       "registry.example.com/my-api:latest"
		environment: "dev"
		replicas:    1
		env_vars: []
		ports: [
			{name: "http", port: 8080, protocol: "tcp", external: true},
		]
		resources: requests: {cpu: "100m", memory: "128Mi"}
		health_checks: liveness: {
			http: {path: "/healthz", port: 8080}
			initial_delay:     10
			period:            10
			timeout:           3
			failure_threshold: 3
		}
		labels: {}
		annotations: {}
	}
}

commands: {
	dev:  ["echo 'running dev server'"]
	test: ["echo 'running tests'"]
}
