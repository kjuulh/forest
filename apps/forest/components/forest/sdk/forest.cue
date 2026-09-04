package sdk

project: #ForestProject & {
	name:         "sdk"
	organisation: "forest"
	description:  "Base CUE types and contracts every Forest component imports — #ForestProject, #ForestComponent, command/hook schemas."
	metadata: {
		domain: "forest"
		owner:  "forest"
	}
}

forest: component: {
	name: "sdk"
	// 0.9.0 published without a CUE module: the packager wrote cue.mod/module.cue
	// twice once components started shipping their own, the zip writer refused the
	// duplicate, and the failure was logged as a warning rather than surfaced
	// (fixed in #225). Versions are immutable, so the fix needed a new one.
	// 0.9.2 adds `#ForestPaths` — `forest.component.paths.{include,exclude}`,
	// which the publish walker has always taken and had no way to be given.
	version: "0.9.2"
}
