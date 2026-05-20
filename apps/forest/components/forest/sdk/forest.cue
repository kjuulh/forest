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
	name:    "sdk"
	version: "0.6.0"
}
