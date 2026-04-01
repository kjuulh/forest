use crate::state::State;

/// Generate comprehensive documentation for the forest CLI.
///
/// Outputs a manpage-style reference covering all commands, the component
/// model, protocol, SDK, and deployment flow.
#[derive(clap::Parser)]
pub struct DocsCommand {
    /// Output format
    #[arg(long, default_value = "text")]
    format: String,

    /// Specific topic to document (omit for full reference)
    topic: Option<String>,
}

impl DocsCommand {
    pub async fn execute(&self, _state: &State) -> anyhow::Result<()> {
        match self.topic.as_deref() {
            Some("commands") => print_commands(),
            Some("architecture") => print_architecture(),
            Some("protocol") => print_protocol(),
            Some("sdk") => print_sdk(),
            Some("components") => print_components(),
            Some("templates") => print_templates(),
            Some("deployment") => print_deployment(),
            Some("config") => print_config(),
            Some("topics") => print_topics(),
            Some(topic) => {
                eprintln!("unknown topic: {topic}");
                eprintln!();
                print_topics();
            }
            None => print_full_reference(),
        }
        Ok(())
    }
}

fn print_topics() {
    println!("FOREST DOCUMENTATION TOPICS");
    println!();
    println!("  architecture   Component model, project structure, and design principles");
    println!("  commands       All CLI commands with usage and flags");
    println!("  protocol       Component protocol v2 (stdin/stdout JSON lines)");
    println!("  sdk            TypeScript and Rust SDK reference");
    println!("  components     Component types, lifecycle, and CUE spec format");
    println!("  templates      Template system (minijinja) with available globals and filters");
    println!("  deployment     Deployment flow: prepare, release, rollback");
    println!("  config         Configuration: CUE modules, registries, environment variables");
    println!();
    println!("Usage: forest docs <topic>");
    println!("       forest docs            (full reference)");
}

fn print_full_reference() {
    print_header();
    println!();
    print_architecture();
    println!();
    print_commands();
    println!();
    print_components();
    println!();
    print_protocol();
    println!();
    print_sdk();
    println!();
    print_templates();
    println!();
    print_deployment();
    println!();
    print_config();
}

fn print_header() {
    println!("FOREST(1)                        Forest Manual                        FOREST(1)");
    println!();
    println!("NAME");
    println!("    forest — development workflow orchestrator for component-based projects");
    println!();
    println!("SYNOPSIS");
    println!("    forest <command> [options]");
    println!("    forest run <command> [-- --key value ...]");
    println!("    forest release prepare");
    println!();
    println!("DESCRIPTION");
    println!("    Forest is a set of tools to help you design the development workflows you");
    println!("    need. It is specifically built to allow you to share workflows and");
    println!("    streamline boring tasks through reusable components.");
    println!();
    println!("    Components define specs, commands, and hooks using CUE. They are compiled");
    println!("    as Rust binaries or Deno/TypeScript scripts and communicate with the");
    println!("    forest runtime via a JSON line protocol over stdin/stdout.");
}

