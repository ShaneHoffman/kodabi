import { describe, expect, it } from "vitest";
import { viewKey, type View } from "./useNavigation";

/**
 * `viewKey` exists for one job: keying the error boundary around the routed
 * view. Every case below is a pair the boundary has to tell apart, because
 * `view.kind` alone could not — and a fallback that will not clear strands the
 * user on a crash screen that tells them to navigate away from it.
 */
describe("viewKey", () => {
  it("separates two projects", () => {
    const a: View = { kind: "project", slug: "paradise-golf" };
    const b: View = { kind: "project", slug: "growth" };

    expect(viewKey(a)).not.toBe(viewKey(b));
  });

  it("separates two notes, and a note from the compose form beside it", () => {
    const open: View = { kind: "noteEditor", noteId: "n_a1b2c3", project: "growth" };
    const other: View = { kind: "noteEditor", noteId: "n_d4e5f6", project: "growth" };
    const compose: View = { kind: "noteEditor", noteId: null, project: "growth" };

    expect(viewKey(open)).not.toBe(viewKey(other));
    expect(viewKey(open)).not.toBe(viewKey(compose));
  });

  it("separates the same note reached through different projects", () => {
    const filed: View = { kind: "noteEditor", noteId: "n_a1b2c3", project: "growth" };
    const unfiled: View = { kind: "noteEditor", noteId: "n_a1b2c3", project: null };

    expect(viewKey(filed)).not.toBe(viewKey(unfiled));
  });

  it("separates two searches", () => {
    const first: View = { kind: "search", query: "golf" };
    const second: View = { kind: "search", query: "invoice" };

    expect(viewKey(first)).not.toBe(viewKey(second));
  });

  it("is stable for the same destination", () => {
    expect(viewKey({ kind: "project", slug: "growth" })).toBe(
      viewKey({ kind: "project", slug: "growth" }),
    );
    expect(viewKey({ kind: "inbox" })).toBe(viewKey({ kind: "inbox" }));
  });

  it("gives the payload-free destinations their own kind", () => {
    expect(viewKey({ kind: "inbox" })).toBe("inbox");
    expect(viewKey({ kind: "needsAttention" })).toBe("needsAttention");
    expect(viewKey({ kind: "settings" })).toBe("settings");
  });
});
