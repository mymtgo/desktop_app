import { beforeEach, describe, expect, it, vi } from "vitest";
import { findAll, mountFake } from "@/replay/replayTestHarness";
import type { MatchListState } from "@/lib/matchList";

const state = vi.hoisted(() => ({ platform: "mtgo" as "mtgo" | "arena" }));
const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import("vue");
  return {
    default: defineComponent({
      inheritAttrs: false,
      setup:
        (_, { attrs, slots }) =>
        () =>
          h(tag, attrs, slots.default?.()),
    }),
  };
});

vi.mock("@/composables/usePlatformContext", async () => {
  const { ref } = await import("vue");
  return { usePlatformContext: () => ({ platform: ref(state.platform) }) };
});
vi.mock("@/components/ui/select", async () => ({
  Select: (await stub("select")).default,
  SelectContent: (await stub("select-content")).default,
  SelectItem: (await stub("select-item")).default,
  SelectTrigger: (await stub("select-trigger")).default,
  SelectValue: (await stub("select-value")).default,
}));
vi.mock("@/components/ui/button", async () => ({
  Button: (await stub("button")).default,
}));
vi.mock("@/components/ui/input", async () => ({
  Input: (await stub("input")).default,
}));
vi.mock("lucide-vue-next", async () => ({ X: (await stub("x")).default }));

const { default: MatchListFilters } = await import("./MatchListFilters.vue");

const list: MatchListState = {
  from: null,
  to: null,
  result: null,
  kind: null,
  bestOf: null,
  archetype: null,
  sortBy: null,
  sortDir: "desc",
  page: 1,
};

function bestOfSelects(props: Record<string, unknown> = {}) {
  const mounted = mountFake(MatchListFilters, {
    state: list,
    summary: "",
    ...props,
  });
  return findAll(
    mounted.root,
    (node) => node.props["aria-label"] === "Best of",
  );
}

beforeEach(() => {
  state.platform = "mtgo";
});

describe("the Bo1 and Bo3 select", () => {
  it("shows on Arena", () => {
    state.platform = "arena";
    expect(bestOfSelects()).toHaveLength(1);
  });

  it("is absent on MTGO", () => {
    expect(bestOfSelects()).toHaveLength(0);
  });

  it("is absent when the page hides it (a deck has its own switch)", () => {
    state.platform = "arena";
    expect(bestOfSelects({ hideBestOf: true })).toHaveLength(0);
  });
});