fn print_architecture() {
    println!("ARCHITECTURE");
    println!();
    println!("  Project Structure");
    println!("    A forest workspace consists of components and projects:");
    println!();
    println!("      components/           Reusable component definitions");
    println!("        org/name/");
    println!("          forest.cue            Component identity and metadata");
    println!("          forest.component.cue  Spec, commands, hooks definitions");
    println!("          cue.mod/module.cue    CUE module with SDK dependencies");
    println!("          deno.json             Import map (for TypeScript components)");
    println!("          src/");
    println!("            forestgen.ts         Generated types and router");
    println!("            main.ts              Component implementation");
    println!("            deps/                Generated dependency clients");
    println!("          templates/             Deployment templates");
    println!("          .forest/component/");
    println!("            meta.json            Built component descriptor");
    println!();
    println!("      projects/             Project configurations using components");
    println!("        my-project/");
    println!("          forest.cue            Project config, dependencies, env/destination mapping");
    println!("          cue.mod/module.cue    CUE module for the project");
    println!("          secrets/              Sealed secret files per environment");
    println!();
    println!("  Component Model");
    println!("    Components are isolated processes invoked by the forest runtime.");
    println!("    They cannot directly call each other — the runtime mediates all");
    println!("    inter-component communication.");
    println!();
    println!("    A component declares:");
    println!("      - #Spec: Input configuration schema");
    println!("      - #Commands: Named operations with typed input/output");
    println!("      - #Hooks: Contract implementations (e.g. forest/deployment)");
    println!();
    println!("  Dependency Graph");
    println!("    Components can depend on other components. Dependencies declared");
    println!("    in forest.cue are resolved at build time:");
    println!();
    println!("      dependencies: {{");
    println!("        \"org/component\": path: \"../path/to/component\"");
    println!("        \"org/component\": version: \"1.0.0\"");
    println!("      }}");
    println!();
    println!("    Components with a usage block in the project config expose their");
    println!("    commands via `forest run`. Peer dependencies (no usage block) are");
    println!("    only callable via inter-component calls through the runtime.");
}

fn print_commands() {
    println!("COMMANDS");
    println!();
    println!("  Project & Component Lifecycle");
    println!();
    println!("    forest init [name]");
    println!("        Scaffold a new project or component.");
    println!();
    println!("    forest add <component>");
    println!("        Add a component dependency to the project.");
    println!();
    println!("    forest build");
    println!("        Build the component for deployment. For Rust/Go/Docker components,");
    println!("        compiles binaries for configured platforms. For Deno/TypeScript");
    println!("        components, generates meta.json with the component descriptor.");
    println!("        Must be run from a component directory.");
    println!();
    println!("    forest generate --output <dir> [--language <lang>]");
    println!("        Generate typed SDK code from the CUE component spec.");
    println!("        Reads forest.component.cue, converts to OpenAPI via `cue def`,");
    println!("        then emits typed code.");
    println!();
    println!("        For TypeScript, generates:");
    println!("          <dir>/forestgen.ts       Own component types and router");
    println!("          <dir>/deps/<dep>.ts      Typed clients for each component dependency");
    println!();
    println!("        Languages: rust, typescript (aliases: deno, ts)");
    println!("        Auto-detected from forest.cue codegen.type if --language is omitted.");
    println!();
    println!("    forest publish");
    println!("        Publish the component to the forest registry. Uploads CUE specs,");
    println!("        binary artifacts, and the component manifest.");
    println!("        Requires FOREST_SERVER to be set.");
    println!();
    println!("    forest validate");
    println!("        Validate project config against component specs. Invokes each");
    println!("        component's validate command.");
    println!();
    println!("    forest update");
    println!("        Update dependencies to the latest versions matching the spec.");
    println!();
    println!("  Running Commands");
    println!();
    println!("    forest run <command> [-- --key value ...]");
    println!("        Run a project or component command. Commands are discovered from");
    println!("        all components with a usage block in the project config.");
    println!();
    println!("        Arguments after -- are parsed as --key value pairs and passed");
    println!("        as the command's input JSON.");
    println!();
    println!("        If command names collide across components, use qualified names:");
    println!("          forest run <component>:<command>");
    println!();
    println!("        Examples:");
    println!("          forest run seal -- --env dev --key MY_SECRET --value s3cr3t --cert path/to/tls.crt");
    println!("          forest run validate");
    println!("          forest run service:status");
    println!();
    println!("  Deployment");
    println!();
    println!("    forest release prepare");
    println!("        Generate deployment manifests for all environments and destinations.");
    println!("        Renders component templates and invokes deployment prepare hooks.");
    println!("        Output: .forest/deployment/<env>/<destination>/<type>/");
    println!();
    println!("    forest release");
    println!("        Execute the deployment (invokes release hooks server-side).");
    println!();
    println!("    forest release rollback");
    println!("        Roll back a deployment (invokes rollback hooks).");
    println!();
    println!("  Resource Management");
    println!();
    println!("    forest project [subcommand]      Manage projects");
    println!("    forest destination [subcommand]   Manage deployment destinations");
    println!("    forest environment [subcommand]   Manage environments");
    println!("    forest organisation [subcommand]  Manage organisations");
    println!("    forest components [subcommand]    Browse and manage components");
    println!("    forest notifications [subcommand] Manage notifications");
    println!();
    println!("  Authentication");
    println!();
    println!("    forest auth login                 Authenticate with the forest server");
    println!("    forest auth logout                Clear stored credentials");
}

