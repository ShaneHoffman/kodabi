import { useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
import {
  revealSessionAudio,
  useSessionArtifacts,
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
 * Loading renders nothing (the house quiet loading — the note body is already
 * on screen and this section arriving a beat later is calmer than a spinner).
 */
export function SessionArtifactsSection({ source }: { source: string }) {
  const { artifacts, error } = useSessionArtifacts(source);

  if (error !== null) {
    return (
      <section className="session-artifacts">
        <StatusMessage variant="error">{error}</StatusMessage>
      </section>
    );
  }
  if (artifacts === null) {
    return null;
  }

  return (
    <section className="session-artifacts" aria-label="Session source">
      {artifacts.audio_path !== null && (
        <RecordingRow audioPath={artifacts.audio_path} />
      )}
      <TranscriptSection
        transcriptAvailable={artifacts.transcript_available}
        segments={artifacts.segments}
      />
    </section>
  );
}

/**
 * The retained recording: native playback controls fed through the asset
 * protocol, plus a reveal action. Fully declarative — playback state is
 * browser chrome, so no effect and no bridge hook. The reserved green is
 * "audio is being recorded" and appears nowhere here.
 */
function RecordingRow({ audioPath }: { audioPath: string }) {
  const [revealError, setRevealError] = useState<string | null>(null);

  return (
    <div className="session-artifacts__recording">
      <div className="flex items-center justify-between gap-md">
        <span className="font-mono text-meta text-text-faint">Recording</span>
        <Button
          variant="quiet"
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
      <audio
        controls
        preload="metadata"
        src={convertFileSrc(audioPath)}
        aria-label="Meeting recording"
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
 * The raw transcript, collapsed by default: the summary stays primary and the
 * source is one interaction away. A pruned transcript states its absence
 * instead of hiding the section, so "checked against the source" never fails
 * silently.
 */
function TranscriptSection({
  transcriptAvailable,
  segments,
}: {
  transcriptAvailable: boolean;
  segments: TranscriptSegment[];
}) {
  const [expanded, setExpanded] = useState(false);

  if (!transcriptAvailable) {
    return (
      <p className="session-artifacts__unavailable text-cap text-text-faint">
        The raw transcript for this note is no longer stored.
      </p>
    );
  }

  const count =
    segments.length === 1 ? "1 segment" : `${segments.length} segments`;
  return (
    <div className="session-artifacts__transcript">
      <Button
        variant="quiet"
        aria-expanded={expanded}
        className="font-mono text-meta text-text-soft"
        onClick={() => setExpanded((open) => !open)}
      >
        Transcript
        <span className="text-text-faint"> · {count}</span>
      </Button>
      {expanded && (
        <ol className="session-artifacts__turns">
          {segments.map((segment) => {
            const channel = CHANNEL_LABELS[segment.channel];
            return (
              <li key={segment.index} className="session-artifacts__turn">
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
      )}
    </div>
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
