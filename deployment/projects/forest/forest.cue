package forest

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "forest"
	organisation: "rawpotion"
}

_destinationTypes: {
	flux: "forest/flux@1"
}

dependencies: sdk.#ForestDependencies & {
	"forest/deployment": version:       "0.3.0"
	"kjuulh/service": path:             "../../components/kjuulh/service"
	"kjuulh/sealed-secrets": path:      "../../components/kjuulh/sealed-secrets"
	"kjuulh/forage-postgresql": path:   "../../components/kjuulh/forage-postgresql"
	"kjuulh/forage-nats": path:         "../../components/kjuulh/forage-nats"
	"kjuulh/forage-s3": path:           "../../components/kjuulh/forage-s3"
}

forest: deployment: enabled: true

kjuulh: service: sdk.#ForestComponentUsage & {
	// Per-environment overrides. Everything env-specific (public host
	// and the URLs the server advertises about itself) lives here and is
	// deep-merged over the base `config` below by `forest release
	// prepare` — scalars in an env override the base, maps merge key by
	// key. Shared, env-independent settings stay in `config`.
	env: {
		dev: {
			destinations: [
				{destination: "flux-dev.*", type: _destinationTypes.flux},
			]
			config: {
				host: "forest.dev.forage.sh"
				env_vars: {
					EXTERNAL_HOST:                     "https://forest.dev.forage.sh"
					FOREST_TERRAFORM_V1_EXTERNAL_HOST: "https://forest.dev.forage.sh"
				}
			}
		}

		prod: {
			destinations: [
				{destination: "flux-prod.*", type: _destinationTypes.flux},
			]
			config: {
				host: "forest.forage.sh"
				env_vars: {
					EXTERNAL_HOST:                     "https://forest.forage.sh"
					FOREST_TERRAFORM_V1_EXTERNAL_HOST: "https://forest.forage.sh"
				}
			}
		}
	}

	config: {
		name:  "forest"
		image: "git.kjuulh.io/kjuulh/forest"
		tag:   "latest"
		port:  4040
		replicas: 3

		port_name: "grpc"

		args: ["serve"]

		health_binary: "forest-server"
		health_host:   "0.0.0.0"

		env_vars: {
			FOREST_HOST: "0.0.0.0:4040"
			LOG_LEVEL:   "short"
			// EXTERNAL_HOST / FOREST_TERRAFORM_V1_EXTERNAL_HOST are this
			// forest instance's own public face — set per-environment in the
			// `env` block above. Previously they pointed at
			// forest.i.kjuulh.io, which is the *bootstrap* forest server in
			// another cluster — having this instance advertise that URL
			// caused agents/callers to be redirected at the wrong server.
			//
			// FOREST_SERVICE_ACCOUNT_API_KEY: now in the forest-secrets
			// sealed secret. The service template envFroms forest-secrets,
			// so the variable still reaches the container — just no
			// longer as plaintext in the manifest. Forage's sealed-secret
			// holds the matching value on the client side.
			//
			// NATS_URL / S3_ENDPOINT / S3_BUCKET / S3_REGION: injected by
			// the kjuulh/service template via secretKeyRef into the
			// respective feature secrets (forage-nats-controller writes
			// NATS_URL alongside the creds; forage-s3-controller exposed
			// endpoint/region/bucket-name).
		}

		forage_postgresql: {}

		forage_nats: {
			publish: [
				{subject: "forest.>"},
			]
			subscribe: [
				{subject: "forest.>"},
				{subject: "_INBOX.>"},
			]
		}

		forage_s3: {}
	}
}

commands: sdk.#ForestProjectCommands & {}
