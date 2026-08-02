import { describe, expect, it } from "vitest";
import { folderHue } from "./useProjects";

describe("folderHue", () => {
  it("gives the same slug the same hue every time", () => {
    // A hue is identity: it has to survive a reload, a rename of the project
    // beside it, and a vault that grows around it.
    expect(folderHue("briarwood-golf")).toBe(folderHue("briarwood-golf"));
  });

  it("does not derive the hue from the slug's neighbours", () => {
    // The bug this rules out is cycling by list position, where creating one
    // project repaints every folder below it.
    const before = ["briarwood-golf", "household"].map(folderHue);
    const after = ["briarwood-golf", "clients/acme", "household"].map(folderHue);

    expect(after[0]).toBe(before[0]);
    expect(after[2]).toBe(before[1]);
  });

  it("spreads across the palette rather than favouring one hue", () => {
    const hues = new Set(
      ["briarwood-golf", "riverbend-deck", "little-league", "a"].map(folderHue),
    );

    expect(hues.size).toBe(4);
  });

  it("pins its output, because changing it recolours every vault on disk", () => {
    // Nothing stores a hue — it is derived on every render — so a "harmless"
    // tweak to the hash is a silent, global, unannounced restyle. If this fails
    // and the new values are genuinely wanted, that is a deliberate decision to
    // write down, not a snapshot to bless.
    expect(folderHue("briarwood-golf")).toBe("coral");
    expect(folderHue("riverbend-deck")).toBe("plum");
    expect(folderHue("little-league")).toBe("teal");
    expect(folderHue("a")).toBe("cobalt");
  });

  it("handles the edges a slug can actually reach", () => {
    // Nested slugs are "/"-joined, and an empty string is what a bad listing
    // would hand it — neither may throw or land outside the four hues.
    expect(folderHue("clients/acme/2026")).toBe("coral");
    expect(["coral", "cobalt", "teal", "plum"]).toContain(folderHue(""));
  });
});
