import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { clsx } from "clsx";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
import {
  revealSessionAudio,
  type SessionArtifacts,
  type TranscriptSegment,
} from "../../useSessions";

/** The visible speaker label per wire channel, and the value tone that
 * separates the two sides of the exchange — label plus value, never hue (the
 * one hue belongs to the recording state, docs/DESIGN_SYSTEM.md §1). */
const CHANNEL_LABELS: Record<
  TranscriptSegment["channel"],
  { label: string; tone: string }
> = {
  you: { label: "You", tone: "text-ink" },
  them: { label: "Them", tone: "text-ink-dim" },
  unknown: { label: "Unknown", tone: "text-ink-faint" },
};

/** The details rail's eyebrow. Data, not a heading: `font-data`, small, tracked
 * wide, and never at ink strength (docs/DESIGN_SYSTEM.md §1). */
export const PANEL_EYEBROW =
  "font-data text-[10px] uppercase tracking-[0.22em] text-ink-faint";

/** What the Transcript chip points at.
 *
 * The chip is the one disclosure in the app whose content is neither its
 * sibling nor below it: the turns need the reading measure, so they land in the
 * body column, which is the grid's FIRST child and therefore upstream of the
 * rail holding the chip. `aria-expanded` alone would leave a screen reader
 * hearing "expanded" with nothing after the button and nothing to jump to. One
 * id, spelled once, is what makes the pair programmatic rather than visual. */
const TRANSCRIPT_TURNS_ID = "note-transcript-turns";

/**
 * The source pairing, as a panel in the note's details rail: the recording
 * retention kept, and the raw transcript the note was distilled from. Rendered
 * only when the note's `source` names a session artifact (`isSessionSource`),
 * so keyword-sourced notes never fetch and never show an empty panel.
 *
 * Two chips, each standing for one artifact you can open. They used to be one
 * disclosure that opened both, because both used to land in the reading column
 * and two controls there would have been a reader's fourth and fifth places to
 * press below a document that already had two. The rail changes that
 * arithmetic: these sit beside the note rather than under it, each opens one
 * thing, and neither nests inside the other (docs/UI_CONVENTIONS.md,
 * *Composition*).
 *
 * The recording opens in place — 272px holds a player. The transcript does not:
 * turns need the reading measure, so the chip toggles them in the body column
 * and this component only reports the state (`transcriptOpen`), which the view
 * owns.
 *
 * The `<audio>` stays mounted whether or not the chip is open. That is what
 * puts a real duration on the chip at rest, and it retires the old accepted
 * consequence that closing the source stopped playback — you can now listen
 * while reading the note. The cost is one ranged metadata read per opened
 * session note, which `preload="metadata"` bounds to the file's header.
 *
 * Loading renders nothing (the house quiet loading — the note body is already
 * on screen and this panel arriving a beat later is calmer than a spinner).
 */
export function SessionPanel({
  artifacts,
  error,
  transcriptOpen,
  onToggleTranscript,
}: {
  artifacts: SessionArtifacts | null;
  error: string | null;
  transcriptOpen: boolean;
  onToggleTranscript: () => void;
}) {
  const [audioOpen, setAudioOpen] = useState(false);
  const [duration, setDuration] = useState<number | null>(null);
  const [revealError, setRevealError] = useState<string | null>(null);

  if (error !== null) {
    // The testid is on this branch too, so it means "this panel exists at all"
    // rather than "the fetch succeeded" — which is the claim a keyword-sourced
    // note has to fail.
    return (
      <section className="glass-card px-4 py-3.5" data-testid="session-artifacts">
        <StatusMessage variant="error" compact>
          {error}
        </StatusMessage>
      </section>
    );
  }
  if (artifacts === null) {
    return null;
  }

  // Derived, never tracked: retention can prune an artifact under an open chip,
  // and an `open` flag that outlived its content would render an empty box.
  // Computed during render, not reconciled in an effect
  // (.claude/rules/no-use-effect.md).
  const audioPath = artifacts.audio_path;
  const playing = audioOpen && audioPath !== null;

  return (
    <section
      className="glass-card px-4 py-3.5"
      aria-label="Session source"
      data-testid="session-artifacts"
    >
      <p className={PANEL_EYEBROW}>Session</p>

      {audioPath !== null && (
        <>
          <Button
            variant="chip"
            className="mt-2.5 w-full"
            aria-expanded={playing}
            data-testid="session-audio"
            onClick={() => {
              setAudioOpen((open) => !open);
              // A reveal that failed is about the press, not about the chip's
              // state, so it does not outlive the chip closing. Without this it
              // does: the error lives up here now that the panel stays mounted,
              // where it used to die with the unmounted player — and a
              // `role="alert"` restored on reopening announces a failure the
              // reader did not just cause.
              setRevealError(null);
            }}
          >
            {/* The split lives INSIDE the button: `justify-between` in
                className cannot beat the variant's own `justify-center`.
                The `{" "}` is load-bearing and invisible: a flex container does
                not render a whitespace-only child, but textContent and the
                accessible name both join these two spans verbatim — without it
                the button announces itself as "Audio1:06". */}
            <span className="flex w-full items-center justify-between gap-2">
              <span>
                <span aria-hidden="true">▶ </span>Audio
              </span>{" "}
              <span className="font-data text-[10.5px] font-normal tabular-nums text-ink-faint">
                {audioLength(duration, artifacts.segments)}
              </span>
            </span>
          </Button>
          {/* The app's only asset-protocol consumer, and the reason the E2E tier
              can gate `media-src` at all — plus the reason the asset scope has
              to follow `KODABI_KB_ROOT` (see the scope widening in `lib.rs`
              setup). Hidden rather than unmounted: see the note above. */}
          <audio
            controls
            preload="metadata"
            src={convertFileSrc(audioPath)}
            aria-label="Meeting recording"
            data-testid="recording-player"
            hidden={!playing}
            onLoadedMetadata={(event) => {
              const seconds = event.currentTarget.duration;
              // A stream with no known length reports Infinity, and a file the
              // player could not read reports NaN. Either way the segment
              // fallback is the better answer.
              if (Number.isFinite(seconds)) setDuration(seconds);
            }}
            className="mt-2 w-full"
          />
          {playing && (
            <>
              <Button
                variant="quiet"
                className="mt-1 w-full"
                data-testid="reveal-recording"
                onClick={() => {
                  setRevealError(null);
                  revealSessionAudio(audioPath).catch((thrown: unknown) => {
                    setRevealError(String(thrown));
                  });
                }}
              >
                Reveal in Explorer
              </Button>
              {revealError !== null && (
                <StatusMessage variant="error" compact className="mt-2">
                  {revealError}
                </StatusMessage>
              )}
            </>
          )}
        </>
      )}

      {artifacts.transcript_available ? (
        <Button
          variant="chip"
          className="mt-2 w-full"
          aria-expanded={transcriptOpen}
          aria-controls={TRANSCRIPT_TURNS_ID}
          data-testid="session-source"
          onClick={onToggleTranscript}
        >
          <span className="flex w-full items-center justify-between gap-2">
            <span>Transcript</span>{" "}
            <span className="font-data text-[10.5px] font-normal tabular-nums text-ink-faint">
              {wordCount(artifacts.segments)}
            </span>
          </span>
        </Button>
      ) : (
        // Stays visible at rest, and deliberately so: this sentence's whole job
        // is that "checked against the source" never fails silently, which it
        // would do until a click if it sat behind a chip. It names only the
        // transcript even when the audio is gone too, because `audio_path: null`
        // cannot tell "pruned" from "never retained".
        <p
          className="mt-2.5 text-[12px] text-ink-faint"
          data-testid="session-source-pruned"
        >
          The raw transcript for this note is no longer stored.
        </p>
      )}
    </section>
  );
}

