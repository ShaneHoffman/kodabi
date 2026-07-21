import { useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  buildRetentionPolicy,
  DEFAULT_KEEP_DAYS,
  RETENTION_OPTIONS,
  setAppearance,
  setCaptureOverlay,
  setRetentionPolicy,
  THEME_OPTIONS,
  useSettings,
  type OverlaySettings,
  type RetentionKind,
  type Theme,
} from "../../useSettings";
import { applyReduceMotion, readReduceMotion } from "../../reduceMotion";
import { INDEX_STATE_EVENT } from "../../events";
import { useTauriEvent } from "../../useTauriEvent";
import { Button } from "../ui/Button";
import { Select } from "../ui/Select";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";
import "./SettingsView.css";

/** The capture toggle's global shortcut, mirroring `DEFAULT_TOGGLE_SHORTCUT`
 * in `src-tauri/src/capture_control.rs`. Rendered rather than editable: the
 * backend registers it at startup and offers no rebinding command yet, and a
 * field that silently fails to save is worse than a value that plainly is
 * what it is. */
const CAPTURE_SHORTCUT = "Ctrl + Shift + K";

const TABS = ["Privacy", "Appearance", "Capture"] as const;
type Tab = (typeof TABS)[number];

/** The `index:state` payload, mirroring the Rust `IndexStateEvent` tagged enum
 * in `src-tauri/src/index_state.rs`. */
type IndexStateEvent =
  | { status: "rebuilding" }
  | { status: "ready"; notes: number }
  | { status: "error"; message: string };

type RebuildStatus = { status: "idle" } | IndexStateEvent;

/**
 * One setting: its name on the left, its control right-aligned into the
 * shared column. Everything on this screen is one of these, which is what
 * makes the column scannable — the eye runs down the controls, not the prose.
 */
function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="settings__row">
      <span className="text-label text-text-soft">{label}</span>
      <div className="settings__control">{children}</div>
    </div>
  );
}

/** The one short line a setting is allowed to explain itself with. Never a
 * paragraph: a config panel that argues with you is a config panel nobody
 * finishes reading. */
function SubLabel({ children }: { children: ReactNode }) {
  return <p className="settings__sublabel text-cap text-text-faint">{children}</p>;
}

/** A boolean whose state is its geometry. `role="switch"` rather than a
 * checkbox: the platform semantics for "this takes effect immediately" differ
 * from "this is one of several things you are about to submit". */
function Toggle({
  label,
  checked,
  onChange,
  disabled,
}: {
  label: string;
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className="settings__toggle ui-focus-ring disabled:cursor-not-allowed"
    >
      <span className="settings__knob" />
    </button>
  );
}

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

  return (
    <>
      <Row label="Search index">
        <Button
          onClick={rebuild}
          loading={state.status === "rebuilding"}
          loadingLabel="Rebuilding…"
          className="text-label"
        >
          Rebuild
        </Button>
      </Row>
      <SubLabel>Reconstructs the index from your note files. Safe to run anytime.</SubLabel>
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
    </>
  );
}

/**
 * Settings — the app's CONFIG PANEL, and it announces that before a word is
 * read: a small title, no list anywhere, and a horizontal tab rail that
 * filters the page rather than navigating it.
 *
 * Every setting is one label-and-control row on a shared grid, so all the
 * controls line up into a single column you can run your eye down. Controls
 * are raised chips; read-only values stay plain text or a mono chip and take
 * no chevron, so "you can change this" and "this is how it is" are told apart
 * by shape rather than by being greyed out.
 */