fn print_components() {
    println!("COMPONENT DEFINITION");
    println!();
    println!("  CUE Files");
    println!();
    println!("    forest.cue — Component identity and build configuration:");
    println!();
    println!("      package mycomponent");
    println!();
    println!("      import \"forest.sh/forest/sdk@v0\"");
    println!();
    println!("      project: sdk.#ForestProject & {{");
    println!("        name:         \"mycomponent\"");
    println!("        organisation: \"myorg\"");
    println!("      }}");
    println!();
    println!("      dependencies: sdk.#ForestDependencies & {{");
    println!("        \"forest/deployment\": version: \"0.0.1\"");
    println!("        \"other/component\":   path:    \"../other\"");
    println!("      }}");
    println!();
    println!("      forest: component: sdk.#ForestComponent & {{");
    println!("        name:    project.name");
    println!("        version: \"0.1.0\"");
    println!("        codegen: {{ type: \"typescript\", output: \"./src/\" }}");
    println!("        upload:  {{ source: \"./src\", type: \"deno\" }}");
    println!("      }}");
    println!();
    println!("    forest.component.cue — Spec, commands, and hooks:");
    println!();
    println!("      import (");
    println!("        \"forest.sh/forest/sdk@v0\"");
    println!("        \"forest.sh/forest/deployment@v0\"");
    println!("      )");
    println!();
    println!("      #Spec: sdk.#ForestSpec & {{");
    println!("        name:  string");
    println!("        image: string");
    println!("        port:  int | *8080");
    println!("      }}");
    println!();
    println!("      #Commands: sdk.#ForestCommands & {{");
    println!("        validate: {{");
    println!("          description: \"Validate configuration\"");
    println!("          input: {{}}");
    println!("          output: {{ valid: bool, errors: [...string] }}");
    println!("        }}");
    println!("        seal: {{");
    println!("          description: \"Seal a secret\"");
    println!("          input: {{ env: string, key: string, value: string, cert: string }}");
    println!("          output: {{}}");
    println!("        }}");
    println!("      }}");
    println!();
    println!("      #Hooks: sdk.#ForestHooks & {{");
    println!("        \"forest/deployment\": deployment.#DeploymentHooks & {{");
    println!("          prepare: description: \"Generate manifests\"");
    println!("          release: description: \"Deploy\"");
    println!("          rollback: description: \"Rollback\"");
    println!("        }}");
    println!("      }}");
    println!();
    println!("  Deployment Contract (forest/deployment)");
    println!();
    println!("    The deployment contract defines three hooks:");
    println!();
    println!("      prepare   Returns manifests: [{{name: string, content: string}}]");
    println!("      release   Accepts release_id, performs deployment");
    println!("      rollback  Accepts release_id + target_revision, rolls back");
    println!();
    println!("    Manifests returned by prepare are written to the deployment output");
    println!("    directory with the specified filename.");
    println!();
    println!("  Component Types");
    println!();
    println!("    rust         Compiled binary, cross-platform via cargo");
    println!("    go           Compiled binary via go build");
    println!("    docker       Container image via docker buildx");
    println!("    deno         TypeScript script, no compilation needed");
    println!("    typescript   Alias for deno");
}

