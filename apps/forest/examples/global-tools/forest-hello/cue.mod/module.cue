module: "cuteorg.example/forest-hello@v0"
language: {
	version: "v0.16.1"
}
source: {
	kind: "self"
}
deps: {
	// v0.8.0 is the first SDK carrying #ForestShellIntegration, which this
	// example's `include: shell: init` block needs — #ForestInclude is a closed
	// definition, so an older SDK rejects the field outright.
	"forest.sh/forest/sdk@v0": {
		v: "v0.8.0"
	}
}
