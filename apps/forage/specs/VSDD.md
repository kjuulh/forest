# Verified Spec-Driven Development (VSDD)

## The Fusion: VDD x TDD x SDD for AI-Native Engineering

### Overview

VSDD is the unified software engineering methodology used for all forage development. It fuses three paradigms into a single AI-orchestrated pipeline:

- **Spec-Driven Development (SDD):** Define the contract before writing a single line of implementation. Specs are the source of truth.
- **Test-Driven Development (TDD):** Tests are written before code. Red -> Green -> Refactor. No code exists without a failing test that demanded it.
- **Verification-Driven Development (VDD):** Subject all surviving code to adversarial refinement until a hyper-critical reviewer is forced to hallucinate flaws.

### The Toolchain

| Role | Entity | Function |
|------|--------|----------|
| The Architect | Human Developer | Strategic vision, domain expertise, acceptance authority |
| The Builder | Claude | Spec authorship, test generation, code implementation, refactoring |
| The Adversary | External reviewer | Hyper-critical reviewer with zero patience |

### The Pipeline

#### Phase 1 - Spec Crystallization

Nothing gets built until the contract is airtight.

**Step 1a: Behavioral Specification**
- Behavioral Contract: preconditions, postconditions, invariants
- Interface Definition: input types, output types, error types
- Edge Case Catalog: exhaustive boundary conditions and failure modes
- Non-Functional Requirements: performance, memory, security

**Step 1b: Verification Architecture**
- Provable Properties Catalog: which invariants must be formally verified
- Purity Boundary Map: deterministic pure core vs effectful shell
- Property Specifications: formal property definitions where applicable

**Step 1c: Spec Review Gate**
- Reviewed by both human and adversary before any tests

#### Phase 2 - Test-First Implementation (The TDD Core)

Red -> Green -> Refactor, enforced by AI.

**Step 2a: Test Suite Generation**
- Unit tests per behavioral contract item
- Edge case tests from the catalog
- Integration tests for system context
- Property-based tests for invariants

**The Red Gate:** All tests must fail before implementation begins.
> **Enforcement note (from Review 002):** When writing tests alongside templates and routes,
> use stub handlers returning 501 to verify tests fail before implementing the real logic.
> This prevents false confidence from tests that were never red.

**Step 2b: Minimal Implementation**
1. Pick the next failing test
2. Write the smallest implementation that makes it pass
3. Run the full suite - nothing else should break
4. Repeat

**Step 2c: Refactor**
After all tests green, refactor for clarity and performance.

#### Phase 3 - Adversarial Refinement

The code survived testing. Now it faces the gauntlet.

Reviews: spec fidelity, test quality, code quality, security surface, spec gaps.

#### Phase 4 - Feedback Integration Loop

Critique feeds back through the pipeline:
- Spec-level flaws -> Phase 1
- Test-level flaws -> Phase 2a
- Implementation flaws -> Phase 2c
- New edge cases -> Spec update -> new tests -> fix

#### Phase 5 - Formal Hardening

- Fuzz testing on the pure core
- Security static analysis (cargo-audit, clippy)
- Mutation testing where applicable

#### Phase 6 - Convergence

Done when:
- Adversary critiques are nitpicks, not real issues
- No meaningful untested scenarios remain
- Implementation matches spec completely
- Security analysis is clean

### Core Principles

1. **Spec Supremacy**: The spec is the highest authority below the human developer
2. **Verification-First Architecture**: Pure core, effectful shell - designed from Phase 1
3. **Red Before Green**: No implementation without a failing test
4. **Anti-Slop Bias**: First "correct" version assumed to contain hidden debt
5. **Minimal Implementation**: Three similar lines > premature abstraction

### Applying VSDD in This Project

Each feature follows this flow:

1. Create spec in `specs/features/<feature-name>.md`
2. Spec review with human
3. Write failing tests in appropriate crate
4. Implement minimally in pure core (`forage-core`)
5. Wire up in effectful shell (`forage-server`, `forage-db`)
6. Adversarial review
7. Iterate until convergence
