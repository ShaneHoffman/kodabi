import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { SETTINGS_CHANGED_EVENT } from "./events";

/*
 * The settings wire shapes, mirroring the Rust DTOs in
 * `crates/kodabi-core/src/settings.rs` (reused verbatim as the IPC types by
 * `src-tauri/src/settings_cmds.rs`). `RetentionPolicy` is internally tagged on
 * `policy`, so it deserializes here as a discriminated union — the same JSON
 * the core crate's `json_wire_shape_is_stable_for_the_frontend_mirror` test
 * locks.
 */

export type RetentionPolicy =
  | { policy: "keep_all" }
  | { policy: "keep_days"; days: number }
  | { policy: "discard_after_distill" };

/** The bare discriminant, e.g. for a Select's value. */
export type RetentionKind = RetentionPolicy["policy"];

/** Whether the always-on-top capture pill shows, split by how the capture
 * began. `auto_captures` is dormant: meeting auto-detection does not exist yet,
 * so nothing produces an auto-detected capture today. */
export type OverlaySettings = {
  manual_captures: boolean;
  auto_captures: boolean;
};

/** Mirrors `Theme` in `crates/kodabi-core/src/settings.rs`. "system" defers to
 * `prefers-color-scheme`, which is what `design/tokens.css` answers on its own. */
export type Theme = "system" | "light" | "dark";

/** Mirrors `AppearanceSettings` in `crates/kodabi-core/src/settings.rs`. */
export type AppearanceSettings = {
  theme: Theme;
};

/** Mirrors `MicCheckOutcome` in `crates/kodabi-core/src/settings.rs` —
 * internally tagged on `outcome`, flattened into `MicCheckResult` below. */
export type MicCheckOutcome =
  | { outcome: "headphones" }
  | { outcome: "speakers"; echo_db: number; delay_ms: number }
  | { outcome: "mic_silent" };

/** Mirrors `MicCheckResult` in `crates/kodabi-core/src/settings.rs`: the
 * outcome fields flattened alongside `measured_at`, an RFC 3339 UTC instant. */
export type MicCheckResult = MicCheckOutcome & {
  measured_at: string;
};

export type Settings = {
  consent_acknowledged: boolean;
  retention: RetentionPolicy;
  overlay: OverlaySettings;
  appearance: AppearanceSettings;
  mic_check: MicCheckResult | null;
};

/** The theme choices, in the order they are offered. */
export const THEME_OPTIONS: { value: Theme; label: string }[] = [
  { value: "system", label: "Match the system" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

/** Default day count seeded into the "keep for N days" control before the user
 * commits one (FOUNDING_DOC §7's lean is "keep transcript N days"). */
export const DEFAULT_KEEP_DAYS = 30;

/** The retention choices, shared by the consent nudge and the Settings view so
 * the two surfaces never drift. */
export const RETENTION_OPTIONS: { value: RetentionKind; label: string }[] = [
  { value: "keep_all", label: "Keep all transcripts" },
  { value: "keep_days", label: "Keep for a number of days" },
  { value: "discard_after_distill", label: "Discard after distilling" },
];

/** Clamps a day count to the backend's floor (NonZeroU32 rejects 0), so a
 * blank or zero field never reaches the command as an invalid policy. */
export function clampDays(days: number): number {
  return Number.isFinite(days) && days >= 1 ? Math.floor(days) : 1;
}

/** Assembles a `RetentionPolicy` from a discriminant + a day count (the day
 * count is ignored unless the kind is `keep_days`). */
export function buildRetentionPolicy(kind: RetentionKind, days: number): RetentionPolicy {
  switch (kind) {
    case "keep_days":
      return { policy: "keep_days", days: clampDays(days) };
    case "discard_after_distill":
      return { policy: "discard_after_distill" };
    case "keep_all":
      return { policy: "keep_all" };
  }
}

export function getSettings(): Promise<Settings> {
  return invoke<Settings>("get_settings");
}

export function setRetentionPolicy(policy: RetentionPolicy): Promise<Settings> {
  return invoke<Settings>("set_retention_policy", { policy });
}

/** Sets both overlay flags at once. The backend re-syncs the pill immediately,
 * so a change during a running capture takes effect right away. */
export function setCaptureOverlay(overlay: OverlaySettings): Promise<Settings> {
  return invoke<Settings>("set_capture_overlay", { overlay });
}

/** Sets the theme preference. The echoed result updates the calling window;
 * the `settings:changed` event carries it to the other two webviews, which have
 * no other way to hear about it (src/theme.ts). */
export function setAppearance(appearance: AppearanceSettings): Promise<Settings> {
  return invoke<Settings>("set_appearance", { appearance });
}

export function acknowledgeConsent(retention: RetentionPolicy): Promise<Settings> {
  return invoke<Settings>("acknowledge_consent", { retention });
}

/** Runs the mic test: plays a short tone through the default output device
 * while recording the default microphone, and returns the settings with the
 * result stored in `mic_check`. Rejects while a capture is running (the
 * backend refuses rather than fighting the live capture for the mic). */
export function runMicTest(): Promise<Settings> {
  return invoke<Settings>("run_mic_test");
}

/**
 * Loads the app settings once on mount. Not routed through `useVaultQuery`:
 * settings are machine-local config, not vault data, so they don't refetch on
 * a `kodabi:vault-changed` event. `setSettings` lets a mutation land its echoed
 * result immediately without a reload.
 */
export function useSettings(): {
  settings: Settings | null;
  loading: boolean;
  error: string | null;
  setSettings: (settings: Settings) => void;
} {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    getSettings()
      .then((loaded) => {
        if (!active) return;
        setSettings(loaded);
        setError(null);
      })
      .catch((err) => {
        if (active) setError(String(err));
      })
      .finally(() => {
        if (active) setLoading(false);
      });

    // Stay in sync with mutations that land elsewhere (the consent nudge
    // acknowledging while this view is mounted), not just our own setSettings.
    listen<Settings>(SETTINGS_CHANGED_EVENT, (event) => {
      if (!active) return;
      setSettings(event.payload);
      setError(null);
    }).then((fn) => {
      if (active) {
        unlisten = fn;
      } else {
        fn();
      }
    });

    return () => {
      active = false;
      unlisten?.();
    };
  }, []);

  return { settings, loading, error, setSettings };
}
