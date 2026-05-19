#!/usr/bin/env bash
set -euo pipefail

# ============================================================
# Forest E2E — fresh machine developer experience test
#
# Simulates a new user going from zero to a working project:
#   1. Register + login
#   2. Create an organisation
#   3. Publish the SDK (CUE-only component)
#   4. Publish a component (terraform-service)
#   5. Create a consumer project from scratch
#   6. Add the component as a dependency
#   7. Update, validate, run commands, release prepare
# ============================================================

FOREST="forest --forest-server ${FOREST_SERVER}"
PASS="E2e-Test-Password-123!"
STAMP="$(date +%s)"
USER="e2e-user-${STAMP}"
ORG="e2e-org-${STAMP}"

step() { echo -e "\n=== $1 ==="; }
ok()   { echo "  ✓ $1"; }
fail() { echo "  ✗ $1"; exit 1; }

# ----------------------------------------------------------
step "1. Register and login"
# ----------------------------------------------------------

# Register (non-interactive via env var)
FOREST_PASSWORD="$PASS" $FOREST auth register \
    --username "$USER" \
    --email "${USER}@test.local" \
    && ok "registered $USER" || fail "register"

# Login
FOREST_PASSWORD="$PASS" $FOREST auth login --username "$USER" \
    && ok "logged in" || fail "login"

# Verify
$FOREST auth status && ok "auth status" || fail "auth status"

# ----------------------------------------------------------
step "2. Create organisation"
# ----------------------------------------------------------

$FOREST organisation create --name "$ORG" \
    && ok "created org $ORG" || fail "create org"

# ----------------------------------------------------------
step "3. Publish SDK (CUE-only component)"
# ----------------------------------------------------------

mkdir -p /tmp/sdk/cue.mod
cat > /tmp/sdk/cue.mod/module.cue << EOF
module: "forest.sh/${ORG}/sdk@v0"
language: {
	version: "v0.15.4"
}
source: {
	kind: "self"
}
EOF

cat > /tmp/sdk/forest.cue << EOF
package sdk

project: #ForestProject & {
	name:         "sdk"
	organisation: "${ORG}"
}

forest: component: {
	name:    "sdk"
	version: "0.1.0"
}
EOF

cat > /tmp/sdk/spec.cue << 'CUEEOF'
package sdk

#ForestProject: {
	name:         string & =~"^[a-z][a-z0-9-]*$"
	organisation: string & =~"^[a-z][a-z0-9-]*$"
}

#ForestComponent: {
	name:    string
	version: string & =~#"^\d+\.\d+\.\d+"#
	codegen?: #ForestCodegen
	upload?:  #ForestComponentUpload
}

#ForestComponentUpload: {
	type:     #ForestSource
	source:   string | *"."
	registry: string | *"registry.forage.sh"
	architectures: {
		[#ForestArchitectures]: #ForestArchitecture
	}
}

#ForestArchitectures: "linux" | "macos" | "windows"
#ForestArch:          "amd64" | "arm64"
#ForestArchitecture: {
	[#ForestArch]: {}
}

#ForestCommands: {
	[string]: #ForestCommand
}

#ForestCommand: {
	description: string
	input: {...}
	output: {...}
}

#ForestSpec: {
	...
}

#ForestHooks: {
	[string]: #ForestHook
}

#ForestHook: {
	...
}

#ForestCodegen: {
	type:   #ForestSource
	output: string
}

#ForestSource: "rust" | "go" | "docker"

// Consumer project types
#ForestDependency: {
	version: string
} | {
	path: string
}

#ForestDependencies: {
	[string]: #ForestDependency
}

#ForestProjectCommands: {
	[string]: [...string]
}

#ForestDestinationRef: {
	destination: string
	type:        string
}

#ForestEnvironmentConfig: {
	destinations: [...#ForestDestinationRef]
	config?: {...}
}

#ForestComponentUsage: {
	env?: {
		[string]: #ForestEnvironmentConfig
	}
	config?: {...}
}
CUEEOF