/** The recording's length for the chip: the player's own duration once it has
 * read the file header, and until then the last turn's end offset — which is
 * the recording's length to within a segment, and arrives with the transcript
 * rather than after a round trip. Neither exists for an audio-only note whose
 * transcript was pruned, and the chip simply carries no value then. */
function audioLength(
  duration: number | null,
  segments: TranscriptSegment[],
): string {
  if (duration !== null) return formatOffset(duration * 1000);
  const last = segments[segments.length - 1];
  return last ? formatOffset(last.end_ms) : "";
}

/** How much transcript is behind the chip, in the unit a reader thinks in.
 * Segments were the old count, and they are an artifact of how the recognizer
 * chunked the audio — "23 segments" says nothing about how long this will take
 * to read. */
function wordCount(segments: TranscriptSegment[]): string {
  const words = segments.reduce(
    (total, segment) => total + segment.text.split(/\s+/).filter(Boolean).length,
    0,
  );
  return words === 1 ? "1 word" : `${words.toLocaleString()} words`;
}

/**
 * The raw transcript's exchange, uncapped, in the note's own reading column —
 * the rail is 272px wide and a turn is a sentence. The chip above IS the cap:
 * this only exists once asked for, so it does not out-measure the note the way
 * an always-open block below a summary would (docs/DESIGN_SYSTEM.md §1). A
 * nested scroller would be worse than long — it traps the wheel inside `main`'s
 * own scroller and breaks Ctrl+F across the very text people came here to
 * search.
 */
export function TranscriptTurns({ segments }: { segments: TranscriptSegment[] }) {
  return (
    <div
      id={TRANSCRIPT_TURNS_ID}
      className="mt-8 border-t border-edge pt-6"
      data-testid="source-panel"
    >
      <p className={PANEL_EYEBROW}>Transcript</p>
      <ol className="mt-3">
        {segments.map((segment) => {
          const channel = CHANNEL_LABELS[segment.channel];
          return (
            // Counting these is a different claim from reading the chip's
            // count: the chip counts words in the data, this counts what was
            // actually rendered. A future cap or virtualization breaks only the
            // second, which is what the doc comment above promises never
            // happens.
            <li
              key={segment.index}
              data-testid="session-turn"
              className="mt-2 grid items-baseline gap-3 [grid-template-columns:56px_72px_minmax(0,1fr)]"
            >
              <span className="font-data text-[11px] tabular-nums text-ink-faint">
                {formatOffset(segment.start_ms)}
              </span>
              <span className={clsx("font-data text-[11px]", channel.tone)}>
                {channel.label}
              </span>
              <span className="font-note text-note text-ink-read">{segment.text}</span>
            </li>
          );
        })}
      </ol>
    </div>
  );
}

/** An offset from session start as `m:ss` (or `h:mm:ss` past an hour) — the
 * shape a player's seek bar speaks, so the two can be read against each
 * other. */
function formatOffset(startMs: number): string {
  const totalSeconds = Math.floor(startMs / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = String(totalSeconds % 60).padStart(2, "0");
  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, "0")}:${seconds}`;
  }
  return `${minutes}:${seconds}`;
}
