import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { backendCopy } from "./errorCopy";
import { SETTINGS_CHANGED_EVENT } from "./events";
import type { NoteCategory } from "./useNotes";

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

/** Mirrors `Theme` in `crates/kodabi-core/src/settings.rs`. "system" resolves
 * `prefers-color-scheme` in `src/theme.ts`, which is where the `.day` class is
 * decided — the Grove tokens carry no media query of their own. */
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

/** Mirrors `LedgerSettings` in `crates/kodabi-core/src/settings.rs`: when an
 * untouched commitment reads as aging then stale, and how sure a conversation
 * has to be before a completion claim closes one without asking. */
export type LedgerSettings = {
  aging_after_days: number;
  stale_after_days: number;
  /** 0..1. At or below this a claim parks for review instead of closing. */
  conversation_autoclose: number;
};

/** Mirrors `CategoryPrefs` in `crates/kodabi-core/src/settings.rs`.
 * `enrollment_default` fills the `category_default` slot in the ledger's
 * `effective_mode`; `null` means the genre inherits the built-in default below
 * rather than meaning "tracked". */
export type CategoryPrefs = {
  enrollment_default: "tracked" | "context_only" | null;
};

/** Mirrors `CategorySettings` in `crates/kodabi-core/src/settings.rs`: one
 * entry per meeting genre, keyed by the snake_case spelling of the variant. */
export type CategorySettings = {
  standup: CategoryPrefs;
  one_on_one: CategoryPrefs;
  client: CategoryPrefs;
  working_session: CategoryPrefs;
  review: CategoryPrefs;
  all_hands: CategoryPrefs;
  observer: CategoryPrefs;
};

/** Which meeting kind a per-kind preference belongs to: the settings struct's
 * snake_case key, from the kebab-case spelling the frontmatter and the wire
 * use. Both spellings are the schema's, and this is the one place they meet on
 * this side, so a view never writes the mapping out again. */
export const CATEGORY_SETTING_KEYS: Record<NoteCategory, keyof CategorySettings> = {
  standup: "standup",
  "one-on-one": "one_on_one",
  client: "client",
  "working-session": "working_session",
  review: "review",
  "all-hands": "all_hands",
  observer: "observer",
};

/** What each genre enrolls when the user has not chosen. Mirrors
 * `kodabi_core::ledger::builtin_category_default`, which is the authority: two
 * genres are attended rather than transacted, so they contribute only what was
 * asked of you directly. Kept here so Settings can name the inherited value. */
export const BUILTIN_CATEGORY_DEFAULTS: Record<
  keyof CategorySettings,
  "tracked" | "context_only"
> = {
  standup: "tracked",
  one_on_one: "tracked",
  client: "tracked",
  working_session: "tracked",
  review: "tracked",
  all_hands: "context_only",
  observer: "context_only",
};

export type Settings = {
  consent_acknowledged: boolean;
  retention: RetentionPolicy;
  overlay: OverlaySettings;
  appearance: AppearanceSettings;
  mic_check: MicCheckResult | null;
  ledger: LedgerSettings;
  categories: CategorySettings;
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

/** Mirrors `kodabi_core::ledger::DEFAULT_CONVERSATION_AUTOCLOSE`. Only a
 * fallback for an unreadable field; the backend owns the real default. */
export const DEFAULT_CONVERSATION_AUTOCLOSE = 0.8;

/** The retention choices, shared by the consent nudge and the Settings view so
 * the two surfaces never drift. */
export const RETENTION_OPTIONS: { value: RetentionKind; label: string }[] = [
  { value: "keep_all", label: "Keep all transcripts and recordings" },
  { value: "keep_days", label: "Keep for a number of days" },
  { value: "discard_after_distill", label: "Discard after distilling" },
];

/** Clamps a day count to the backend's floor (NonZeroU32 rejects 0), so a
 * blank or zero field never reaches the command as an invalid policy. */
export function clampDays(days: number): number {
  return Number.isFinite(days) && days >= 1 ? Math.floor(days) : 1;
}

/** Clamps the auto-close confidence into the 0..1 the backend validates, so a
 * blank or out-of-range field never reaches the command. */
export function clampConfidence(confidence: number): number {
  if (!Number.isFinite(confidence)) return DEFAULT_CONVERSATION_AUTOCLOSE;
  return Math.min(1, Math.max(0, confidence));
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

/** Sets the commitment aging thresholds and the auto-close confidence. The
 * backend emits `ledger:changed` alongside `settings:changed`, so an open
 * Commitments view re-reads its tiers rather than keeping the old ones. */
export function setLedgerTuning(ledger: LedgerSettings): Promise<Settings> {
  return invoke<Settings>("set_ledger_tuning", { ledger });
}

/** Sets the per-genre enrollment defaults. Prospective by design: the backend
 * emits only `settings:changed`, because nothing in the ledger has moved yet.
 * Existing meetings are re-evaluated when one is recategorized or its own
 * tracking switch is flipped. */
export function setCategoryPrefs(categories: CategorySettings): Promise<Settings> {
  return invoke<Settings>("set_category_prefs", { categories });
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
        if (active)
          setError(
            backendCopy(
              err,
              "Couldn't load your settings. They are still saved; reopen this view to try again.",
            ),
          );
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
