import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
import {
  revealSessionAudio,
  useSessionArtifacts,
  type SessionArtifacts,
  type TranscriptSegment,
} from "../../useSessions";
import "./SessionArtifactsSection.css";

/** The visible speaker label per wire channel, and the value tone that
 * separates the two sides of the exchange — label plus value, never hue
 * (docs/DESIGN.md: the one hue belongs to the recording state). */
const CHANNEL_LABELS: Record<
  TranscriptSegment["channel"],
  { label: string; tone: string }
> = {
  you: { label: "You", tone: "text-text" },
  them: { label: "Them", tone: "text-text-soft" },
  unknown: { label: "Unknown", tone: "text-text-faint" },
};

/**
 * The source pairing under a distilled note: the raw transcript the note was
 * distilled from, and the retained recording when retention kept it. Mounted
 * by `ReadNote` only when the note's `source` names a session artifact
 * (`isSessionSource`), so keyword-sourced notes never fetch.
 *
 * ONE control, behind ONE disclosure. The summary stays primary and the source
 * is one interaction away — the recording and the transcript arrive together
 * because they are used together (a turn's `m:ss` offset exists so the two can
 * be read against each other), and they arrive only when asked for. This used
 * to be two rows with a control each, which put a reader's fourth and fifth
 * places to press below a document that already had two
 * (docs/UI_CONVENTIONS.md, *Composition*). Nesting a second toggle inside this
 * one would be the same failure again, so opening Source opens all of it.
 *
 * Accepted consequence: collapsing unmounts the `<audio>`, so closing the
 * source stops the source. Playing a recording while reading the note below it
 * no longer works. Revisit only if that turns out to be how people listen — the
 * fix is keeping the panel mounted behind `hidden`, at the cost of a live but
 * invisible media element.
 *
 * Loading renders nothing (the house quiet loading — the note body is already
 * on screen and this section arriving a beat later is calmer than a spinner).
 */
export function SessionArtifactsSection({ source }: { source: string }) {
  const { artifacts, error } = useSessionArtifacts(source);
  const [expanded, setExpanded] = useState(false);

  if (error !== null) {
    return (
      <section className="session-artifacts" data-testid="session-artifacts">
        <StatusMessage variant="error">{error}</StatusMessage>
      </section>
    );
  }
  if (artifacts === null) {
    return null;
  }

  // Derived, never tracked: retention can prune both artifacts under an open
  // panel, and an `open` flag that outlived its content would render an empty
  // box. Computed during render, not reconciled in an effect
  // (.claude/rules/no-use-effect.md).
  const summonable =
    artifacts.audio_path !== null || artifacts.transcript_available;
  const open = expanded && summonable;

  return (
    // The testid is on the error branch above too, so it means "this section
    // exists at all" rather than "the fetch succeeded" — which is the claim a
    // keyword-sourced note has to fail.
    <section
      className="session-artifacts"
      aria-label="Session source"
      data-testid="session-artifacts"
    >
      {summonable && (
        <Button
          variant="quiet"
          aria-expanded={open}
          data-testid="session-source"
          className="session-artifacts__toggle font-mono text-meta text-text-soft"
          onClick={() => setExpanded((isOpen) => !isOpen)}
        >
          Source
          <span className="text-text-faint">{sourceDetail(artifacts)}</span>
        </Button>
      )}

      {/* Stays visible at rest, and deliberately so: this sentence's whole job
          is that "checked against the source" never fails silently, which it
          would do until the click if it sat inside the panel. It names only the
          transcript even when the audio is gone too, because `audio_path: null`
          cannot tell "pruned" from "never retained". */}
      {!artifacts.transcript_available && (
        <p
          className="session-artifacts__unavailable text-cap text-text-faint"
          data-testid="session-source-pruned"
        >
          The raw transcript for this note is no longer stored.
        </p>
      )}

      {open && (
        <div className="session-artifacts__panel" data-testid="source-panel">
          {artifacts.audio_path !== null && (
            <Recording audioPath={artifacts.audio_path} />
          )}
          {artifacts.transcript_available && <Turns segments={artifacts.segments} />}
        </div>
      )}
    </section>
  );
}

