import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  buildRetentionPolicy,
  DEFAULT_KEEP_DAYS,
  RETENTION_OPTIONS,
  setCaptureOverlay,
  setRetentionPolicy,
  useSettings,
  type OverlaySettings,
  type RetentionKind,
} from "../../useSettings";
import { INDEX_STATE_EVENT } from "../../events";
import { useTauriEvent } from "../../useTauriEvent";
import { Button } from "../ui/Button";
import { Checkbox } from "../ui/Checkbox";
import { Select } from "../ui/Select";
import { StatusMessage } from "../ui/StatusMessage";
import { TextField } from "../ui/TextField";
import { ViewFrame } from "../ui/ViewFrame";

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
        <Button onClick={rebuild} loading={busy} loadingLabel="Rebuilding…">
          Rebuild index
        </Button>
      </div>
      <p className="text-cap text-text-faint">
        Reconstructs the search index from your note files. Safe to run anytime.
      </p>
      {state.status === "ready" && (
        <StatusMessage variant="status" compact>
          Index rebuilt. {state.notes} {state.notes === 1 ? "note" : "notes"} indexed.
        </StatusMessage>
      )}
      {state.status === "error" && (
        <StatusMessage variant="error" compact>
          Couldn&apos;t rebuild the index: {state.message}
        </StatusMessage>
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
  const [overlayError, setOverlayError] = useState<string | null>(null);
  // Which async write is in flight, so the control that started it can say so
  // rather than flipping instantly and silently reverting if it fails.
  const [savingRetention, setSavingRetention] = useState(false);
  const [savingOverlay, setSavingOverlay] = useState(false);
  // Set once the day field has been committed, so the save is visible: this
  // control persists on Enter or blur, and without an acknowledgement a
  // keyboard user has no signal that anything happened.
  const [daysSaved, setDaysSaved] = useState(false);

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
    setSavingRetention(true);
    try {
      const updated = await setRetentionPolicy(buildRetentionPolicy(nextKind, nextDays));
      setSettings(updated);
      setDaysSaved(true);
    } catch (err) {
      setSaveError(String(err));
      setDaysSaved(false);
    } finally {
      setSavingRetention(false);
    }
  };

  // Both flags travel together (the command takes the whole struct), so each
  // toggle sends the stored pair with its own field replaced. Its own error
  // slot, not the retention one: an error has to appear beside the control
  // that failed, or it reads as a failure of whatever it sits under.
  const applyOverlay = async (change: Partial<OverlaySettings>) => {
    if (!settings) return;
    setOverlayError(null);
    setSavingOverlay(true);
    try {
      const updated = await setCaptureOverlay({ ...settings.overlay, ...change });
      setSettings(updated);
    } catch (err) {
      setOverlayError(String(err));
    } finally {
      setSavingOverlay(false);
    }
  };

  return (
    <ViewFrame eyebrow="Settings" title="Privacy">
      {error && (
        <StatusMessage variant="error">Couldn&apos;t load settings: {error}</StatusMessage>
      )}

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
              disabled={savingRetention}
            />
            {kind === "keep_days" && (
              <TextField
                label="Days to keep"
                type="number"
                min={1}
                value={days}
                hint="Saves when you press Enter or leave the field."
                onChange={(event) => {
                  setDays(event.target.value);
                  setDaysSaved(false);
                }}
                // Enter as well as blur. Blur-only made this the one setting a
                // keyboard user could change without ever being told it saved,
                // and without a way to commit it deliberately
                // (docs/DESIGN_SYSTEM.md §6).
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    void apply("keep_days", Number(days));
                  }
                }}
                onBlur={() => apply("keep_days", Number(days))}
              />
            )}
            {daysSaved && kind === "keep_days" && (
              <StatusMessage variant="status" compact>
                Saved.
              </StatusMessage>
            )}
            <p className="text-cap text-text-faint">
              Discard after distilling applies to captures from now on. Sessions
              distilled before you chose it are not removed.
            </p>
            {saveError && (
              <StatusMessage variant="error" compact>
                Couldn&apos;t save: {saveError}
              </StatusMessage>
            )}
          </div>
        </div>
      )}

      {settings && (
        <div className="flex max-w-measure flex-col gap-sm">
          <h3 className="font-serif text-h3 text-text">Capture</h3>
          <Checkbox
            label="Show the capture pill during captures you start"
            hint="A small pill stays on top of full screen apps while a capture is running, so a recording is never invisible. Drag it anywhere, or hide it for the current capture."
            data-testid="overlay-manual-captures"
            checked={settings.overlay.manual_captures}
            disabled={savingOverlay}
            onChange={(checked) => applyOverlay({ manual_captures: checked })}
          />
          <Checkbox
            label="Show the capture pill for auto detected captures"
            hint="Applies when meeting auto detection arrives. Kodabi does not detect meetings on its own yet, so this setting has nothing to act on today."
            data-testid="overlay-auto-captures"
            checked={settings.overlay.auto_captures}
            disabled={savingOverlay}
            onChange={(checked) => applyOverlay({ auto_captures: checked })}
          />
          {overlayError && (
            <StatusMessage variant="error" compact>
              Couldn&apos;t save: {overlayError}
            </StatusMessage>
          )}
        </div>
      )}

      <div className="flex max-w-measure flex-col gap-sm">
        <h3 className="font-serif text-h3 text-text">Knowledge base</h3>
        <RebuildIndexControl />
      </div>
    </ViewFrame>
  );
}
