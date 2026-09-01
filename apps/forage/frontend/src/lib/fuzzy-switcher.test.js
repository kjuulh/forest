import { describe, it, expect } from "vitest";
// The switcher is a plain module served straight out of static/ — no build
// step, which is the point of it. The matcher is still the piece worth
// testing: the dropdown renders either way, it just puts the wrong project
// under the cursor when the ranking is wrong, and Enter takes you there.
import { fuzzyMatch, rankMatches } from "../../../static/js/fuzzy-switcher.js";
import parity from "./fzf-parity.json";

// A representative slice of the understory org — the list this exists for.
const PROJECTS = [
  "awslogin",
  "bark",
  "cache-reaper",
  "canopy-data-gateway",
  "canopy-transformations",
  "commission-accounting",
  "commission-dashboard",
  "dynamo-ingest",
  "forage",
  "forest",
  "gitnow",
  "snag",
];

const rank = (query, items = PROJECTS) => rankMatches(query, items).map((m) => m.label);
const top = (query, items) => rank(query, items)[0];

describe("the queries people actually type", () => {
  it("`cdg` finds canopy-data-gateway by its initials", () => {
    expect(top("cdg")).toBe("canopy-data-gateway");
  });

  it("`comm` finds commission-dashboard", () => {
    expect(top("comm")).toBe("commission-dashboard");
  });

  it("`transf` finds canopy-transformations mid-name", () => {
    expect(top("transf")).toBe("canopy-transformations");
  });

  it("`cache` finds cache-reaper", () => {
    expect(top("cache")).toBe("cache-reaper");
  });

  it("scattered characters still find their project", () => {
    expect(top("dyng")).toBe("dynamo-ingest");
  });
});

describe("the ranking this exists to get right", () => {
  it("a word boundary beats the middle of a word", () => {
    expect(top("gate", ["aggregate-logs", "canopy-data-gateway"])).toBe(
      "canopy-data-gateway",
    );
  });

  it("an unbroken run beats the same characters scattered", () => {
    expect(top("data", ["canopy-data-mcp", "d-a-t-a-x"])).toBe("canopy-data-mcp");
  });

  it("an initialism beats an accident of spelling", () => {
    // `cardigan` holds c-d-g more tightly than canopy-data-gateway does, but
    // the initials are what was meant, and the boundary bonuses say so.
    expect(top("cdg", ["cardigan", "canopy-data-gateway"])).toBe("canopy-data-gateway");
  });

  it("a repeated leading character costs you", () => {
    // fzf's first row restarts the alignment on every occurrence of the first
    // pattern character rather than carrying the best score forward, so the
    // second `c` in `cxxxxcxxp` drags it below the name without one. This is
    // the asymmetry the port got wrong first time round.
    expect(rank("cp", ["cxxxxcxxp", "cxxxxxxxp"])).toEqual([
      "cxxxxxxxp",
      "cxxxxcxxp",
    ]);
  });

  it("the shorter name wins a tie", () => {
    // Both are prefix matches; commission-dashboard is one character shorter.
    expect(rank("commission")).toEqual([
      "commission-dashboard",
      "commission-accounting",
    ]);
  });

  it("equal scores keep the order the server sent", () => {
    const same = ["one-thing", "two-things"];
    expect(rank("", same)).toEqual(same);
    expect(rank("", [...same].reverse())).toEqual([...same].reverse());
  });
});

describe("case", () => {
  it("matching ignores it, in both directions", () => {
    expect(top("CDG")).toBe("canopy-data-gateway");
    expect(top("cdg", ["Canopy-Data-Gateway"])).toBe("Canopy-Data-Gateway");
  });

  it("a query's case never changes its score", () => {
    // A deliberate departure from fzf, which is smart-case: there, `CDG`
    // matches nothing in an all-lowercase list. In a dropdown of a dozen rows
    // that reads as broken rather than as a feature.
    expect(fuzzyMatch("CDG", "canopy-data-gateway")).toEqual(
      fuzzyMatch("cdg", "canopy-data-gateway"),
    );
  });

  it("surrounding whitespace is not part of the query", () => {
    expect(fuzzyMatch("  cdg  ", "canopy-data-gateway")).toEqual(
      fuzzyMatch("cdg", "canopy-data-gateway"),
    );
  });
});

describe("what does not match", () => {
  it("a name missing one of the characters is not a match", () => {
    expect(fuzzyMatch("zzz", "canopy-data-gateway")).toBeNull();
    expect(fuzzyMatch("cdgx", "canopy-data-gateway")).toBeNull();
  });

  it("out-of-order characters are not a subsequence", () => {
    expect(fuzzyMatch("gdc", "canopy-data-gateway")).toBeNull();
  });

  it("a query longer than the name cannot match", () => {
    expect(fuzzyMatch("forestry", "forest")).toBeNull();
  });

  it("non-matches are dropped, so the caller can safely take the top N", () => {
    expect(rank("cdg")).toEqual(["canopy-data-gateway"]);
  });

  it("an empty query matches everything", () => {
    expect(rank("")).toEqual(PROJECTS);
    expect(rank("   ")).toEqual(PROJECTS);
  });
});

describe("the positions the dropdown underlines", () => {
  it("are the characters the winning alignment used", () => {
    // c·d·g on the three word boundaries of canopy-data-gateway.
    expect(fuzzyMatch("cdg", "canopy-data-gateway").positions).toEqual([0, 7, 12]);
  });

  it("stay contiguous for a plain substring", () => {
    expect(fuzzyMatch("data", "canopy-data-mcp").positions).toEqual([7, 8, 9, 10]);
  });

  it("are empty for an empty query, so nothing is marked", () => {
    expect(fuzzyMatch("", "canopy-data-gateway")).toEqual({ score: 0, positions: [] });
  });

  it("always name exactly as many characters as were typed", () => {
    for (const label of PROJECTS) {
      for (const query of ["c", "an", "sn", "ore", "gi", "amo"]) {
        const match = fuzzyMatch(query, label);
        if (!match) continue;
        expect(match.positions).toHaveLength(query.length);
        // In order, in range, and actually the characters typed.
        const chars = match.positions.map((p) => label[p].toLowerCase()).join("");
        expect(chars).toBe(query);
        for (let k = 1; k < match.positions.length; k++) {
          expect(match.positions[k]).toBeGreaterThan(match.positions[k - 1]);
        }
      }
    }
  });
});

describe("the shape rankMatches returns", () => {
  it("carries the index back, so a caller can find its own DOM node", () => {
    const [best] = rankMatches("cdg", PROJECTS);
    expect(best.index).toBe(PROJECTS.indexOf("canopy-data-gateway"));
    expect(best.label).toBe("canopy-data-gateway");
    expect(best.score).toBeGreaterThan(0);
  });
});

// The port is only worth having if it ranks the way fzf ranks. These are real
// fzf answers, recorded by fzf-parity.gen.mjs so the suite doesn't need fzf
// installed. Regenerate that file when tracking a new fzf release and read the
// diff — a change here is a change in what the switcher puts under the cursor.
describe(`parity with fzf ${parity.fzf}`, () => {
  for (const [query, expected] of Object.entries(parity.expected)) {
    it(`${JSON.stringify(query)} ranks the way fzf ranks it`, () => {
      expect(rank(query, parity.corpus)).toEqual(expected);
    });
  }
});