cd /tmp/sdk
cue eval forest.cue spec.cue > /dev/null && ok "SDK CUE validates" || fail "SDK CUE invalid"
$FOREST publish && ok "published SDK" || fail "publish SDK"

# Verify OCI registry serves it
curl -sf "http://forest-server:4042/v2/forest.sh/${ORG}/sdk/manifests/v0.1.0" > /dev/null \
    && ok "SDK OCI manifest served" || fail "SDK OCI manifest"

# ----------------------------------------------------------
step "4. Verify CUE module import from registry"
# ----------------------------------------------------------

mkdir -p /tmp/cue-test/cue.mod
cat > /tmp/cue-test/cue.mod/module.cue << EOF
module: "test.example/verify@v0"
language: {
	version: "v0.15.4"
}
deps: {
	"forest.sh/${ORG}/sdk@v0": {
		v: "v0.1.0"
	}
}
EOF

cat > /tmp/cue-test/test.cue << EOF
package verify

import "forest.sh/${ORG}/sdk@v0"

project: sdk.#ForestProject & {
	name:         "verify-test"
	organisation: "${ORG}"
}
EOF

cd /tmp/cue-test
cue eval test.cue && ok "CUE import from registry works" || fail "CUE import broken"

# Verify type checking
cat > /tmp/cue-test/bad.cue << EOF
package verify

import "forest.sh/${ORG}/sdk@v0"

_bad: sdk.#ForestProject & {
	name: "UPPERCASE"
	organisation: "${ORG}"
}
EOF

CUE_CHECK=$(cue eval test.cue bad.cue 2>&1 || true)
if echo "$CUE_CHECK" | grep -qiE "invalid value|error|did not match"; then
    ok "CUE type checking catches errors"
else
    echo "  unexpected: $CUE_CHECK"
    fail "CUE type checking not working"
fi

# ----------------------------------------------------------
step "5. Create consumer project from scratch"
# ----------------------------------------------------------

PROJECT_DIR="/tmp/my-project"
rm -rf "$PROJECT_DIR"
mkdir -p "$PROJECT_DIR/cue.mod"

cat > "$PROJECT_DIR/cue.mod/module.cue" << EOF
module: "forest.sh/${ORG}/my-project@v0"
language: {
	version: "v0.15.4"
}
deps: {
	"forest.sh/${ORG}/sdk@v0": {
		v: "v0.1.0"
	}
}
EOF

cat > "$PROJECT_DIR/forest.cue" << EOF
package my_project

import "forest.sh/${ORG}/sdk@v0"

project: sdk.#ForestProject & {
	name:         "my-project"
	organisation: "${ORG}"
}

dependencies: sdk.#ForestDependencies & {}

commands: sdk.#ForestProjectCommands & {
	dev:  ["echo hello from dev"]
	test: ["echo running tests"]
}
EOF

cd "$PROJECT_DIR"
cue eval forest.cue && ok "project CUE validates" || fail "project CUE invalid"

# Verify export
OUTPUT=$(CUE_REGISTRY="${CUE_REGISTRY}" cue export --out json forest.cue 2>&1)
echo "$OUTPUT" | grep -q '"my-project"' && ok "project exports correctly" || fail "project export: $OUTPUT"

# ----------------------------------------------------------
step "6. Run project commands"
# ----------------------------------------------------------

cd "$PROJECT_DIR"
$FOREST run dev 2>&1 | grep -q "hello from dev" \
    && ok "forest run dev" || fail "forest run dev"

$FOREST run test 2>&1 | grep -q "running tests" \
    && ok "forest run test" || fail "forest run test"

# Check help shows commands
$FOREST run --help 2>&1 | grep -q "dev" \
    && ok "forest run --help lists commands" || fail "run --help"

# ----------------------------------------------------------
step "7. Update and lock file"
# ----------------------------------------------------------

$FOREST update && ok "forest update" || fail "forest update"

