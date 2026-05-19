project: {
	name:         "forage-client"
	organisation: "forage"
}

commands: {
	dev:     ["cargo run -p forage-server"]
	build:   ["cargo build --release -p forage-server"]
	compile: ["cargo build --release"]
	test:    ["cargo test --workspace"]
	check:   ["cargo check --workspace", "cargo clippy --workspace -- -D warnings"]
	fmt:     ["cargo fmt"]
	"fmt:check": ["cargo fmt -- --check"]

	"docker:build": [
		"docker build -f templates/forage-server.Dockerfile -t forage/forage-server:dev .",
	]
}
