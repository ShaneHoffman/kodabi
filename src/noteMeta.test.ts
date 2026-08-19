import { describe, expect, it } from "vitest";
import { matchScore, noteKind, noteMeta, projectRowMeta } from "./noteMeta";

describe("noteMeta", () => {
  const note = { date: "2026-07-10T09:15:00-07:00", tags: ["research", "phase-3"] };

  it("leads with the calendar day off either date form", () => {
    expect(noteMeta(note)).toBe("2026-07-10 · research · phase-3");
    expect(noteMeta({ date: "2026-07-10", tags: [] })).toBe("2026-07-10");
  });

  it("drops falsy middles so a caller need not branch", () => {
    expect(noteMeta(note, null, matchScore(0.62))).toBe(
      "2026-07-10 · 62% match · research · phase-3",
    );
  });
});

describe("noteKind", () => {
  it("names a kind worth naming", () => {
    expect(noteKind("chat")).toBe("chat");
    expect(noteKind("meeting")).toBe("meeting");
  });

  it("stays silent for a plain note", () => {
    // Every note is a `note` until proven otherwise, so the value is noise in
    // a list. `noteMeta` drops the null for free.
    expect(noteKind("note")).toBeNull();
  });
});

describe("projectRowMeta", () => {
  it("leads with the kind, then the genre, then the day", () => {
    expect(
      projectRowMeta({ date: "2026-07-10T09:15:00-07:00", type: "meeting", category: "standup" }),
    ).toBe("meeting · Stand-up · 2026-07-10");
  });

  it("says nothing extra for a meeting nothing has classified", () => {
    expect(projectRowMeta({ date: "2026-07-10", type: "meeting", category: null })).toBe(
      "meeting · 2026-07-10",
    );
  });

  it("still drops the kind on a plain note", () => {
    expect(projectRowMeta({ date: "2026-07-10", type: "note", category: null })).toBe("2026-07-10");
  });
});
