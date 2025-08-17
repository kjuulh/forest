# Non

Non is a set of tools to help you design the development workflows you need. It is specifically built to allow you to share workflows and streamline boring tasks.

## Example

With `non` you can quickly compose a shareable workflow to initiate a starter kit for a service, builds to produce production ready artifacts, spin up a development environment, release production services, and more.

**Install `non`**

*bargo*

```bash
cargo (b)install non
```

*brew*

```bash
brew install nonothing/tap/non  
```

**Create service**

```bash
non init
> starter: github.com/nonothing/rust-service-starter
> name: my-starter-service
> http: y
cd my-starter-service
```

**Build production grade artifact**

```bash
non run docker:build
> tag: my-service-starter:local
```

**Development environment**

```bash
non run dev:up
> spins up: postgresql at tcp/5432
```

**Deploy artifact**

```bash
non service release
> branch: main
> artifact: abc123 - $(date)
> release: y
```

**Rollback service**

```bash
non service rollback
> artifact: abc123 - $(date)
> release: y
```

## Architecture

Non allows you to use standard components, either upstream from non, or build your own. Often we see that you use wrap standard non components in your own ways to make `non` truly yours.

### Project

Project is the place where actual work happens, it is in this context that `non` components are executed.

A `non.toml` is the artifact that describes your dependencies, or any local workflows you may've got

### Component

Component can be thought of as a library or repository of logic, files or media. Components are what makes it possible to do `docker:build` as `no-rust-docker` is a component, that houses the build command, allowing the user to build the service in question into a production ready artifact

Components can be added to a project with the following:

```bash
non add
> nonothing: rust_service:docker
# Rust is the language in question, service is the type of project we care about
# Docker is the component under this namespace
```

Components are generally language, and/or organisation specific, and can pull certain interfaces from upstream to get access to top level commands. Such as docker:build, which is a reserved key project, all others has to go through a namespace.

- myorg/rust_service:docker
- rust_service:docker # blessed upstream implementation of the docker interface for a rust service implementation
- go_service:docker # blessed upstream implementation of the docker interface for a golang service implementation
- docker # blessed upstream interface cannot be used on its own


## Roadmap

- [ ] Init system
  - [ ] Templating
- [ ] Components
  - [ ] Local
  - [ ] Remote


## Domain model

- Projects: scoped by org and name, contains everything related to a single project. Such as dependencies, code, links and more, documentation.
- Components: a non runnable project. It can contain requirements, other dependencies. It is basically either a tool, or a set of requirements and features that make upstream development easier.
- Dependencies: A project or component can have a set of dependencies. These are components to include. Each component can require certain things their upstream parts, and add certain functionality.
- Requirements: A component can require information from upstream services. These requirements are extra bits of information that can either be provided at runtime via. args, or via. the project variables. A component has to implement the requirements and re-require them from their upstream.
- Artifact: A project can be published via. artifacts. An artifact can be used by interested parties and can optionally be annotated. These can either be static, or dynamic. If static it acts as releases, if dynamic these can be released, rolled back, re-released. A static artifact can be upgraded to a dynamic artifact, but the other way around isn't possible
