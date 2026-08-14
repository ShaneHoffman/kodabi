import { formatMegabytes, type DownloadProgress } from "../../models";

/**
 * How far the model download has got, as a continuous ink bar plus the numbers
 * in words beside it.
 *
 * Continuous rather than segmented, which is the opposite call from the Inbox
 * meter — and for the same two reasons that one gives. That quantity is
 * discrete (notes cleared) and the work is the user's, so a creeping bar would
 * have read as the app labouring over something. This quantity is genuinely
 * continuous (bytes off a wire) and the work genuinely is the app's, so the
 * loading-indicator reading is the correct one. It is the first determinate
 * percentage in the app.
 *
 * The fill is INK, never green: progress is information, and green here is the
 * kodama's voice (docs/DESIGN_SYSTEM.md §2).
 *
 * Deliberately not a `src/components/ui/` primitive. Two call sites in one
 * feature do not earn a catalogue row, a gallery entry and coverage under four
 * grounds (docs/UI_CONVENTIONS.md §4). A third, unrelated consumer would.
 *
 * `aria-hidden` on the bar with the figures stated in adjacent text, per the
 * Inbox meter's reasoning: the numbers are already readable, and a
 * `role="progressbar"` would announce the same fact a second time as a
 * percentage nobody asked for. The live region belongs to the caller and
 * carries phase words only — byte counts arriving five times a second would
 * make it unusable.
 */
export function ModelDownloadProgress({ progress }: { progress: DownloadProgress | null }) {
  // Before the first byte report there is nothing true to draw: the backend
  // has been asked but has not answered, and a bar at zero with no numbers
  // beside it says less than a sentence does.
  if (!progress || progress.total === 0) {
    return (
      <p className="text-[12px] text-ink-faint" data-testid="model-progress-starting">
        Starting download
      </p>
    );
  }

  const fraction = Math.min(1, progress.received / progress.total);
  return (
    <div className="flex flex-col gap-1.5" data-testid="model-progress">
      <div className="flex items-baseline justify-between gap-4">
        <span className="text-[12px] text-ink-read">
          {progress.verifying ? "Checking the download" : "Downloading models"}
        </span>
        <span className="font-data text-[12px] tabular-nums text-ink-faint">
          {formatMegabytes(progress.received)} of {formatMegabytes(progress.total)}
        </span>
      </div>
      <div className="h-1 w-full overflow-hidden rounded-[2px] bg-wash" aria-hidden="true">
        <div
          className="h-full rounded-[2px] bg-ink/50 transition-[width] duration-200 ease-out-strong motion-reduce:transition-none"
          style={{ width: `${fraction * 100}%` }}
          data-testid="model-progress-fill"
        />
      </div>
      {progress.fileCount > 0 && (
        <span className="font-data text-[10.5px] text-ink-faint">
          {progress.file} · file {progress.fileIndex + 1} of {progress.fileCount}
        </span>
      )}
    </div>
  );
}