/** What survived retention, in the toggle's own faint voice: ` · recording · 3
 * segments`. The `·`-joined parts are the house meta shape (`noteMeta`, and the
 * Needs Attention shelf's `Dismissed · N`), and they are the only signal at
 * rest for what opening this will show. Never called with nothing to say — the
 * caller renders no toggle at all then. */
function sourceDetail(artifacts: SessionArtifacts): string {
  const parts: string[] = [];
  if (artifacts.audio_path !== null) {
    parts.push("recording");
  }
  if (artifacts.transcript_available) {
    parts.push(
      artifacts.segments.length === 1
        ? "1 segment"
        : `${artifacts.segments.length} segments`,
    );
  }
  return parts.map((part) => ` · ${part}`).join("");
}

/**
 * The retained recording: native playback controls fed through the asset
 * protocol, plus a reveal action. Fully declarative — playback state is browser
 * chrome, so no effect and no bridge hook. The reserved green is "audio is
 * being recorded" and appears nowhere here.
 *
 * Reveal sits here rather than up in the note's title row because it acts on
 * the `.wav`, not on the note: the slot follows what a control acts on, never
 * where there happened to be room.
 */
function Recording({ audioPath }: { audioPath: string }) {
  const [revealError, setRevealError] = useState<string | null>(null);

  return (
    <div className="session-artifacts__recording">
      <div className="flex items-center justify-between gap-md">
        <span className="font-mono text-meta text-text-faint">Recording</span>
        <Button
          variant="quiet"
          data-testid="reveal-recording"
          className="text-label text-text-soft"
          onClick={() => {
            setRevealError(null);
            revealSessionAudio(audioPath).catch((thrown: unknown) => {
              setRevealError(String(thrown));
            });
          }}
        >
          Reveal in Explorer
        </Button>
      </div>
      {/* The app's only asset-protocol consumer, and the reason the E2E tier can
          gate `media-src` at all — plus the reason the asset scope has to follow
          `KODABI_KB_ROOT` (see the scope widening in `lib.rs` setup). */}
      <audio
        controls
        preload="metadata"
        src={convertFileSrc(audioPath)}
        aria-label="Meeting recording"
        data-testid="recording-player"
        className="session-artifacts__player mt-2xs"
      />
      {revealError !== null && (
        <StatusMessage variant="error" compact className="mt-2xs">
          {revealError}
        </StatusMessage>
      )}
    </div>
  );
}

/**
 * The raw transcript's exchange, uncapped. The disclosure above IS the cap:
 * this only exists once asked for, so it does not out-measure the note the way
 * an always-open block below a summary would (docs/DESIGN_SYSTEM.md §1). A
 * nested scroller would be worse than long — it traps the wheel inside `main`'s
 * own scroller and breaks Ctrl+F across the very text people came here to
 * search.
 */
function Turns({ segments }: { segments: TranscriptSegment[] }) {
  return (
    <ol className="session-artifacts__turns">
      {segments.map((segment) => {
        const channel = CHANNEL_LABELS[segment.channel];
        return (
          // Counting these is a different claim from reading the toggle's
          // count: the toggle reads `segments.length`, this reads what was
          // actually rendered. A future cap or virtualization breaks only the
          // second, which is what the doc comment above promises never happens.
          <li key={segment.index} className="session-artifacts__turn" data-testid="session-turn">
            <span className="ui-tnum font-mono text-meta text-text-faint">
              {formatOffset(segment.start_ms)}
            </span>
            <span className={`font-mono text-meta ${channel.tone}`}>
              {channel.label}
            </span>
            <span className="session-artifacts__turn-text font-serif text-body text-text-read">
              {segment.text}
            </span>
          </li>
        );
      })}
    </ol>
  );
}

/** A segment's offset from session start as `m:ss` (or `h:mm:ss` past an
 * hour) — the shape a player's seek bar speaks, so the two can be read
 * against each other. */
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
