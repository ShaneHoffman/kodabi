import { useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useDialogFocus } from "../useDialogFocus";
import {
  acknowledgeConsent,
  buildRetentionPolicy,
  DEFAULT_KEEP_DAYS,
  RETENTION_OPTIONS,
  type RetentionKind,
} from "../useSettings";
import { Button } from "./ui/Button";
import { Overlay } from "./ui/Overlay";
import { Select } from "./ui/Select";
import { StatusMessage } from "./ui/StatusMessage";
import { TextField } from "./ui/TextField";

type Props = {
  onClose: () => void;
};

// Focusable descendants for the dialog's Tab-wrap trap. The Select renders its
// options as non-tabbable list items, so only the trigger, day field, and
// buttons participate.
const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])';

const PRIMARY_ID = "consent-nudge-primary";

/**
 * The one-time recording-consent nudge, shown before the very first capture
 * (FOUNDING_DOC §3.7). It pairs the consent ask with the retention choice —
 * nothing is recorded until the user acknowledges, and their retention policy
 * is set in the same step. Same overlay shape as the command palette
 * (role=dialog, aria-modal, Escape/backdrop dismiss, focus save/restore).
 */
export function ConsentNudge({ onClose }: Props) {
  const [kind, setKind] = useState<RetentionKind>("keep_all");
  // Held as the raw input string so the field can be cleared mid-edit rather
  // than snapping to 0; `buildRetentionPolicy` parses and clamps it on submit.
  const [days, setDays] = useState(String(DEFAULT_KEEP_DAYS));
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const panelRef = useRef<HTMLDivElement>(null);

  // Focus the primary action on open; restore focus on close.
  useDialogFocus(() => document.getElementById(PRIMARY_ID));

  const acknowledge = async () => {
    setSubmitting(true);
    setError(null);
    // Persist consent + the chosen policy first, then start the capture the
    // user's toggle intended — start_capture's own gate passes now. The two
    // steps fail differently, so they're caught separately: a failed persist
    // means consent was NOT granted (the nudge will correctly reappear on the
    // next toggle), while a failed start leaves consent granted.
    try {
      await acknowledgeConsent(buildRetentionPolicy(kind, Number(days)));
    } catch (err) {
      setError(`Couldn't save your choice: ${String(err)}`);
      setSubmitting(false);
      return;
    }
    try {
      await invoke("start_capture");
      onClose();
    } catch (err) {
      setError(`Couldn't start capture: ${String(err)}`);
      setSubmitting(false);
    }
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      // A child (the Select's open list) stops Escape from bubbling, so this
      // only fires when nothing inside is intercepting it.
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key !== "Tab") return;
    const focusables = panelRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE);
    if (!focusables || focusables.length === 0) return;
    const first = focusables[0];
    const last = focusables[focusables.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <Overlay
      onDismiss={onClose}
      labelledBy="consent-nudge-title"
      panelRef={panelRef}
      onKeyDown={onKeyDown}
      className="flex flex-col gap-md p-md"
    >
      <h2 id="consent-nudge-title" className="font-serif text-h2 text-text">
        Before your first capture
      </h2>

      <div className="flex flex-col gap-sm text-body text-text-soft">
        <p>
          Kodabi records your microphone and system audio while the listening
          indicator is green. It only ever records while that indicator shows.
        </p>
        <p>
          Please announce your recordings. Many places (Massachusetts among
          them) require everyone on a call to consent before you record.
        </p>
        <p>
          Transcripts stay on this device as plain files. Choose how long
          Kodabi keeps the raw transcripts:
        </p>
      </div>

      <Select
        label="Retention"
        value={kind}
        onChange={(value) => setKind(value as RetentionKind)}
        options={RETENTION_OPTIONS}
        disabled={submitting}
      />
      {kind === "keep_days" && (
        <TextField
          label="Days to keep"
          type="number"
          min={1}
          value={days}
          disabled={submitting}
          onChange={(event) => setDays(event.target.value)}
        />
      )}

      {error && <StatusMessage variant="error" compact>{error}</StatusMessage>}

      <div className="flex items-center justify-end gap-sm">
        <Button variant="quiet" onClick={onClose} disabled={submitting}>
          Not now
        </Button>
        <Button
          id={PRIMARY_ID}
          onClick={acknowledge}
          loading={submitting}
          loadingLabel="Starting…"
        >
          I understand, start capture
        </Button>
      </div>
    </Overlay>
  );
}