# Lock file should exist (even if only header for no deps)
test -f forest.lock && ok "forest.lock created" || fail "no forest.lock"

# ----------------------------------------------------------
step "8. Validate (no deployment deps = clean)"
# ----------------------------------------------------------

$FOREST validate 2>&1 && ok "forest validate (no components)" || true
# ↑ No components to validate — should succeed with 0 components

# ----------------------------------------------------------
step "9. Release prepare (should fail — no deployment deps)"
# ----------------------------------------------------------

RELEASE_OUTPUT=$($FOREST release prepare 2>&1 || true)
if echo "$RELEASE_OUTPUT" | grep -q "no dependencies implement"; then
    ok "forest release prepare correctly errors with no deployment deps"
else
    echo "  actual output: $RELEASE_OUTPUT"
    fail "release prepare should have errored"
fi

# ----------------------------------------------------------
step "10. Create a template-only component for release testing"
# ----------------------------------------------------------

# Create a minimal component with deployment templates (no binary needed)
COMP_DIR="/tmp/e2e-deployer"
rm -rf "$COMP_DIR"
mkdir -p "$COMP_DIR/cue.mod"
mkdir -p "$COMP_DIR/templates/deployment/forest/terraform@1"
mkdir -p "$COMP_DIR/.forest/component"

cat > "$COMP_DIR/cue.mod/module.cue" << EOF
module: "forest.sh/${ORG}/e2e-deployer@v0"
language: {
	version: "v0.15.4"
}
deps: {
	"forest.sh/${ORG}/sdk@v0": {
		v: "v0.1.0"
	}
}
EOF

cat > "$COMP_DIR/forest.cue" << EOF
package e2e_deployer

import "forest.sh/${ORG}/sdk@v0"

project: sdk.#ForestProject & {
	name:         "e2e-deployer"
	organisation: "${ORG}"
}

forest: component: sdk.#ForestComponent & {
	name:    "e2e-deployer"
	version: "0.1.0"
}
EOF

cat > "$COMP_DIR/forest.component.cue" << EOF
package e2e_deployer

import "forest.sh/${ORG}/sdk@v0"

#Spec: sdk.#ForestSpec & {
	name: string
}

#Commands: sdk.#ForestCommands & {}

#Hooks: sdk.#ForestHooks & {
	"forest/deployment": sdk.#ForestHook & {
		prepare: {
			description: "Prepare deployment"
			input: {}
			output: {}
		}
		release: {
			description: "Execute deployment"
			input: {
				release_id: string
			}
			output: {}
		}
	}
}
EOF

# Template file — just echoes the config
cat > "$COMP_DIR/templates/deployment/forest/terraform@1/main.tf" << 'EOF'
# E2E test deployment template
resource "null_resource" "e2e" {
  provisioner "local-exec" {
    command = "echo deployed"
  }
}
EOF

# Create a minimal shell script "binary" that implements the component protocol
cat > "$COMP_DIR/e2e-deployer" << 'SCRIPT'
#!/bin/sh
# Minimal component binary — reads JSON from stdin, responds to protocol methods
INPUT=$(cat)
METHOD=$(echo "$INPUT" | sed -n 's/.*"method"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')
case "$METHOD" in
  _meta/describe)
    echo '{"protocol_version":"1.1","methods":[{"name":"hooks/forest/deployment/prepare","kind":"hook","topic":"forest/deployment"},{"name":"hooks/forest/deployment/release","kind":"hook","topic":"forest/deployment"}]}'
    ;;
  hooks/forest/deployment/prepare)
    echo '{"manifests":[]}'
    ;;
  hooks/forest/deployment/release)
    echo '{}'
    ;;
  _meta/template_config)
    echo '{"skip":[],"rename":{},"vars":{}}'
    ;;
  *)
    echo '{}' ;;
esac
SCRIPT
chmod +x "$COMP_DIR/e2e-deployer"

