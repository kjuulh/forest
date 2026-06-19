module: "forest.sh/forest-contrib/terraform-service@v0"
language: {
	version: "v0.16.1"
}
source: {
	kind: "self"
}
deps: {
	"forest.sh/forest/sdk@v0": {
		v: "v0.7.0"
	}
	"forest.sh/forest/deployment@v0": {
		v: "v0.3.0"
	}
}