fn print_protocol() {
    println!("COMPONENT PROTOCOL v2");
    println!();
    println!("  Components communicate with the forest runtime via JSON lines on");
    println!("  stdin/stdout. The method name is passed as a CLI argument.");
    println!();
    println!("  Message Types");
    println!();
    println!("    Runtime → Component (invocation):");
    println!("      {{\"type\":\"invoke\",\"method\":\"commands/seal\",\"spec\":{{...}},\"input\":{{...}},\"context\":{{...}}}}");
    println!();
    println!("    Component → Runtime (call another component):");
    println!("      {{\"type\":\"call\",\"id\":\"1\",\"component\":\"org/name\",\"method\":\"commands/seal\",\"spec\":{{...}},\"input\":{{...}},\"context\":{{...}}}}");
    println!();
    println!("    Runtime → Component (call result):");
    println!("      {{\"type\":\"call_result\",\"id\":\"1\",\"result\":{{...}}}}");
    println!();
    println!("    Component → Runtime (final return):");
    println!("      {{\"type\":\"return\",\"result\":{{...}}}}");
    println!();
    println!("  The id field correlates call/call_result pairs.");
    println!("  Context is forwarded from the caller to the callee, preserving");
    println!("  environment, work_dir, and other runtime metadata.");
    println!();
    println!("  Meta-methods (legacy single-shot, no envelope):");
    println!("    _meta/describe          Returns ComponentDescriptor (protocol_version, methods)");
    println!("    _meta/template_config   Returns skip/rename/vars for template rendering");
    println!();
    println!("  CallContext Fields");
    println!();
    println!("    project       Project name");
    println!("    organisation  Organisation name");
    println!("    environment   Deployment environment (dev, staging, prod)");
    println!("    work_dir      Project directory path");
    println!("    release_id    Release identifier (during release/rollback)");
    println!("    dry_run       Boolean flag for dry runs");
}

fn print_sdk() {
    println!("SDK REFERENCE");
    println!();
    println!("  TypeScript SDK (forest-sdk.ts)");
    println!();
    println!("    Exports:");
    println!("      runOnce(service)        Run the component, handling one invocation");
    println!("      callComponent(          Call another component via the runtime");
    println!("        component, method,");
    println!("        spec, input)");
    println!();
    println!("    Types:");
    println!("      ComponentService<S>     Interface: call(), methods(), templateConfig?()");
    println!("      CallContext             Runtime context (project, env, work_dir, ...)");
    println!("      MethodDescriptor        Method metadata (name, kind, topic, description)");
    println!("      ForestError             Base error class");
    println!("      MethodNotFoundError     Unknown method");
    println!();
    println!("  Generated Code (forestgen.ts)");
    println!();
    println!("    Generated by `forest generate`, contains:");
    println!("      - Spec interface matching the CUE #Spec");
    println!("      - Input/Output interfaces for each command and hook");
    println!("      - CommandHandler interface with typed methods");
    println!("      - HookHandler interfaces with typed methods (receive CallContext)");
    println!("      - createRouter() function that wires handlers into a ComponentService");
    println!();
    println!("  Generated Dependency Clients (src/deps/<org>_<name>.ts)");
    println!();
    println!("    For each component dependency with forest.component.cue, generates");
    println!("    typed wrapper functions:");
    println!();
    println!("      import * as sealedSecrets from \"./deps/org_sealed-secrets.ts\";");
    println!();
    println!("      await sealedSecrets.commandsSeal(spec, input);");
    println!("      const result = await sealedSecrets.hooksForestDeploymentPrepare(spec, {{}});");
    println!();
    println!("    Functions call callComponent() internally — the runtime resolves and");
    println!("    invokes the target component.");
    println!();
    println!("  Rust SDK (forest-sdk crate)");
    println!();
    println!("    Provides the same ComponentService trait and protocol handling");
    println!("    for Rust binary components.");
}