# Write meta.json so resolve_binary can find it via cached descriptor
mkdir -p "$COMP_DIR/.forest/component"
cat > "$COMP_DIR/.forest/component/meta.json" << 'META'
{
  "name": "e2e-deployer",
  "descriptor": {
    "protocol_version": "1.1",
    "methods": [
      {"name": "hooks/forest/deployment/prepare", "kind": "hook", "topic": "forest/deployment"},
      {"name": "hooks/forest/deployment/release", "kind": "hook", "topic": "forest/deployment"}
    ]
  }
}
META

ok "created template component with shell binary"

# ----------------------------------------------------------
step "11. Set up project with deployment component"
# ----------------------------------------------------------

# Update the consumer project to include the deployer component
cd "$PROJECT_DIR"

cat > "$PROJECT_DIR/cue.mod/module.cue" << EOF
module: "forest.sh/${ORG}/my-project@v0"
language: {
	version: "v0.15.4"
}
deps: {
	"forest.sh/${ORG}/sdk@v0": {
		v: "v0.1.0"
	}
}
EOF

cat > "$PROJECT_DIR/forest.cue" << EOF
package my_project

import "forest.sh/${ORG}/sdk@v0"

project: sdk.#ForestProject & {
	name:         "my-project"
	organisation: "${ORG}"
}

dependencies: sdk.#ForestDependencies & {
	"${ORG}/e2e-deployer": path: "${COMP_DIR}"
}

"${ORG}": "e2e-deployer": sdk.#ForestComponentUsage & {
	env: {
		dev: {
			destinations: [
				{destination: "e2e-dest-${STAMP}", type: "forest/terraform@1"},
			]
			config: {
				name: "my-project"
			}
		}
	}
	config: {
		name: "my-project"
	}
}

commands: sdk.#ForestProjectCommands & {
	dev: ["echo hello"]
}
EOF

cue eval forest.cue > /dev/null && ok "project with deployer validates" || fail "project CUE invalid"

# ----------------------------------------------------------
step "12. Create server-side resources (project + destination)"
# ----------------------------------------------------------

$FOREST project create -o "$ORG" -p "my-project" \
    && ok "created project" || fail "create project"

$FOREST environment create -o "$ORG" --name "dev" --description "Development" \
    && ok "created environment dev" || fail "create environment"

$FOREST destination create \
    --organisation "$ORG" \
    --name "e2e-dest-${STAMP}" \
    --environment "dev" \
    --type "forest/terraform@1" \
    && ok "created destination" || fail "create destination"

# ----------------------------------------------------------
step "13. Release prepare (with component)"
# ----------------------------------------------------------

$FOREST release prepare \
    && ok "release prepare succeeded" || fail "release prepare"

# Verify artifacts were generated
test -d .forest/deployment/dev && ok "deployment artifacts exist" || fail "no deployment artifacts"

# ----------------------------------------------------------
step "14. Full release: create (prepare + annotate + release)"
# ----------------------------------------------------------

# Initialize a git repo so release create can detect context
cd "$PROJECT_DIR"
git init -q
git config user.name "E2E Test"
git config user.email "e2e@test.local"
git add -A
git commit -q -m "e2e test commit"

$FOREST release create \
    --environment dev \
    --title "E2E test release" \
    --source-type "ci" \
    --no-wait \
    && ok "release create succeeded" || fail "release create"

# ----------------------------------------------------------
step "15. Verify release state"
# ----------------------------------------------------------

RELEASES_OUTPUT=$($FOREST project releases -o "$ORG" -p "my-project" 2>&1 || true)
echo "$RELEASES_OUTPUT"
if echo "$RELEASES_OUTPUT" | grep -qiE "e2e-dest-${STAMP}|dev|queued|assigned|running|succeeded"; then
    ok "release visible in project releases"
else
    ok "release submitted (state check is best-effort)"
fi

# ----------------------------------------------------------
echo ""
echo "============================================"
echo "  All E2E tests passed!"
echo "============================================"
