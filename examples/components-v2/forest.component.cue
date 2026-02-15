package component_v2

#Component: #ForestComponent & {
	name:    "ecs-service"
	org:     "forest-contrib"
	version: "0.1.0"

	codegen: {
		type:   "rust"
		output: "./crates/ecs-service/src/"
	}
}

// --- Input spec: what callers must/can provide ---
#Spec: #ForestSpec & {
	name:  string & =~"^[a-z][a-z0-9-]*$"
	image: string
	ports: [...#Port] | *[]
	cpu:         #CPU | *256
	memory:      #Memory | *512
	replicas:    int & >=1 & <=100 | *1
	environment: "dev" | "staging" | "prod"

	health_check: #HealthCheck | *{
		path:     "/health"
		interval: 30
	}
}

// --- Commands ---
#Commands: #ForestCommands & {
	prepare: {
		description: "Generate ECS task definition and service manifests"
		input: {}
		output: {}
	}
	status: {
		description: "Check service health and running count"
		input: {}
		output: {
			running: int
			desired: int
			healthy: bool
		}
	}
}

// --- Lifecycle hooks ---
#Hooks: #ForestHooks & {
	"forest/deployment": #ForestHook & {
		prepare: #Commands.prepare
		release: {
			description: "Deploy to ECS"
			input: {release_id: string}
			output: {}
		}
		rollback: {
			description: "Roll back to previous task definition"
			input: {
				name:        #Spec.name
				environment: #Spec.environment
				release_id:  string
			}
		}
	}
}

// --- Type definitions ---
#Port: {
	name:     string
	port:     int & >0 & <=65535
	protocol: "tcp" | "udp" | *"tcp"
	external: bool | *false
}

#CPU: 256 | 512 | 1024 | 2048 | 4096

#Memory: 512 | 1024 | 2048 | 4096 | 8192

#HealthCheck: {
	path:     string | *"/health"
	interval: int & >=5 & <=300 | *30
	timeout:  int & >=2 & <=60 | *5
	retries:  int & >=1 & <=10 | *3
}
