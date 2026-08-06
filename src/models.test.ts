import { describe, expect, it } from "vitest";
import {
  aggregateModelStatus,
  formatMegabytes,
  progressFromEvent,
  type ModelSetStatus,
  type ModelStatusDto,
} from "./models";

function set(overrides: Partial<ModelSetStatus> = {}): ModelSetStatus {
  return {
    id: "parakeet-tdt-0.6b-v2-int8",
    state: "missing",
    bytesTotal: 100,
    bytesPresent: 0,
    license: "CC-BY-4.0",
    ...overrides,
  };
}

function status(overrides: Partial<ModelStatusDto> = {}): ModelStatusDto {
  return {
    ready: false,
    bytesRequired: 100,
    bytesPresent: 0,
    sets: [set()],
    downloading: false,
    modelsDir: "C:\\app\\.models",
    ...overrides,
  };
}

describe("aggregateModelStatus", () => {
  it("reports ready when every set this build needs is installed", () => {
    const state = aggregateModelStatus(
      status({
        ready: true,
        bytesRequired: 0,
        sets: [set({ state: "ready", bytesPresent: 100 })],
      }),
    );
    expect(state).toEqual({ status: "ready", envOverridden: false });
  });

  it("flags a developer override so Settings can say so instead of offering a download", () => {
    const state = aggregateModelStatus(
      status({ ready: true, bytesRequired: 0, sets: [set({ state: "env_overridden" })] }),
    );
    expect(state).toEqual({ status: "ready", envOverridden: true });
  });

  it("quotes what is left to fetch, not the full size, so a resumed download is honest", () => {
    const state = aggregateModelStatus(
      status({ bytesRequired: 762_000_000, bytesPresent: 131_000_000 }),
    );
    expect(state).toEqual({ status: "missing", bytesRequired: 631_000_000 });
  });

  it("never quotes a negative figure if the backend's counts cross", () => {
    const state = aggregateModelStatus(status({ bytesRequired: 10, bytesPresent: 99 }));
    expect(state).toEqual({ status: "missing", bytesRequired: 0 });
  });

  it("lets a download in flight outrank everything, since that is what the user is watching", () => {
    const state = aggregateModelStatus(status({ downloading: true }));
    expect(state).toEqual({ status: "downloading", progress: null });
  });
});

describe("formatMegabytes", () => {
  it("quotes decimal megabytes, the unit a download is advertised in", () => {
    expect(formatMegabytes(631_000_000)).toBe("631 MB");
    expect(formatMegabytes(643_854)).toBe("1 MB");
  });

  it("switches to gigabytes past a thousand rather than printing five digits", () => {
    expect(formatMegabytes(1_400_000_000)).toBe("1.4 GB");
  });

  it("floors at zero rather than printing a negative size", () => {
    expect(formatMegabytes(-5)).toBe("0 MB");
  });
});

describe("progressFromEvent", () => {
  it("reads the overall figures, which are what the bar renders", () => {
    expect(
      progressFromEvent({
        status: "downloading",
        file: "parakeet-tdt-0.6b-v2-int8/encoder.int8.onnx",
        file_index: 2,
        file_count: 5,
        file_received: 10,
        file_total: 20,
        overall_received: 300,
        overall_total: 762,
      }),
    ).toEqual({
      file: "parakeet-tdt-0.6b-v2-int8/encoder.int8.onnx",
      fileIndex: 2,
      fileCount: 5,
      received: 300,
      total: 762,
      verifying: false,
    });
  });

  it("has nothing to report for the events that carry no bytes", () => {
    expect(progressFromEvent({ status: "verifying", file: "a.onnx" })).toBeNull();
    expect(progressFromEvent({ status: "ready" })).toBeNull();
  });
});
