import { useState } from "react";

import { backendCopy } from "../../errorCopy";
import {
  setMeetingTracking,
  trackCommitmentItem,
  useNoteCommitments,
  type NoteCommitmentItem,
} from "../../useNoteCommitments";
import { Button } from "../ui/Button";
import { StatusMessage } from "../ui/StatusMessage";
import { Switch } from "../ui/Switch";
import { PANEL_EYEBROW } from "./SessionPanel";

/** The meta register under a line: who owes it, and whether it is out. */
const ITEM_META = "mt-0.5 font-data text-[10px] text-ink-faint tabular-nums";

/**
 * What this meeting contributes to the ledger, and the two controls that
 * change it.
 *
 * **Extraction is not tracking.** The note above always records what was said;
 * this panel is the separate question of what the ledger follows. It sits in
 * the rail rather than beside the checkboxes in the body for a mechanical
 * reason as much as a compositional one: rendered task rows carry no item
 * identity, and only the lines under `## Action items` are extracted at all, so
 * anything keyed on body order would drift the moment a note has a task list
 * elsewhere. The rail also keeps the header's one-cluster budget intact
 * (docs/UI_CONVENTIONS.md, Composition).
 *
 * Renders nothing at all for a note with no extracted lines and no override,
 * which is most notes: a panel that says "no commitments" on a shopping list is
 * noise, not an empty state.
 */
export function NoteCommitmentsPanel({ noteId }: { noteId: string }) {
  const { contextOnly, items, response, loading, error } =
    useNoteCommitments(noteId);
  const [flipping, setFlipping] = useState(false);
  const [pendingItem, setPendingItem] = useState<string | null>(null);
  const [panelError, setPanelError] = useState<string | null>(null);

  // Adjust-state-during-render, not an effect: a refetch that drops a line has
  // to drop the error aimed at it. Keyed on the response object, never on the
  // derived array, whose identity changes every render.
  const [previousResponse, setPreviousResponse] = useState(response);
  if (previousResponse !== response) {
    setPreviousResponse(response);
    setPanelError(null);
  }

  // Loading renders nothing rather than a skeleton: this panel is subordinate
  // to the note, and a flash of chrome beside a document that has already
  // arrived reads as breakage (docs/DESIGN_SYSTEM.md §3).
  //
  // A failed READ renders nothing either, which is the one place this panel
  // parts company with the four-states rule, and deliberately. The read fails
  // for one reason in practice: the ledger did not open this session. That is
  // structural and identical on every note, so an error card here would repeat
  // a single problem on every note screen the user visits, next to a document
  // that is perfectly intact. The Commitments view owns that message and states
  // it once, in full. A failed WRITE is the opposite case and is always spoken:
  // it is this person's own action, on this line, and it must never look like it
  // worked.
  if (loading || error !== null) return null;
  if (items.length === 0 && !contextOnly) return null;

  const flip = async (next: boolean) => {
    setFlipping(true);
    setPanelError(null);
    try {
      await setMeetingTracking(noteId, next);
      // Nothing is applied here: the write announces itself on `ledger:changed`
      // and the refetch that follows is the truth, the same bargain the
      // Commitments view's rows make.
    } catch (err) {
      setPanelError(
        backendCopy(
          err,
          "Couldn't change this meeting's tracking. Your note is unchanged; try again.",
        ),
      );
    } finally {
      setFlipping(false);
    }
  };

  const track = async (item: NoteCommitmentItem) => {
    setPendingItem(item.item_id);
    setPanelError(null);
    try {
      await trackCommitmentItem(noteId, item.item_id);
    } catch (err) {
      setPanelError(
        backendCopy(
          err,
          "Couldn't track this line. Your note is unchanged; try again.",
        ),
      );
    } finally {
      setPendingItem(null);
    }
  };

  return (
    <section className="glass-card px-4 py-3.5">
      <p className={PANEL_EYEBROW}>Commitments</p>

      <div className="mt-2 flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="text-[12.5px] text-ink">Context only</p>
          {/* A control hint: the one optional line the reader may take or
              leave, which is the register faint is for. */}
          <p className="mt-0.5 text-[11px] leading-snug text-ink-faint">
            Only items assigned to you are tracked from this meeting.
          </p>
        </div>
        <Switch
          label="Context only"
          checked={contextOnly}
          onChange={(next) => void flip(next)}
          busy={flipping}
        />
      </div>

      {items.length > 0 && (
        <ul className="mt-3 border-t border-edge">
          {items.map((item) => (
            <li
              key={item.item_id}
              className="flex items-start justify-between gap-3 border-b border-edge py-2 last:border-b-0"
            >
              <div className="min-w-0">
                <p className="text-[12px] leading-snug text-ink-dim">
                  {item.description}
                </p>
                <p className={ITEM_META}>
                  {item.owner}
                  {item.tracking === "untracked" && " · untracked"}
                  {item.tracking === "not_enrolled" && " · not tracked"}
                </p>
              </div>
              {item.tracking !== "tracked" && (
                <Button
                  variant="quiet"
                  loading={pendingItem === item.item_id}
                  onClick={() => void track(item)}
                >
                  Track
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}

      {panelError !== null && (
        <div className="mt-2">
          <StatusMessage variant="error" compact>
            {panelError}
          </StatusMessage>
        </div>
      )}
    </section>
  );
}