fn print_templates() {
    println!("TEMPLATE SYSTEM");
    println!();
    println!("  Templates use minijinja (Jinja2-compatible) syntax.");
    println!("  Files with .jinja2 extension are rendered; others are copied as-is.");
    println!();
    println!("  Available Globals");
    println!();
    println!("    config   The merged component configuration (base + env override)");
    println!("    env      The deployment environment name (dev, staging, prod)");
    println!();
    println!("  Available Filters");
    println!();
    println!("    to_lower             Lowercase string");
    println!("    to_upper             Uppercase string");
    println!("    to_snake             snake_case");
    println!("    to_camel             camelCase");
    println!("    to_pascal            PascalCase");
    println!("    to_screaming_snake   SCREAMING_SNAKE_CASE");
    println!("    to_kebab             kebab-case");
    println!("    as_bool              Parse string as boolean");
    println!("    dictsort             Sort map for iteration");
    println!();
    println!("  Map Iteration");
    println!();
    println!("    Use dictsort to iterate maps:");
    println!("      {{% for key, value in config.env_vars | dictsort %}}");
    println!("        - name: \"{{{{ key }}}}\"");
    println!("          value: \"{{{{ value }}}}\"");
    println!("      {{% endfor %}}");
    println!();
    println!("  Template Directory");
    println!();
    println!("    templates/deployment/<destination_type>/");
    println!("      10-namespace.yaml.jinja2");
    println!("      30-deployment.yaml.jinja2");
    println!("      40-ingress.yaml.jinja2");
    println!();
    println!("    Files are numbered to control ordering in the output.");
}

fn print_deployment() {
    println!("DEPLOYMENT FLOW");
    println!();
    println!("  `forest release prepare` processes each deployment item:");
    println!();
    println!("    1. Parse project config (forest.cue)");
    println!("    2. For each component × environment × destination:");
    println!("       a. Merge base config with env-specific config overrides");
    println!("       b. Render templates from templates/deployment/<type>/");
    println!("          with config and env globals");
    println!("       c. Invoke the component's deployment prepare hook");
    println!("       d. Write returned manifests ({{name, content}}) to output");
    println!("       e. Write forest/config.json with deployment metadata");
    println!();
    println!("  Output Structure");
    println!();
    println!("    .forest/deployment/");
    println!("      <env>/");
    println!("        <destination>/");
    println!("          <destination_type>/");
    println!("            10-namespace.yaml");
    println!("            20-sealed-secrets.yaml    (from hook)");
    println!("            30-deployment.yaml");
    println!("            40-ingress.yaml");
    println!("            forest/config.json");
    println!();
    println!("  Inter-Component Calls During Prepare");
    println!();
    println!("    A component's prepare hook can call other components. The runtime");
    println!("    resolves the target from the project's dependencies and forwards");
    println!("    the context (including environment) to the callee.");
    println!();
    println!("  Project Config (forest.cue)");
    println!();
    println!("    org: component: sdk.#ForestComponentUsage & {{");
    println!("      env: {{");
    println!("        dev: {{");
    println!("          destinations: [{{destination: \"flux-dev.*\", type: \"forest/flux@1\"}}]");
    println!("          config: {{}}   // env-specific overrides");
    println!("        }}");
    println!("      }}");
    println!("      config: {{          // base config (merged with env overrides)");
    println!("        name: \"myapp\"");
    println!("        image: \"registry/myapp\"");
    println!("      }}");
    println!("    }}");
}

fn print_config() {
    println!("CONFIGURATION");
    println!();
    println!("  Environment Variables");
    println!();
    println!("    FOREST_SERVER    Forest server URL (required for registry operations)");
    println!("    CUE_REGISTRY     CUE module registry (OCI). Passed to `cue` commands.");
    println!("                     Example: localhost:4042+insecure");
    println!();
    println!("  CUE Module (cue.mod/module.cue)");
    println!();
    println!("    module: \"forest.sh/org/name@v0\"");
    println!("    language: version: \"v0.15.4\"");
    println!("    deps: {{");
    println!("      \"forest.sh/forest/sdk@v0\": v: \"v0.3.0\"");
    println!("      \"forest.sh/forest/deployment@v0\": v: \"v0.3.0\"");
    println!("    }}");
    println!();
    println!("  Component Metadata (.forest/component/meta.json)");
    println!();
    println!("    Generated by `forest build`. Contains:");
    println!("      - organisation, name, version");
    println!("      - kind: \"deno\" | binary platform info");
    println!("      - entrypoint (for deno components)");
    println!("      - descriptor: protocol_version and method list");
}
