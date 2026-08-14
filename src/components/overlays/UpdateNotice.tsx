import { formatMegabytes } from "../../models";
import { useUpdaterStatus } from "../../useUpdaterStatus";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";

type Props = {
  onClose: () => void;
};

/**
 * A new release is waiting, said quietly in the corner.
 *
 * Not a dialog, unlike the first-run model ask. That one gates something the
 * app cannot do without; this one gates nothing at all — the app the user
 * already has works, and an update they have not asked for has no business
 * taking the screen. So it borrows `CaptureToast`'s corner and its
 * no-entrance-animation rule (showing the surface IS the transition) rather
 * than the nudge's modal.
 *
 * Every step is a click: check, download, restart. Dismissal is session-only,
 * the same bargain the model nudge makes — the check runs again next launch,
 * so a user who keeps waving it away is never locked out of the update, and
 * the permanent path is the Settings About card.
 *
 * `role="status"` rather than `alert`: a waiting update is news, not a
 * problem, and it should wait its turn behind whatever the user is doing. The
 * failure branches below are the exception and say so.
 */
export function UpdateNotice({ onClose }: Props) {
  const { state, download, install } = useUpdaterStatus();
  const { phase } = state;

  // Nothing to say: no check has run, one is running, this build is current,
  // or the startup check failed. That last one is deliberate — a check the
  // user never asked for must not report its own failure into their corner.
  if (
    phase.status === "idle" ||
    phase.status === "checking" ||
    phase.status === "upToDate" ||
    (phase.status === "error" && phase.step === "check")
  ) {
    return null;
  }

  return (
    <div
      className="glass-overlay flex max-w-[280px] flex-col gap-3 px-4 py-3"
      role={phase.status === "error" ? "alert" : "status"}
      data-testid="update-notice"
    >
      {phase.status === "available" && (
        <>
          <p className="font-ui text-[13.5px] leading-snug text-ink">
            Kodabi {phase.version} is available.
          </p>
          <div className="flex items-center justify-end gap-2.5">
            <Button variant="quiet" onClick={onClose}>
              Not now
            </Button>
            <Button onClick={() => void download()}>Download</Button>
          </div>
        </>
      )}

      {phase.status === "downloading" && (
        <>
          <p className="font-ui text-[13.5px] leading-snug text-ink">
            Downloading Kodabi {phase.version}.
          </p>
          {/* The byte line, not a bar: the denominator is unknown until the
              server volunteers a content length, and a bar with a made-up end
              is a worse lie than a number that is simply counting up. */}
          <p className="font-data text-[11px] text-ink-dim">
            {phase.progress.totalBytes === null
              ? formatMegabytes(phase.progress.receivedBytes)
              : `${formatMegabytes(phase.progress.receivedBytes)} of ${formatMegabytes(phase.progress.totalBytes)}`}
          </p>
        </>
      )}

      {phase.status === "readyToInstall" && (
        <>
          <p className="font-ui text-[13.5px] leading-snug text-ink">
            Kodabi {phase.version} is ready. Installing restarts the app.
          </p>
          <div className="flex items-center justify-end gap-2.5">
            <Button variant="quiet" onClick={onClose}>
              Later
            </Button>
            <Button onClick={() => void install()}>Restart and update</Button>
          </div>
        </>
      )}

      {phase.status === "installing" && (
        <p className="font-ui text-[13.5px] leading-snug text-ink">
          Installing Kodabi {phase.version}. The app will restart.
        </p>
      )}

      {phase.status === "error" && phase.step !== "check" && (
        <>
          <StatusMessage variant="error" compact>
            {phase.step === "download"
              ? `Couldn't download the update: ${phase.message}`
              : `Couldn't install the update: ${phase.message}`}
          </StatusMessage>
          {/* Says what was NOT harmed, because a failed self-update is exactly
              the kind of thing a user assumes has broken their install. */}
          <p className="text-[12px] text-ink-dim">
            Your current version is untouched and your notes are safe.
          </p>
          <div className="flex items-center justify-end gap-2.5">
            <Button variant="quiet" onClick={onClose}>
              Dismiss
            </Button>
            <Button onClick={() => void download()}>Try again</Button>
          </div>
        </>
      )}
    </div>
  );
}
