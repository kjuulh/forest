package build_rust_example

import "forest.sh/forest/sdk@v0"

project: sdk.#ForestProject & {
	name:         "build-rust-example"
	organisation: "examples"
}

// Depend on the rust build component. This is the whole point of DATA-312:
// building is a declared dependency, not a baked-in CLI command.
dependencies: {
	"forest-contrib/build-rust": path: "../../components/forest-contrib/build-rust"
}

// Usage block — surfaces the component's `commands/build` as `forest run build`.
"forest-contrib": "build-rust": {}

forest: component: sdk.#ForestComponent & {
	name:    project.name
	version: "0.1.0"
	upload: {
		source: "./crates/build-rust-example"
		type:   "rust"
		// `forest run build` compiles EVERY platform listed here. Add your
		// deploy targets (e.g. `linux: amd64: {}`); cross-compiling Rust may
		// need the rustup target + a cross linker installed.
		architectures: {
			macos: arm64: {}
		}
	}
}
