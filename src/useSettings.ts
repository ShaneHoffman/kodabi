import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** Backend event carrying the new settings after a mutation, so a view already
 * mounted when the change lands (the Settings view open while the consent nudge
 * acknowledges) refreshes without a reload. Mirrors `SETTINGS_CHANGED_EVENT` in
 * `src-tauri/src/settings_cmds.rs`. */
const SETTINGS_CHANGED_EVENT = "settings:changed";

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

export type Settings = {
  consent_acknowledged: boolean;
  retention: RetentionPolicy;
};

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

export function acknowledgeConsent(retention: RetentionPolicy): Promise<Settings> {
  return invoke<Settings>("acknowledge_consent", { retention });
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
