import { useRef, useState } from "react";
import { formatMegabytes } from "../../models";
import { useModelStatus } from "../../useModelStatus";
import { useTimeout } from "../../useTimeout";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { StatusMessage } from "../ui/StatusMessage";
import { ModelDownloadProgress } from "../models/ModelDownloadProgress";

/** How long "Models ready." stays before the dialog closes itself. */
const CONFIRMATION_MS = 2500;

type Props = {
  onClose: () => void;
};

/**
 * The first-run ask: Kodabi needs its speech and search models, which are a
 * download rather than part of the installer.
 *
 * Dismissible, unlike the consent nudge it borrows its shape from. Consent is a
 * legal gate with nothing behind it; this is not — notes, search, chat and
 * quick capture all work with no models at all, and only transcription and
 * semantic search are waiting. A modal the user could not leave would be lying
 * about how stuck they are. Dismissal is session-only: readiness is re-derived
 * from `model_status` on every launch, so the ask returns next time, and the
 * permanent path is the Settings card.
 *
 * The download starts on a click and never on its own. 760 MB is the user's
 * decision to make, and an app that fetched it unasked would be at odds with
 * everything else Kodabi promises about staying on your machine.
 */
export function ModelDownloadNudge({ onClose }: Props) {
  const { state, start, cancel } = useModelStatus();
  const primaryRef = useRef<HTMLButtonElement>(null);

  // Whether this session has watched a download run. It is what separates the
  // two ways of arriving at "ready": finishing one here earns the confirmation
  // beat, while a returning user whose models were already installed must never
  // see a dialog at all. Adjusted during render, not in an effect
  // (.claude/rules/no-use-effect.md).
  const [sawDownloading, setSawDownloading] = useState(false);
  if (state.status === "downloading" && !sawDownloading) {
    setSawDownloading(true);
  }

  const ready = state.status === "ready";
  // Success clears itself; errors never do (docs/DESIGN_SYSTEM.md §3).
  useTimeout(onClose, ready ? CONFIRMATION_MS : null);

  // `unknown` is the pre-seed beat: showing anything then would flash an ask at
  // every returning user for as long as the status invoke takes to answer.
  if (state.status === "unknown") return null;
  // Nothing to ask for, and nothing this session did to confirm.
  if (ready && !sawDownloading) return null;
  return (
    <Dialog
      open
      onDismiss={onClose}
      labelledBy="model-nudge-title"
      initialFocus={primaryRef}
      className="flex flex-col gap-4"
    >
      <h2 id="model-nudge-title" className="text-[15px] font-semibold text-ink">
        {ready ? "Models ready." : "Set up transcription"}
      </h2>

      {ready && (
        <p className="text-[13px] leading-relaxed text-ink-read">
          Transcription and search are fully available.
        </p>
      )}

      {state.status === "missing" && (
        <div className="flex flex-col gap-2.5 text-[13px] leading-relaxed text-ink-read">
          <p>
            Kodabi transcribes meetings and searches your notes on this device.
            That needs two model files, a one time download of about{" "}
            {formatMegabytes(state.bytesRequired)}. Nothing you record ever
            leaves this machine.
          </p>
          <p className="text-[12px] text-ink-faint">
            Speech model: NVIDIA Parakeet, CC BY 4.0. Details in Settings.
          </p>
        </div>
      )}

      {state.status === "downloading" && (
        <div className="flex flex-col gap-2.5">
          <p className="text-[13px] leading-relaxed text-ink-read">
            You can keep working. Recordings you make now are saved and will be
            transcribed once this finishes.
          </p>
          <ModelDownloadProgress progress={state.progress} />
        </div>
      )}

      {state.status === "error" && (
        <div className="flex flex-col gap-2">
          <StatusMessage variant="error" compact>
            Couldn&apos;t finish the download: {state.message}
          </StatusMessage>
          <p className="text-[12px] text-ink-faint">
            Nothing else was affected. Trying again picks up where it stopped,
            and you can also start it from Settings.
          </p>
        </div>
      )}

      <div className="flex items-center justify-end gap-2.5">
        {state.status === "downloading" && (
          <>
            <Button variant="quiet" onClick={() => void cancel()}>
              Cancel download
            </Button>
            <Button ref={primaryRef} variant="quiet" onClick={onClose}>
              Hide
            </Button>
          </>
        )}
        {ready && (
          <Button ref={primaryRef} variant="quiet" onClick={onClose}>
            Close
          </Button>
        )}
        {(state.status === "missing" || state.status === "error") && (
          <>
            <Button variant="quiet" onClick={onClose}>
              Not now
            </Button>
            <Button ref={primaryRef} onClick={() => void start()}>
              {state.status === "error"
                ? "Try again"
                : `Download ${formatMegabytes(state.bytesRequired)}`}
            </Button>
          </>
        )}
      </div>
    </Dialog>
  );
}
