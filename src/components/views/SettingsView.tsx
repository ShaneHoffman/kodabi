import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  buildRetentionPolicy,
  DEFAULT_KEEP_DAYS,
  RETENTION_OPTIONS,
  setRetentionPolicy,
  useSettings,
  type RetentionKind,
} from "../../useSettings";
import { INDEX_STATE_EVENT } from "../../events";
import { useTauriEvent } from "../../useTauriEvent";
import { Button } from "../ui/Button";
import { Select } from "../ui/Select";
import { TextField } from "../ui/TextField";

/** The `index:state` payload, mirroring the Rust `IndexStateEvent` tagged enum
 * in `src-tauri/src/index_state.rs`. */
type IndexStateEvent =
  | { status: "rebuilding" }
  | { status: "ready"; notes: number }
  | { status: "error"; message: string };

type RebuildStatus = { status: "idle" } | IndexStateEvent;

/**
 * Rebuilds the note index from the files on disk. The index is a derived cache
 * the file watcher normally keeps live; this is the manual "reconstruct from
 * scratch" escape hatch. The command returns as soon as the job is queued, so
 * progress is driven entirely by the `index:state` event.
 */
function RebuildIndexControl() {
  const [state, setState] = useState<RebuildStatus>({ status: "idle" });
  useTauriEvent<IndexStateEvent>(INDEX_STATE_EVENT, (payload) => setState(payload));

  const rebuild = async () => {
    setState({ status: "rebuilding" });
    try {
      await invoke("rebuild_index");
    } catch (err) {
      // A command-level failure (index unavailable this session) never emits an
      // event, so surface it here.
      setState({ status: "error", message: String(err) });
    }
  };

  const busy = state.status === "rebuilding";
  return (
    <div className="flex flex-col gap-sm">
      <div>
        <Button onClick={rebuild} disabled={busy}>
          {busy ? "Rebuilding..." : "Rebuild index"}
        </Button>
      </div>
      <p className="text-cap text-text-faint">
        Reconstructs the search index from your note files. Safe to run anytime.
      </p>
      {state.status === "ready" && (
        <p className="text-cap text-text-faint">
          Index rebuilt. {state.notes} {state.notes === 1 ? "note" : "notes"} indexed.
        </p>
      )}
      {state.status === "error" && (
        <p role="alert" className="text-cap text-text-soft">
          Couldn't rebuild the index: {state.message}
        </p>
      )}
    </div>
  );
}

/**
 * The Settings view — Privacy only for now. Shows whether recording consent
 * has been acknowledged and lets the user change the raw-transcript retention
 * policy, which persists (and prunes) immediately on change.
 */
export function SettingsView() {
  const { settings, error, setSettings } = useSettings();
  // Raw input string so the field can be cleared mid-edit rather than snapping
  // to 0; `buildRetentionPolicy` parses and clamps it on apply.
  const [days, setDays] = useState(String(DEFAULT_KEEP_DAYS));
  const [saveError, setSaveError] = useState<string | null>(null);

  // Seed the day field from the stored policy the first time a keep_days value
  // is seen, so editing starts from the stored value rather than the
  // placeholder default. The once-only flag means a later `settings` change (an
  // apply() echoing its result back) never overwrites an edit the user is
  // still typing.
  const [seededDays, setSeededDays] = useState(false);
  if (!seededDays && settings?.retention.policy === "keep_days") {
    setSeededDays(true);
    setDays(String(settings.retention.days));
  }

  const kind: RetentionKind = settings?.retention.policy ?? "keep_all";

  const apply = async (nextKind: RetentionKind, nextDays: number) => {
    setSaveError(null);
    try {
      const updated = await setRetentionPolicy(buildRetentionPolicy(nextKind, nextDays));
      setSettings(updated);
    } catch (err) {
      setSaveError(String(err));
    }
  };

  return (
    <section className="flex min-h-full flex-col p-xl">
      <div className="mx-auto flex w-full max-w-content flex-col gap-lg">
        <header className="flex flex-col gap-3xs">
          <p className="text-eyebrow uppercase tracking-wide text-text-faint">Settings</p>
          <h2 className="font-serif text-h2 text-text">Privacy</h2>
        </header>

        {error && <p className="text-body text-text-soft">Couldn't load settings: {error}</p>}

        {settings && (
          <div className="flex max-w-measure flex-col gap-lg">
            <p className="text-body text-text-soft">
              {settings.consent_acknowledged
                ? "Recording consent acknowledged."
                : "The consent nudge is shown before your first capture."}
            </p>

            <div className="flex flex-col gap-sm">
              <Select
                label="Retention"
                value={kind}
                onChange={(value) => apply(value as RetentionKind, Number(days))}
                options={RETENTION_OPTIONS}
              />
              {kind === "keep_days" && (
                <TextField
                  label="Days to keep"
                  type="number"
                  min={1}
                  value={days}
                  onChange={(event) => setDays(event.target.value)}
                  onBlur={() => apply("keep_days", Number(days))}
                />
              )}
              <p className="text-cap text-text-faint">
                Discard after distilling applies to captures from now on. Sessions
                distilled before you chose it are not removed.
              </p>
              {saveError && (
                <p role="alert" className="text-cap text-text-soft">
                  Couldn't save: {saveError}
                </p>
              )}
            </div>
          </div>
        )}

        <div className="flex max-w-measure flex-col gap-sm">
          <h3 className="font-serif text-h3 text-text">Knowledge base</h3>
          <RebuildIndexControl />
        </div>
      </div>
    </section>
  );
}
