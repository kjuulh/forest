package kubernetes_service

import "forest.sh/forest/sdk@v0"

// --- Input spec: what consuming projects must provide ---
#Spec: sdk.#ForestSpec & {
	name:        string & =~"^[a-z][a-z0-9-]*$"
	namespace:   string | *"default"
	image:       string
	environment: "dev" | "staging" | "prod"

	// Core scaling & compute
	replicas:  int & >=1 & <=100 | *1
	resources: #Resources

	// Networking
	ports: [...#Port]

	// Health probes
	health_checks: #HealthChecks

	// Environment variables
	env_vars: [...#EnvVar]

	// Labels & annotations (open maps)
	labels: {[string]:      string}
	annotations: {[string]: string}

	// Optional features — only generated when present
	autoscaling?: #Autoscaling
	ingress?:     #Ingress
	volumes?: [...#Volume]
	secrets?: [...#SecretRef]
	service_mesh?: #ServiceMesh
}

// --- Commands: actions the component can perform ---
#Commands: sdk.#ForestCommands & {
	prepare: {
		description: "Generate Kubernetes manifests (Deployment, Service, Ingress, HPA, etc.)"
		input: {}
		output: {
			manifests: [...string]
		}
	}
	status: {
		description: "Check deployment status against the cluster"
		input: {}
		output: {
			ready:   int
			desired: int
			healthy: bool
			age:     string
		}
	}
	validate: {
		description: "Validate spec and generated manifests"
		input: {}
		output: {
			valid:  bool
			errors: [...string]
		}
	}
	diff: {
		description: "Show diff of generated manifests against live cluster state"
		input: {}
		output: {
			changes: [...#Change]
		}
	}
	logs: {
		description: "Tail pod logs for the service"
		input: {
			lines:     int | *100
			container: string | *""
		}
		output: {}
	}
}

// --- Lifecycle hooks ---
#Hooks: sdk.#ForestHooks & {
	"forest/deployment": sdk.#ForestHook & {
		prepare: {
			description: "Generate and validate Kubernetes manifests for deployment"
			input: {}
			output: {
				manifests: [...string]
			}
		}
		release: {
			description: "Apply manifests to the target cluster"
			input: {
				release_id: string
			}
			output: {}
		}
		rollback: {
			description: "Roll back to a previous revision"
			input: {
				release_id:      string
				target_revision: string | *""
			}
		}
	}
	"forest/observability": sdk.#ForestHook & {
		configure_monitoring: {
			description: "Create or update ServiceMonitor and PrometheusRule resources"
			input: {}
			output: {}
		}
		configure_logging: {
			description: "Configure log collection pipeline for the service"
			input: {
				log_level: string | *"info"
			}
			output: {}
		}
	}
	"forest/security": sdk.#ForestHook & {
		scan_image: {
			description: "Run vulnerability scan on the container image"
			input: {}
			output: {
				vulnerabilities: int
				critical:        int
				passed:          bool
			}
		}
		apply_policies: {
			description: "Apply NetworkPolicy and PodSecurity resources"
			input: {}
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

#Resources: {
	requests: #ResourceQuantity
	limits?:  #ResourceQuantity
}

#ResourceQuantity: {
	cpu:    string
	memory: string
}

#HealthChecks: {
	liveness:  #Probe
	readiness?: #Probe
	startup?:   #Probe
}

#Probe: {
	http?: #HttpProbe
	tcp?:  #TcpProbe
	initial_delay: int & >=0 & <=300 | *10
	period:        int & >=1 & <=300 | *10
	timeout:       int & >=1 & <=60 | *3
	failure_threshold: int & >=1 & <=10 | *3
}

#HttpProbe: {
	path: string
	port: int & >0 & <=65535
}

#TcpProbe: {
	port: int & >0 & <=65535
}

#EnvVar: {
	key:   string
	value: string
}

#Autoscaling: {
	min_replicas: int & >=1 | *1
	max_replicas: int & >=1
	target_cpu:   int & >=1 & <=100 | *80
}

#Ingress: {
	host: string
	tls:  bool | *true
	path: string | *"/"
	annotations: {[string]: string}
}

#Volume: {
	name:        string
	volume_type: "configmap" | "secret" | "pvc" | "emptydir"
	source:      string
	mount_path:  string
}

#SecretRef: {
	name:        string
	mount_path?: string
	env_prefix?: string
}

#ServiceMesh: {
	enabled: bool | *false
	mtls:    bool | *true
}

#Change: {
	resource: string
	kind:     "add" | "modify" | "remove"
	diff:     string
}
