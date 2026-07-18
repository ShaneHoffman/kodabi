import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  acknowledgeConsent,
  buildRetentionPolicy,
  DEFAULT_KEEP_DAYS,
  RETENTION_OPTIONS,
  type RetentionKind,
} from "../useSettings";
import { Button } from "./ui/Button";
import { Select } from "./ui/Select";
import { TextField } from "./ui/TextField";
import "./ConsentNudge.css";

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
  const [days, setDays] = useState(DEFAULT_KEEP_DAYS);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const panelRef = useRef<HTMLDivElement>(null);
  // Whether the current pointer gesture started on the backdrop itself, so a
  // press that began inside the panel never dismisses (the palette's guard).
  const backdropPressed = useRef(false);

  // Focus the primary action on open; restore focus on close. Focused by id
  // rather than a ref since `Button` doesn't forward one.
  useEffect(() => {
    const previous = document.activeElement;
    document.getElementById(PRIMARY_ID)?.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) {
        previous.focus();
      }
    };
  }, []);

  const acknowledge = async () => {
    setSubmitting(true);
    setError(null);
    try {
      // Persist consent + the chosen policy first, then start the capture the
      // user's toggle intended — start_capture's own gate passes now.
      await acknowledgeConsent(buildRetentionPolicy(kind, days));
      await invoke("start_capture");
      onClose();
    } catch (err) {
      // Consent is already granted; surface the failure and let the user retry
      // or dismiss (the nudge won't reappear on the next toggle).
      setError(String(err));
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
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-bg-sink/60 px-md pt-2xl"
      onPointerDown={(event) => {
        backdropPressed.current = event.target === event.currentTarget;
      }}
      onClick={(event) => {
        if (backdropPressed.current && event.target === event.currentTarget) {
          onClose();
        }
      }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="consent-nudge-title"
        onKeyDown={onKeyDown}
        className="consent-nudge__panel flex w-full max-w-measure flex-col gap-md rounded-md bg-surface p-md"
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
        />
        {kind === "keep_days" && (
          <TextField
            label="Days to keep"
            type="number"
            min={1}
            value={days}
            onChange={(event) => setDays(Number(event.target.value))}
          />
        )}

        {error && (
          <p role="alert" className="text-cap text-text-soft">
            Couldn't start capture: {error}
          </p>
        )}

        <div className="flex items-center justify-end gap-sm">
          <Button variant="quiet" onClick={onClose} disabled={submitting}>
            Not now
          </Button>
          <Button id={PRIMARY_ID} onClick={acknowledge} disabled={submitting}>
            I understand, start capture
          </Button>
        </div>
      </div>
    </div>
  );
}
