/**
 * The environment palette.
 *
 * Colour here is information, not decoration — cool means the pipeline is
 * working, warm means a person is needed — so getting an environment into the
 * wrong bucket is a correctness bug, not a taste one. See
 * design/RELEASE-SWIMLANE.md.
 */
import { describe, it, expect } from "vitest";
import { envStage, envRank, envColorPair, orderLanes, SIGNAL } from "./colors.js";

describe("classifying an environment by its stage", () => {
  it("recognises the plain names", () => {
    expect(envStage("prod")).toBe("prod");
    expect(envStage("production")).toBe("prod");
    expect(envStage("preprod")).toBe("preprod");
    expect(envStage("staging")).toBe("staging");
    expect(envStage("dev")).toBe("dev");
    expect(envStage("test")).toBe("test");
  });

  // forest names environments `<domain>-<stage>`, so the tail is the part that
  // says what the environment *is*. The old substring scan got these two right
  // by luck, and would have called `prod-metrics` production.
  it("reads the stage off the last segment", () => {
    expect(envStage("data-prod")).toBe("prod");
    expect(envStage("platform-dev")).toBe("dev");
    expect(envStage("platform_dev")).toBe("dev");
    expect(envStage("data prod")).toBe("prod");
  });

  it("falls back to the first segment when the last names nothing", () => {
    expect(envStage("prod-eu-north-1")).toBe("prod");
    expect(envStage("staging-2")).toBe("staging");
  });

  it("leaves an environment that names no stage unclassified", () => {
    expect(envStage("infrastructure-hetzner")).toBeNull();
    expect(envStage("finance")).toBeNull();
    expect(envStage("")).toBeNull();
    expect(envStage(undefined)).toBeNull();
  });
});

describe("lane order", () => {
  // Production first, closest to the page edge; the card column is on the
  // right, so code flows right to left out to production.
  it("puts production first and works back down the pipeline", () => {
    const names = orderLanes([
      { name: "dev" }, { name: "prod" }, { name: "staging" }, { name: "preprod" },
    ]).map((l) => l.name);
    expect(names).toEqual(["prod", "preprod", "staging", "dev"]);
  });

  it("puts unclassified environments after the ones it understands", () => {
    const names = orderLanes([{ name: "finance" }, { name: "prod" }]).map((l) => l.name);
    expect(names).toEqual(["prod", "finance"]);
  });

  // The server sends environments in its own order; ties must not scramble it.
  it("is stable within a rank", () => {
    const names = orderLanes([
      { name: "finance" }, { name: "billing" }, { name: "hetzner" },
    ]).map((l) => l.name);
    expect(names).toEqual(["finance", "billing", "hetzner"]);
  });

  it("survives missing input", () => {
    expect(orderLanes(undefined)).toEqual([]);
    expect(orderLanes([])).toEqual([]);
  });

  it("ranks by stage, not by name", () => {
    expect(envRank("data-prod")).toBeLessThan(envRank("data-dev"));
    expect(envRank("anything-unknown")).toBeGreaterThan(envRank("dev"));
  });
});

describe("the palette itself", () => {
  it("gives every environment a light and a dark colour", () => {
    for (const name of ["prod", "data-prod", "dev", "finance", "hetzner", ""]) {
      const pair = envColorPair(name);
      expect(pair, name).toHaveLength(2);
      for (const c of pair) expect(c, name).toMatch(/^#[0-9a-f]{6}$/);
    }
  });

  it("gives an unclassified environment the same colour every time", () => {
    expect(envColorPair("infrastructure-hetzner")).toEqual(envColorPair("infrastructure-hetzner"));
  });

  // The rule the whole system rests on: warm belongs to "a person is needed"
  // and "it broke". An environment that borrowed amber would make the gutter
  // unreadable at exactly the moment it matters.
  it("never hands an environment a warm colour", () => {
    const warm = new Set(
      [SIGNAL.attention.light, SIGNAL.attention.dark, SIGNAL.failure.light, SIGNAL.failure.dark]
        .map((c) => c.toLowerCase()),
    );
    const names = [
      "prod", "production", "preprod", "staging", "test", "dev",
      "data-prod", "platform-dev", "finance", "hetzner", "billing", "canary",
      "infrastructure-hetzner", "sandbox", "qa", "local",
    ];
    for (const name of names) {
      for (const c of envColorPair(name)) {
        expect(warm.has(c.toLowerCase()), `${name} → ${c}`).toBe(false);
      }
    }
  });

  it("gives the stages distinct colours from each other", () => {
    const stages = ["prod", "preprod", "staging", "test", "dev"];
    const lights = stages.map((s) => envColorPair(s)[0]);
    expect(new Set(lights).size).toBe(stages.length);
  });
});
