module: "forest.sh/forest-contrib/ecs-service@v0"
language: {
	version: "v0.15.4"
}
source: {
	kind: "self"
}
deps: {
	"forest.sh/forest/sdk@v0": {
		v: "v0.3.0"
	}
	"forest.sh/forest/deployment@v0": {
		v: "v0.2.0"
	}
}
