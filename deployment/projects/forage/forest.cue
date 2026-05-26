package forage

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "forage"
	organisation: "rawpotion"
}

_destinationTypes: {
	flux: "forest/flux@1"
}

dependencies: sdk.#ForestDependencies & {
	"forest/deployment": version:     "0.3.0"
	"kjuulh/service": path:           "../../components/kjuulh/service"
	"kjuulh/sealed-secrets": path:    "../../components/kjuulh/sealed-secrets"
	"kjuulh/forage-postgresql": path: "../../components/kjuulh/forage-postgresql"
	"kjuulh/forage-nats": path:       "../../components/kjuulh/forage-nats"
}

forest: deployment: enabled: true

kjuulh: service: sdk.#ForestComponentUsage & {
	// Per-environment overrides. The public host and the in-cluster
	// forest address (forest runs as svc/forest in the same namespace as
	// the release env) are env-specific and live here; they're
	// deep-merged over the base `config` below by `forest release
	// prepare`. Shared settings stay in `config`.
	env: {
		dev: {
			destinations: [
				{destination: "flux-dev.*", type: _destinationTypes.flux},
			]
			config: {
				host: "forage.dev.forage.sh"
				env_vars: {
					FOREST_SERVER_URL: "http://forest.dev:4040"
				}
			}
		}

		prod: {
			destinations: [
				{destination: "flux-prod.*", type: _destinationTypes.flux},
			]
			config: {
				host: "forage.forage.sh"
				env_vars: {
					FOREST_SERVER_URL: "http://forest.prod:4040"
				}
			}
		}
	}

	config: {
		name:     "forage"
		image:    "git.kjuulh.io/kjuulh/forage"
		tag:      "latest"
		port:     3000
		replicas: 3

		// No health_binary: forage-server has no admin subcommand and
		// the binary lives at /usr/local/bin/forage-server in the
		// distroless image. The deployment template omits the
		// liveness/readiness probes when health_binary isn't set.

		env_vars: {
			FORAGE_HOST: "0.0.0.0:3000"
			// FOREST_SERVER_URL is the in-cluster address of forest, which
			// runs as svc/forest in the same namespace as the release env
			// (dev → forest.dev, prod → forest.prod) — set per-environment
			// in the `env` block above. An earlier value `forest.forest:4040`
			// hit NXDOMAIN; there is no `forest` namespace in the cluster.
			LOG_LEVEL: "short"
		}

		forage_postgresql: {}

		// forage publishes durable notifications + emails to NATS JetStream.
		// Subject namespaces are from forage-core/integrations: notifications
		// live under `forage.notifications.>`, emails under `forage.email.>`.
		// Without this block the kjuulh/service template doesn't inject
		// NATS_URL / NATS_CREDS, and forage falls back to in-process
		// notification dispatch (no durability — visible in startup logs
		// as the `NATS_URL not set — using direct notification dispatch`
		// warning).
		forage_nats: {
			publish: [
				{subject: "forage.notifications.>"},
				{subject: "forage.email.>"},
				// JetStream stream/consumer management — without this the
				// server rejects with "Permissions Violation for Publish
				// to $JS.API.STREAM.INFO.<stream>".
				{subject: "$JS.API.>"},
			]
			subscribe: [
				{subject: "forage.notifications.>"},
				{subject: "forage.email.>"},
				{subject: "_INBOX.>"},
			]
		}
	}
}

commands: sdk.#ForestProjectCommands & {}
