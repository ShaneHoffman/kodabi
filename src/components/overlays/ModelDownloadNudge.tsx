import { useRef, useState } from "react";
import { formatMegabytes } from "../../models";
import { useModelStatus } from "../../useModelStatus";
import { useNavigation } from "../../useNavigation";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { StatusMessage } from "../ui/StatusMessage";
import { ModelDownloadProgress } from "../models/ModelDownloadProgress";

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
 *
 * The ready beat is the app's one onboarding moment for the vault glossary
 * (docs/ROADMAP.md's Phase 4 "glossary seeding"): the user has just watched
 * transcription become possible and has not yet recorded anything, which is the
 * only instant where seeding the terms it should spell right is still ahead of
 * the first meeting rather than behind it. That is why this beat no longer
 * clears itself after a couple of seconds the way a bare confirmation would —
 * it now poses a choice, and a dialog that vanished as the user reached for it
 * would be worse than one that waits. Close, Escape and the scrim all dismiss.
 */
export function ModelDownloadNudge({ onClose }: Props) {
  const { state, start, cancel } = useModelStatus();
  const { navigate } = useNavigation();
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
        <div className="flex flex-col gap-2.5 text-[13px] leading-relaxed text-ink-read">
          <p>Transcription and search are fully available.</p>
          {/* The one place the vault glossary is introduced. Named terms are
              what transcription gets wrong most often, and the fix only helps
              for captures made after it, so the ask belongs here rather than
              after the first meeting has already misspelled them. */}
          <p>
            Before your first meeting, add the names and jargon it should spell
            right to the vault glossary. Every capture is transcribed against
            it.
          </p>
        </div>
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
            {state.message}
          </StatusMessage>
          {/* The message above now carries what was unaffected and what a retry
              does (`models_cmds`), so this only adds the other way in. */}
          <p className="text-[12px] text-ink-dim">
            You can also start it from Settings.
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
          <>
            <Button variant="quiet" onClick={onClose}>
              Close
            </Button>
            {/* The primary, and so where focus opens: the models are already
                ready, so the only thing left to decide here is whether to seed
                the glossary. The view it lands on carries the rest with its own
                empty state. */}
            <Button
              ref={primaryRef}
              onClick={() => {
                navigate({ kind: "glossary", slug: null });
                onClose();
              }}
            >
              Add glossary terms
            </Button>
          </>
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