export function SettingsView() {
  const { settings, error, setSettings } = useSettings();
  const [tab, setTab] = useState<Tab>("Privacy");

  // Raw input string so the field can be cleared mid-edit rather than snapping
  // to 0; `buildRetentionPolicy` parses and clamps it on apply.
  const [days, setDays] = useState(String(DEFAULT_KEEP_DAYS));
  const [saveError, setSaveError] = useState<string | null>(null);
  const [overlayError, setOverlayError] = useState<string | null>(null);
  // Which async write is in flight, so the control that started it can say so
  // rather than flipping instantly and silently reverting if it fails.
  const [savingRetention, setSavingRetention] = useState(false);
  const [savingOverlay, setSavingOverlay] = useState(false);
  const [appearanceError, setAppearanceError] = useState<string | null>(null);
  const [savingAppearance, setSavingAppearance] = useState(false);
  // Set once the day field has been committed, so the save is visible: this
  // control persists on Enter or blur, and without an acknowledgement a
  // keyboard user has no signal that anything happened.
  const [daysSaved, setDaysSaved] = useState(false);
  // Seeded from storage during render rather than an effect — it is a plain
  // synchronous read (src/reduceMotion.ts).
  const [reduceMotion, setReduceMotion] = useState(readReduceMotion);

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

  // The window does not re-theme from here: src/theme.ts listens for the
  // settings event and applies it, so all three webviews change together
  // rather than only the one holding this control.
  const applyAppearanceTheme = async (theme: Theme) => {
    setAppearanceError(null);
    setSavingAppearance(true);
    try {
      setSettings(await setAppearance({ theme }));
    } catch (err) {
      setAppearanceError(String(err));
    } finally {
      setSavingAppearance(false);
    }
  };

  return (
    <ViewFrame variant="panel" eyebrow="System" title="Settings">
      {/* The rail filters; it does not navigate. Real tab semantics, so a
          screen reader announces it as a filter rather than as a second set
          of destinations competing with the sidebar. */}
      <div className="mt-md">
        <div className="settings__tabs" role="tablist" aria-label="Settings categories">
          {TABS.map((name) => (
            <button
              key={name}
              type="button"
              role="tab"
              aria-selected={tab === name}
              onClick={() => setTab(name)}
              className={`settings__tab ui-focus-ring text-label font-semibold ${
                tab === name ? "text-text" : "text-text-faint"
              }`}
            >
              {name}
            </button>
          ))}
        </div>
        <div className="settings__rule" />
      </div>

      {error && (
        <StatusMessage variant="error">Couldn&apos;t load settings: {error}</StatusMessage>
      )}

      {settings && (
        <div className="settings__rows mt-sm">
          {tab === "Privacy" && (
            <>
              <Row label="Recording consent">
                {/* Read-only, so it stays plain text with a check glyph — no
                    chip, no chevron, nothing that invites a click. */}
                <span className="inline-flex items-center gap-2xs text-label text-text-faint">
                  <svg width="13" height="13" viewBox="0 0 14 14" fill="none" aria-hidden="true">
                    <path
                      d="M2.5 7.5l3 3 6-7"
                      stroke="currentColor"
                      strokeWidth="1.5"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    />
                  </svg>
                  {settings.consent_acknowledged ? "Acknowledged" : "Not yet shown"}
                </span>
              </Row>

              <Row label="Retention">
                <Select
                  hideLabel
                  label="Retention"
                  value={kind}
                  onChange={(value) => apply(value as RetentionKind, Number(days))}
                  options={RETENTION_OPTIONS}
                  disabled={savingRetention}
                />
              </Row>
              <SubLabel>Older captures are removed automatically.</SubLabel>

              {kind === "keep_days" && (
                <Row label="Days to keep">
                  <input
                    type="number"
                    min={1}
                    value={days}
                    aria-label="Days to keep"
                    className="settings__chip ui-focus-ring w-24 text-right text-label text-text"
                    onChange={(event) => {
                      setDays(event.target.value);
                      setDaysSaved(false);
                    }}
                    // Enter as well as blur. Blur-only made this the one setting
                    // a keyboard user could change without ever being told it
                    // saved, and without a way to commit it deliberately
                    // (docs/DESIGN_SYSTEM.md §6).
                    onKeyDown={(event) => {
                      if (event.key === "Enter") {
                        event.preventDefault();
                        void apply("keep_days", Number(days));
                      }
                    }}
                    onBlur={() => apply("keep_days", Number(days))}
                  />
                </Row>
              )}
              {daysSaved && kind === "keep_days" && (
                <StatusMessage variant="status" compact>
                  Saved.
                </StatusMessage>
              )}
              {saveError && (
                <StatusMessage variant="error" compact>
                  Couldn&apos;t save: {saveError}
                </StatusMessage>
              )}
            </>
          )}

          {tab === "Appearance" && (
            <>
              <Row label="Theme">
                <Select
                  hideLabel
                  label="Theme"
                  value={settings.appearance.theme}
                  onChange={(value) => applyAppearanceTheme(value as Theme)}
                  options={THEME_OPTIONS}
                  disabled={savingAppearance}
                />
              </Row>
              <SubLabel>Follows your OS light and dark setting.</SubLabel>

              <Row label="Reduce motion">
                <Toggle
                  label="Reduce motion"
                  checked={reduceMotion}
                  onChange={(next) => {
                    // Applied in the handler, not an effect: it happens
                    // because the user did something
                    // (.claude/rules/no-use-effect.md).
                    setReduceMotion(next);
                    applyReduceMotion(next);
                  }}
                />
              </Row>
              <SubLabel>
                Holds the listening glow and the caret still. Your OS setting
                already does this; here it applies to Kodabi alone.
              </SubLabel>

              {appearanceError && (
                <StatusMessage variant="error" compact>
                  Couldn&apos;t save: {appearanceError}
                </StatusMessage>
              )}
            </>
          )}

          {tab === "Capture" && (
            <>
              <Row label="Global shortcut">
                {/* Mono, because it is a key sequence — the same voice the
                    palette hint and every path in the app uses. */}
                <span className="settings__chip font-mono text-action text-text">
                  {CAPTURE_SHORTCUT}
                </span>
              </Row>
              <SubLabel>Starts and stops a capture from anywhere.</SubLabel>

              <Row label="Capture pill">
                <Toggle
                  label="Show the capture pill during captures you start"
                  checked={settings.overlay.manual_captures}
                  disabled={savingOverlay}
                  onChange={(checked) => applyOverlay({ manual_captures: checked })}
                />
              </Row>
              <SubLabel>
                Stays on top of full screen apps, so a recording is never invisible.
              </SubLabel>

              <Row label="Pill for auto detected captures">
                <Toggle
                  label="Show the capture pill for auto detected captures"
                  checked={settings.overlay.auto_captures}
                  disabled={savingOverlay}
                  onChange={(checked) => applyOverlay({ auto_captures: checked })}
                />
              </Row>
              <SubLabel>
                Kodabi does not detect meetings on its own yet, so this has nothing
                to act on today.
              </SubLabel>

              <RebuildIndexControl />

              {overlayError && (
                <StatusMessage variant="error" compact>
                  Couldn&apos;t save: {overlayError}
                </StatusMessage>
              )}
            </>
          )}
        </div>
      )}
    </ViewFrame>
  );
}
