import { useRef, useState } from "react";
import { startCapture } from "../../captureControl";
import { backendCopy } from "../../errorCopy";
import {
  acknowledgeConsent,
  buildRetentionPolicy,
  DEFAULT_KEEP_DAYS,
  RETENTION_OPTIONS,
  type RetentionKind,
} from "../../useSettings";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Field } from "../ui/Field";
import { Select } from "../ui/Select";
import { StatusMessage } from "../ui/StatusMessage";

type Props = {
  onClose: () => void;
};

/**
 * The one-time recording-consent nudge, shown before the very first capture
 * (FOUNDING_DOC §3.7). It pairs the consent ask with the retention choice —
 * nothing is recorded until the user acknowledges, and their retention policy
 * is set in the same step.
 *
 * The shell is the Grove `Dialog`: base-ui owns the focus trap, Escape, the
 * scrim press and the focus restore, which is what retires the hand-rolled Tab
 * wrapper this used to carry. Focus opens on the PRIMARY action rather than the
 * first control, because the retention default is already the safe one and the
 * thing the user came here to do is acknowledge.
 *
 * The retention `Select` is still the pre-Grove combobox (its base-ui
 * replacement is its own ticket). It stops Escape from bubbling while its list
 * is open, which is exactly what keeps that Escape from closing the whole
 * dialog — a behaviour worth knowing about, since it is load-bearing rather
 * than incidental, and pinned by a test.
 */
export function ConsentNudge({ onClose }: Props) {
  const [kind, setKind] = useState<RetentionKind>("keep_all");
  // Held as the raw input string so the field can be cleared mid-edit rather
  // than snapping to 0; `buildRetentionPolicy` parses and clamps it on submit.
  const [days, setDays] = useState(String(DEFAULT_KEEP_DAYS));
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The one place the app asks who you are. This gate is the only surface
  // every install passes before its first capture, which makes it the last
  // moment the answer is still ahead of the first meeting rather than behind
  // it. Optional: blank simply means not yet, and Settings holds the field.
  const [displayName, setDisplayName] = useState("");

  const primaryRef = useRef<HTMLButtonElement>(null);

  const acknowledge = async () => {
    setSubmitting(true);
    setError(null);
    // Persist consent + the chosen policy first, then start the capture the
    // user's toggle intended — start_capture's own gate passes now. The two
    // steps fail differently, so they're caught separately: a failed persist
    // means consent was NOT granted (the nudge will correctly reappear on the
    // next toggle), while a failed start leaves consent granted.
    try {
      await acknowledgeConsent(
        buildRetentionPolicy(kind, Number(days)),
        displayName.trim(),
      );
    } catch (err) {
      setError(
        backendCopy(err, "Couldn't save your choice, so recording stays off. Try again."),
      );
      setSubmitting(false);
      return;
    }
    try {
      await startCapture();
      onClose();
    } catch (err) {
      setError(
        backendCopy(
          err,
          "Couldn't start recording. Check your microphone in Windows sound settings, then try again.",
        ),
      );
      setSubmitting(false);
    }
  };

  return (
    <Dialog
      open
      onDismiss={onClose}
      labelledBy="consent-nudge-title"
      initialFocus={primaryRef}
      className="flex flex-col gap-4"
    >
      <h2 id="consent-nudge-title" className="text-[15px] font-semibold text-ink">
        Before your first capture
      </h2>

      <div className="flex flex-col gap-2.5 text-[13px] leading-relaxed text-ink-read">
        <p>
          Kodabi records your microphone and system audio while the listening
          indicator is green. It only ever records while that indicator shows.
        </p>
        <p>
          Please announce your recordings. In many places everyone on a call
          has to consent before you record, and the rules differ by where each
          person is sitting.
        </p>
        <p>
          Transcripts and recordings stay on this device as plain files.
          Choose how long Kodabi keeps them:
        </p>
      </div>

      {/* Every control in here goes inert while the acknowledgement is in
          flight, and NONE of them uses the native `disabled` attribute to do
          it. This is the case docs/DESIGN_SYSTEM.md §6 singles out as the
          worst one: disabling the focused element blurs it and focus resets
          to <body>, which is outside the dialog. The keyboard then has nowhere
          in the modal to Tab from, and the user is left inside a surface they
          can no longer leave with the keyboard. `busy` / `readOnly` /
          `loading` all keep their control focusable. */}
      <Select
        label="Retention"
        value={kind}
        onChange={(value) => setKind(value as RetentionKind)}
        options={RETENTION_OPTIONS}
        busy={submitting}
      />
      {/* "Days to keep", not Settings' "Days". Deliberate divergence: that
          screen nests this field inside a role="group" named Retention, so the
          group supplies the rest of the meaning. This dialog is a flat column of
          fields with nothing to lean on, so the label carries all of it. */}
      {kind === "keep_days" && (
        <Field
          label="Days to keep"
          type="number"
          min={1}
          value={days}
          // readOnly, not disabled: a read-only input stays focusable and in
          // the tab order, and refusing the edit is exactly what is wanted
          // here anyway.
          readOnly={submitting}
          onChange={(event) => setDays(event.target.value)}
        />
      )}

      {/* Last of the fields, and deliberately after the retention choice: the
          consent and what happens to the recording are what the dialog is for,
          and this is a convenience that rides along. Nothing gates on it. */}
      <Field
        label="Your name"
        hint="Optional. Lets Kodabi tell which commitments in a meeting are yours."
        placeholder="Not set"
        value={displayName}
        readOnly={submitting}
        onChange={(event) => setDisplayName(event.target.value)}
      />

      {error && <StatusMessage variant="error" compact>{error}</StatusMessage>}

      <div className="flex items-center justify-end gap-2.5">
        {/* `loading` with no loadingLabel: the label is unchanged (Button
            falls back to its children), and all this buys is the inert
            treatment that keeps the button focusable. There is nothing to
            report here — the primary beside it is the control doing the work
            and the one that says so. */}
        <Button variant="quiet" onClick={onClose} loading={submitting}>
          Not now
        </Button>
        <Button
          ref={primaryRef}
          onClick={acknowledge}
          loading={submitting}
          loadingLabel="Starting…"
        >
          I understand, start capture
        </Button>
      </div>
    </Dialog>
  );
}
