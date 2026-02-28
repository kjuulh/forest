package rust_service

// --- Input spec: what consuming projects must provide ---
#Spec: #ForestSpec & {
	name:  string & =~"^[a-z][a-z0-9-]*$"
	image: string

	ports:         [...#Port] | *[]
	replicas:      int & >=1 & <=100 | *1
	environment:   [...#EnvVar] | *[]
	resources:     #Resources | *{requests: {cpu: "100m", memory: "128Mi"}, limits: {cpu: "500m", memory: "256Mi"}}
	health_checks: #HealthChecks | *{liveness: {path: "/healthz", port: 8080}, readiness: {path: "/readyz", port: 8080}}
}

// --- Commands: actions the component can perform ---
#Commands: #ForestCommands & {
	build: {
		description: "Compile the release binary"
		input: {}
		output: {
			binary: string
		}
	}
	validate: {
		description: "Run clippy, fmt check, and cargo check"
		input: {}
		output: {
			valid:    bool
			messages: [...string]
		}
	}
	test: {
		description: "Run the test suite (nextest if available, else cargo test)"
		input: {}
		output: {
			passed: int
			failed: int
			total:  int
		}
	}
	"docker-build": {
		description: "Build a Docker container image for the service"
		input: {
			tag:      string
			registry: string | *""
		}
		output: {
			image: string
		}
	}
	status: {
		description: "Show build artifacts, git state, and deployment info"
		input: {}
		output: {
			binary_exists: bool
			git_branch:    string
			git_commit:    string
			git_dirty:     bool
		}
	}
}

// --- Lifecycle hooks: integration with forest/deployment ---
#Hooks: #ForestHooks & {
	"forest/deployment": #ForestHook & {
		prepare: {
			description: "Generate Kubernetes manifests for deployment"
			input: {}
			output: {
				manifests: [...string]
			}
		}
		release: {
			description: "Deploy the service to the target cluster"
			input: {
				release_id: string
			}
			output: {
				deployed: bool
			}
		}
		rollback: {
			description: "Roll back to a previous release"
			input: {
				name:       #Spec.name
				release_id: string
			}
			output: {}
		}
	}
}

// --- Domain types ---
#Port: {
	name:     string
	port:     int & >0 & <=65535
	protocol: "tcp" | "udp" | *"tcp"
	external: bool | *false
}

#EnvVar: {
	key:   string
	value: string
}

#Resources: {
	requests: {
		cpu:    string
		memory: string
	}
	limits: {
		cpu:    string
		memory: string
	}
}

#HealthChecks: {
	liveness: #HealthCheckProbe
	readiness: #HealthCheckProbe
}

#HealthCheckProbe: {
	path:                  string | *"/healthz"
	port:                  int & >0 & <=65535 | *8080
	initial_delay_seconds: int & >=0 & <=300 | *10
	period_seconds:        int & >=1 & <=300 | *10
	timeout_seconds:       int & >=1 & <=60 | *3
	failure_threshold:     int & >=1 & <=10 | *3
}
