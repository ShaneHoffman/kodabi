import { Fragment, useState, type CSSProperties, type ReactNode } from "react";

import {
  arrangeCommitments,
  arrangeTriage,
  commitmentOwner,
  commitmentText,
  formatConfidence,
  formatDay,
  formatInstant,
  needsReview,
  nextWeekIso,
  settledBy,
  tomorrowIso,
  triageWatermark,
  workloadSummary,
  type CommitmentGroup,
  type TriageGroup,
} from "../../commitmentGroups";
import { backendCopy } from "../../errorCopy";
import { useNavigation, type View } from "../../useNavigation";
import { categoryLabel } from "../../useNotes";
import {
  confirmCommitmentEvidence,
  dismissCommitmentEvidence,
  markCommitmentsSeen,
  reopenCommitment,
  setCommitmentDone,
  snoozeCommitment,
  snoozeCommitments,
  untrackCommitment,
  untrackCommitments,
  claimCommitmentMine,
  useCommitments,
  waiveCommitment,
  type BulkMutateResult,
  type Commitment,
} from "../../useCommitments";
import { SpiritMark } from "../capture/SpiritMark";
import { Button } from "../ui/Button";
import { Checkbox } from "../ui/Checkbox";
import { Dialog } from "../ui/Dialog";
import { Field } from "../ui/Field";
import { Menu } from "../ui/Menu";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";

type Props = {
  /** Which ledger to show. `null` is the whole vault. */
  slug: string | null;
};

/**
 * A row you owe. THE MINE ROWS ARE CARDS AND THEY LIFT, because every one of
 * them is a thing you are meant to clear — the Inbox's stance, for the same
 * reason (docs/DESIGN_SYSTEM.md §1).
 *
 * This is where the me/them split is actually spelled. It is not a filter chip
 * and not a colour: the two halves sit on different PLANES, so which half you
 * are looking at is legible from across the room before a single word is read.
 */
const MINE_CARD = [
  "glass-card flex items-start gap-5 rounded-card px-5 py-4",
  "hover:-translate-y-[2px] hover:glass-card-lift motion-reduce:hover:translate-y-0",
  "transition-[translate,box-shadow,border-color] duration-180 ease-out-strong",
  "motion-reduce:transition-none",
].join(" ");

/**
 * A row someone else owes, and the quiet half of the split. Flat, hairline
 * separated, no plane and no lift: nothing here is waiting on YOU, so it reads
 * as a register you keep rather than a queue you work. Same anatomy as the card
 * above, so the eye tracks one column of checkboxes down the whole page.
 */
const WATCH_ROW = "flex items-start gap-5 border-t border-edge px-5 py-4 first:border-t-0";

/** The eyebrow over each run of rows. The view's one level of eyebrow, so the
 * frame deliberately carries none (the ProjectView rule). */
const GROUP_LABEL = [
  "font-data text-[10px] uppercase tracking-[0.22em] text-ink-faint",
  "mt-6.5 mb-1.5 px-2.5 first:mt-4.5",
].join(" ");

/** The meta register: owner, dates, provenance. Never a sentence to read. */
const META_LINE = "mt-1.5 flex flex-wrap items-center gap-2.5 font-data text-[10.5px] text-ink-faint tabular-nums";

/** The entrance, and its reduced partner. */
const RISES_IN = "animate-rise-in motion-reduce:animate-fade-in";
const STAGGER_STEP_MS = 45;
const STAGGER_CAP = 5;

function staggerStyle(position: number): CSSProperties | undefined {
  return position === 0
    ? undefined
    : {
        animationDelay: `${Math.min(position, STAGGER_CAP - 1) * STAGGER_STEP_MS}ms`,
      };
}

/** One in-flight write, named so the row that owns it can go busy. */
type Pending = { entryId: string; verb: string };

/** The commitments enrolled since the marker, frozen at this mount's first
 * read. `lastSeen` is kept beside them so the arrangement can re-apply the cut
 * it was built from rather than a marker the review has since moved. */
type TriageBatch = {
  lastSeen: string | null;
  rows: { entry_id: string; created_at: string }[];
};

/** Shared empty set, so an untouched selection keeps one identity. */
const EMPTY_IDS: ReadonlySet<string> = new Set<string>();

/** `ids` minus `remove`, or the same set when nothing was in it. */
function withoutIds(
  ids: ReadonlySet<string>,
  remove: readonly string[],
): ReadonlySet<string> {
  if (!remove.some((id) => ids.has(id))) return ids;
  const next = new Set(ids);
  for (const id of remove) next.delete(id);
  return next;
}

/** `ids` narrowed to those still present, or the same set when all are. */
function pruneIds(
  ids: ReadonlySet<string>,
  live: ReadonlySet<string>,
): ReadonlySet<string> {
  const kept = [...ids].filter((id) => live.has(id));
  return kept.length === ids.size ? ids : new Set(kept);
}

/**
 * The commitment ledger, as a person reads it: what you owe, what you are
 * waiting on, and what just settled.
 *
 * The organizing principle is the me/them split rather than a filter, because
 * the two halves are different kinds of work. "Mine" is a queue you clear.
 * "Waiting on them" is the half a checkbox list normally loses entirely, and it
 * is not a to-do list at all — it is a register, which is why it looks like one.
 *
 * The checkbox writes the note, not the ledger: the Markdown owns done/not-done
 * and always has (`kodabi_core::ledger`). What the ledger records is the
 * judgement beside it, which is why snooze and waive live here and never touch
 * a note. Waiving exists precisely so nobody has to edit a meeting note to
 * pretend something was not said.
 */
export function CommitmentsView({ slug }: Props) {
  const { navigate } = useNavigation();
  const { entries, settled, response, loading, error } = useCommitments(slug);
  const [pending, setPending] = useState<Pending | null>(null);
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});
  const [showSnoozed, setShowSnoozed] = useState(false);
  const [showSettled, setShowSettled] = useState(false);
  const [snoozeTarget, setSnoozeTarget] = useState<Commitment | null>(null);
  // The review batch, frozen at the first read of this mount. It is held in
  // state rather than derived because every `ledger:changed` refetch would
  // otherwise recompute it against a marker the review itself has advanced,
  // and rows would vanish from under the hand clearing them. What the strip
  // renders is this batch intersected with what is still live, minus what has
  // been reviewed, so a row settled elsewhere still drops out.
  const [batch, setBatch] = useState<TriageBatch | null>(null);
  const [reviewedIds, setReviewedIds] = useState<ReadonlySet<string>>(EMPTY_IDS);
  const [selectedIds, setSelectedIds] = useState<ReadonlySet<string>>(EMPTY_IDS);
  const [bulkPending, setBulkPending] = useState<string | null>(null);
  const [bulkError, setBulkError] = useState<string | null>(null);
  const [bulkSkipped, setBulkSkipped] = useState<number>(0);
  const [bulkSnoozeOpen, setBulkSnoozeOpen] = useState(false);
  const [markedThrough, setMarkedThrough] = useState<string | null>(null);

  // Adjust-state-during-render, not an effect: when a refetch drops a row, its
  // error and any dialog aimed at it have to go with it. Keyed on the response
  // object the query holds, never on a derived array — a derived array is a new
  // identity every render, so the reset would fire forever.
  const [previousResponse, setPreviousResponse] = useState(response);
  if (previousResponse !== response) {
    setPreviousResponse(response);
    const live = new Set(
      [...entries, ...settled].map((commitment) => commitment.entry_id),
    );
    setRowErrors((errors) => {
      const kept = Object.entries(errors).filter(([id]) => live.has(id));
      return kept.length === Object.keys(errors).length
        ? errors
        : Object.fromEntries(kept);
    });
    if (snoozeTarget && !live.has(snoozeTarget.entry_id)) setSnoozeTarget(null);
    // Seeded from the first read only, and never re-seeded: a second read is a
    // consequence of this session's own writes.
    if (batch === null && response) {
      const lastSeen = response.last_seen;
      setBatch({
        lastSeen,
        rows: lastSeen
          ? entries
              .filter((commitment) => commitment.created_at > lastSeen)
              .map(({ entry_id, created_at }) => ({ entry_id, created_at }))
          : [],
      });
      setMarkedThrough(lastSeen);
    }
    // A selection may not outlive the rows it named.
    setSelectedIds((ids) => pruneIds(ids, live));
  }

  const { groups, snoozed, settled: settledRows } = arrangeCommitments(
    entries,
    settled,
  );
  const isEmpty =
    groups.length === 0 && snoozed.length === 0 && settledRows.length === 0;

  // The strip is the whole-vault view's alone. The marker is one instant for
  // the whole ledger, so reviewing inside a project would silently mark other
  // projects' commitments seen as well.
  const batchIds = new Set(batch?.rows.map((row) => row.entry_id) ?? []);
  const triageGroups =
    slug === null
      ? arrangeTriage(
          entries.filter(
            (commitment) =>
              batchIds.has(commitment.entry_id) &&
              !reviewedIds.has(commitment.entry_id),
          ),
          batch?.lastSeen ?? null,
        )
      : [];
  const triageCount = triageGroups.reduce(
    (total, group) => total + group.rows.length,
    0,
  );

  /** Moves the last-seen marker as far as the reviewed rows allow. */
  const advanceMarker = async (reviewed: ReadonlySet<string>) => {
    if (!batch) return;
    const watermark = triageWatermark(batch.rows, reviewed);
    // Only ever forward, and only when it actually moved: the backend keeps the
    // later of the two anyway, so a redundant call would be a wasted round trip
    // rather than a bug.
    if (!watermark || (markedThrough && watermark <= markedThrough)) return;
    setMarkedThrough(watermark);
    try {
      await markCommitmentsSeen(watermark);
    } catch (err) {
      // The review still happened on screen; only the memory of it failed, and
      // the copy says exactly that rather than pretending the rows are back.
      setBulkError(
        backendCopy(
          err,
          "Couldn't save your place in this list. These may show up again next time.",
        ),
      );
    }
  };

  /** Marks rows reviewed locally and advances the marker behind them. */
  const markReviewed = async (ids: string[]) => {
    const next = new Set([...reviewedIds, ...ids]);
    setReviewedIds(next);
    setSelectedIds((selected) => withoutIds(selected, ids));
    await advanceMarker(next);
  };

  /** Runs one batched gesture, reporting what the ledger declined. */
  const runBulk = async (
    verb: string,
    ids: string[],
    fallback: string,
    write: () => Promise<BulkMutateResult>,
  ) => {
    if (ids.length === 0) return;
    setBulkPending(verb);
    setBulkError(null);
    setBulkSkipped(0);
    try {
      const result = await write();
      setBulkSkipped(result.skipped);
      await markReviewed(ids);
    } catch (err) {
      setBulkError(backendCopy(err, fallback));
    } finally {
      setBulkPending(null);
    }
  };

  const keepRows = (rows: Commitment[]) => {
    // Keep changes no ledger state at all: the commitment is already tracked,
    // and the only thing being recorded is that a person looked at it.
    setBulkError(null);
    setBulkSkipped(0);
    void markReviewed(rows.map((row) => row.entry_id));
  };

  const untrackRows = (rows: Commitment[]) =>
    void runBulk(
      "untrack",
      rows.map((row) => row.entry_id),
      "Couldn't untrack these commitments. The ledger is unchanged; try again.",
      () => untrackCommitments(rows.map((row) => row.entry_id)),
    );

  const selectedRows = triageGroups
    .flatMap((group) => group.rows)
    .filter((row) => selectedIds.has(row.entry_id));

  const snoozeSelected = (until: string) =>
    runBulk(
      "snooze",
      selectedRows.map((row) => row.entry_id),
      "Couldn't snooze these commitments. The ledger is unchanged; try again.",
      () =>
        snoozeCommitments(
          selectedRows.map((row) => row.entry_id),
          until,
        ),
    );

  const toggleSelected = (ids: string[], selected: boolean) =>
    setSelectedIds((current) =>
      selected ? new Set([...current, ...ids]) : withoutIds(current, ids),
    );

  /** Runs one write, keeping the row's own control busy while it is in flight. */
  const run = async (
    commitment: Commitment,
    verb: string,
    fallback: string,
    write: () => Promise<unknown>,
  ) => {
    setPending({ entryId: commitment.entry_id, verb });
    setRowErrors((errors) => {
      if (!(commitment.entry_id in errors)) return errors;
      return Object.fromEntries(
        Object.entries(errors).filter(([id]) => id !== commitment.entry_id),
      );
    });
    try {
      await write();
      // Nothing is applied here on purpose. Both writes announce themselves
      // (`vault:changed` for the note, `ledger:changed` for the judgement), and
      // the refetch that follows is the truth. An optimistic flip would have to
      // be reconciled against it a moment later for no gain.
    } catch (err) {
      setRowErrors((errors) => ({
        ...errors,
        [commitment.entry_id]: backendCopy(err, fallback),
      }));
    } finally {
      setPending(null);
    }
  };

  const toggleDone = (commitment: Commitment, done: boolean) => {
    const item = commitment.item;
    if (!item) return;
    void run(
      commitment,
      "done",
      "Couldn't update this commitment. Its note is unchanged; try again.",
      () =>
        setCommitmentDone({
          entry_id: commitment.entry_id,
          note_id: item.note_id,
          item_id: item.item_id,
          done,
        }),
    );
  };

  const snooze = (commitment: Commitment, until: string) =>
    run(
      commitment,
      "snooze",
      "Couldn't snooze this commitment. The ledger is unchanged; try again.",
      () => snoozeCommitment(commitment.entry_id, until),
    );

  const waive = (commitment: Commitment) =>
    void run(
      commitment,
      "waive",
      "Couldn't waive this commitment. The ledger is unchanged; try again.",
      () => waiveCommitment(commitment.entry_id),
    );

  const untrack = (commitment: Commitment) =>
    void run(
      commitment,
      "untrack",
      "Couldn't untrack this commitment. The ledger is unchanged; try again.",
      () => untrackCommitment(commitment.entry_id),
    );

  /**
   * The correction behind every misfiled row: this one is mine.
   *
   * Moves it to Mine and teaches the name it was filed under, so the next
   * meeting gets it right unprompted. The move is what the user asked for and
   * always lands; the name is the part that can quietly fail, and a failure
   * there is worth saying, because the same misfiling will happen again.
   */
  const claimMine = (commitment: Commitment) =>
    void run(
      commitment,
      "claim",
      "Couldn't move this to Mine. The ledger is unchanged; try again.",
      async () => {
        const result = await claimCommitmentMine(commitment.entry_id);
        // Only a real failure earns a line. A name the backend declined to
        // learn (a reserved token, or one it already knew) is the design
        // working, and apologising for it would send the reader to Settings to
        // add something that must never be added.
        if (result.alias === "failed") {
          setRowErrors((errors) => ({
            ...errors,
            [commitment.entry_id]: `Moved to Mine, but "${commitmentOwner(
              commitment,
            )}" wasn't saved as one of your names, so future mentions may still file under Waiting on them. Add it in Settings.`,
          }));
        }
      },
    );

  /**
   * The undo, behind every settled row and every snooze.
   *
   * A closed row whose line is still in its note unticks the box as well: the
   * Markdown owns done, so reopening the ledger alone would leave a ticked box
   * over an open commitment. The closure annotation stays either way. Annotate,
   * never destroy.
   */
  const reopen = (commitment: Commitment) => {
    const item = commitment.item;
    const untick = item && commitment.state === "closed" && item.done;
    void run(
      commitment,
      "reopen",
      "Couldn't reopen this commitment. The ledger is unchanged; try again.",
      () =>
        untick
          ? setCommitmentDone({
              entry_id: commitment.entry_id,
              note_id: item.note_id,
              item_id: item.item_id,
              done: false,
            })
          : reopenCommitment(commitment.entry_id),
    );
  };

  const openSource = (commitment: Commitment) => {
    const source = commitment.source;
    if (!source) return;
    const origin: View = { kind: "commitments", slug };
    navigate({
      kind: "noteEditor",
      noteId: source.note_id,
      project: source.project,
      origin,
    });
  };

  const rowProps = (commitment: Commitment) => ({
    commitment,
    pending,
    error: rowErrors[commitment.entry_id],
    showProject: slug === null,
    onToggle: toggleDone,
    onOpen: openSource,
    onReopen: reopen,
    onSnooze: snooze,
    onSnoozePick: setSnoozeTarget,
    onWaive: waive,
    onUntrack: untrack,
    onClaimMine: claimMine,
    onConfirm: (target: Commitment, evidenceId: string) =>
      void run(
        target,
        "confirm",
        "Couldn't record your answer. The commitment is unchanged; try again.",
        () => confirmCommitmentEvidence(target.entry_id, evidenceId),
      ),
    onDismiss: (target: Commitment, evidenceId: string) =>
      void run(
        target,
        "dismiss",
        "Couldn't record your answer. The commitment is unchanged; try again.",
        () => dismissCommitmentEvidence(target.entry_id, evidenceId),
      ),
  });

  return (
    <ViewFrame
      variant="queue"
      title="Commitments"
      summary={workloadSummary(groups, slug)}
      // The frame's one action slot is deliberately empty until the GitHub
      // evidence pass lands and gives it "Check now" to hold. A disabled
      // button naming a feature that does not exist teaches nothing.
    >
      {error ? (
        <StatusMessage variant="error">{error}</StatusMessage>
      ) : (
        <>
          {triageCount > 0 && (
            <TriageStrip
              groups={triageGroups}
              count={triageCount}
              selectedIds={selectedIds}
              pending={bulkPending}
              error={bulkError}
              skipped={bulkSkipped}
              onToggle={toggleSelected}
              onKeep={keepRows}
              onUntrack={untrackRows}
              onSnooze={() => setBulkSnoozeOpen(true)}
            />
          )}
          {isEmpty
            ? // Gated on `!loading`: without it a cold start tells the user
              // their ledger is empty before the first read has landed.
              !loading && <EmptyLedger scoped={slug !== null} />
            : groups.length === 0
              ? !loading && (
                  <StatusMessage variant="empty">
                    Nothing open right now. Snoozed and settled commitments are
                    on the shelves below.
                  </StatusMessage>
                )
              : groups.map((group, groupIndex) => (
                  <Fragment key={group.direction}>
                    <p
                      className={`${GROUP_LABEL} ${RISES_IN}`}
                      style={staggerStyle(staggerPosition(groups, groupIndex, -1))}
                    >
                      {group.label}
                      <span> · {group.rows.length}</span>
                    </p>
                    <ul
                      className={
                        group.direction === "mine"
                          ? "flex flex-col gap-3.5"
                          : "flex flex-col"
                      }
                      data-testid={`commitments-${group.direction}`}
                    >
                      {group.rows.map((commitment, rowIndex) => (
                        <li
                          key={commitment.entry_id}
                          className={`${
                            group.direction === "mine" ? MINE_CARD : WATCH_ROW
                          } ${RISES_IN}`}
                          style={staggerStyle(
                            staggerPosition(groups, groupIndex, rowIndex),
                          )}
                        >
                          <CommitmentRow {...rowProps(commitment)} />
                        </li>
                      ))}
                    </ul>
                  </Fragment>
                ))}

          {snoozed.length > 0 && (
            <Shelf
              label="Snoozed"
              count={snoozed.length}
              open={showSnoozed}
              onToggle={() => setShowSnoozed((open) => !open)}
              testId="snoozed-commitments"
            >
              {snoozed.map((commitment) => (
                <li key={commitment.entry_id} className={WATCH_ROW}>
                  <ShelfRow
                    commitment={commitment}
                    meta={
                      commitment.snoozed_until
                        ? `snoozed until ${formatDay(commitment.snoozed_until)}`
                        : "snoozed"
                    }
                    actionLabel="Wake"
                    busy={
                      pending?.entryId === commitment.entry_id &&
                      pending.verb === "reopen"
                    }
                    disabled={
                      pending !== null && pending.entryId !== commitment.entry_id
                    }
                    error={rowErrors[commitment.entry_id]}
                    onAction={() => reopen(commitment)}
                    onOpen={() => openSource(commitment)}
                  />
                </li>
              ))}
            </Shelf>
          )}

          {settledRows.length > 0 && (
            <Shelf
              label="Settled"
              count={settledRows.length}
              open={showSettled}
              onToggle={() => setShowSettled((open) => !open)}
              testId="settled-commitments"
            >
              {settledRows.map((commitment) => (
                <li key={commitment.entry_id} className={WATCH_ROW}>
                  <ShelfRow
                    commitment={commitment}
                    // Never silent: an entry an evidence pass closed on its own
                    // says who closed it, right beside the undo.
                    meta={settledBy(commitment)}
                    evidence={commitment.evidence[0]?.reference ?? null}
                    actionLabel="Reopen"
                    busy={
                      pending?.entryId === commitment.entry_id &&
                      pending.verb === "reopen"
                    }
                    disabled={
                      pending !== null && pending.entryId !== commitment.entry_id
                    }
                    error={rowErrors[commitment.entry_id]}
                    onAction={() => reopen(commitment)}
                    onOpen={() => openSource(commitment)}
                    muted
                  />
                </li>
              ))}
            </Shelf>
          )}
        </>
      )}

      {snoozeTarget && (
        <SnoozeDialog
          title="Snooze this commitment"
          subject={commitmentText(snoozeTarget)}
          busy={pending?.entryId === snoozeTarget.entry_id}
          onClose={() => setSnoozeTarget(null)}
          onConfirm={async (until) => {
            await snooze(snoozeTarget, until);
            setSnoozeTarget(null);
          }}
        />
      )}

      {bulkSnoozeOpen && (
        <SnoozeDialog
          title={
            selectedIds.size === 1
              ? "Snooze this commitment"
              : `Snooze ${selectedIds.size} commitments`
          }
          subject="They stay tracked, and come back on the day you pick."
          busy={bulkPending === "snooze"}
          onClose={() => setBulkSnoozeOpen(false)}
          onConfirm={async (until) => {
            await snoozeSelected(until);
            setBulkSnoozeOpen(false);
          }}
        />
      )}
    </ViewFrame>
  );
}

type TriageStripProps = {
  groups: TriageGroup[];
  count: number;
  selectedIds: ReadonlySet<string>;
  pending: string | null;
  error: string | null;
  skipped: number;
  onToggle: (ids: string[], selected: boolean) => void;
  onKeep: (rows: Commitment[]) => void;
  onUntrack: (rows: Commitment[]) => void;
  onSnooze: () => void;
};

/**
 * The review-after-the-fact strip: what the ledger enrolled since you last
 * looked, grouped by the meeting that produced it.
 *
 * **Not a gate, and the design says so.** Every row here is already live and
 * already counted in the queue below; nothing waited for a blessing. The strip
 * is contextual chrome doing one job — clearing a backlog of attention, not of
 * work — and it disappears the moment there is nothing new, so it can never
 * become an inbox that goes stale and takes the ledger's credibility with it.
 *
 * That is also why Keep is the primary rectangle and Untrack is quiet: the
 * common answer is "yes, that is a real commitment", and the strip should cost
 * a glance, not a decision per row.
 *
 * The row carries three controls where the two-affordance ceiling
 * (UI_CONVENTIONS §5) allows two. The argued exception is the checkbox: it is
 * selection chrome for the bar below, not a third verb on the item, and it is
 * what lets a heavy day be cleared by the meeting rather than by the row.
 */
function TriageStrip({
  groups,
  count,
  selectedIds,
  pending,
  error,
  skipped,
  onToggle,
  onKeep,
  onUntrack,
  onSnooze,
}: TriageStripProps) {
  const allRows = groups.flatMap((group) => group.rows);
  const selectedRows = allRows.filter((row) => selectedIds.has(row.entry_id));
  const busy = pending !== null;

  return (
    <section
      className={`mt-4 mb-6 rounded-card border border-edge px-5 py-4 ${RISES_IN}`}
      aria-label="New commitments to review"
      data-testid="triage-strip"
    >
      <p className="text-[13px] text-ink">
        {count === 1
          ? "1 new since you last looked"
          : `${count} new since you last looked`}
      </p>

      {groups.map((group) => {
        const ids = group.rows.map((row) => row.entry_id);
        const allSelected = ids.every((id) => selectedIds.has(id));
        return (
          <div key={group.noteId} className="mt-4">
            <div className="flex items-center gap-2.5">
              <Checkbox
                checked={allSelected}
                hideLabel
                label={`Select all from ${group.label}`}
                onChange={(next) => onToggle(ids, next)}
              />
              <p className={TRIAGE_GROUP_LABEL}>
                {group.rows.length} from {group.label}
              </p>
            </div>
            <ul className="mt-1.5 flex flex-col">
              {group.rows.map((row) => (
                <li
                  key={row.entry_id}
                  className="flex items-center gap-2.5 border-t border-edge py-2 first:border-t-0"
                >
                  <Checkbox
                    checked={selectedIds.has(row.entry_id)}
                    hideLabel
                    label={`Select ${commitmentText(row)}`}
                    onChange={(next) => onToggle([row.entry_id], next)}
                  />
                  <span className="min-w-0 flex-1 truncate text-[12.5px] text-ink-dim">
                    {commitmentOwner(row)} · {commitmentText(row)}
                  </span>
                  <Button
                    variant="action"
                    disabled={busy}
                    onClick={() => onKeep([row])}
                  >
                    Keep
                  </Button>
                  <Button
                    variant="quiet"
                    disabled={busy}
                    onClick={() => onUntrack([row])}
                  >
                    Untrack
                  </Button>
                </li>
              ))}
            </ul>
          </div>
        );
      })}

      {selectedRows.length > 0 && (
        <div
          className="mt-4 flex items-center gap-2.5 border-t border-edge pt-3.5"
          data-testid="triage-selection"
        >
          <span className="flex-1 font-data text-[10.5px] text-ink-faint tabular-nums">
            {selectedRows.length} selected
          </span>
          <Button
            variant="action"
            disabled={busy}
            onClick={() => onKeep(selectedRows)}
          >
            Keep
          </Button>
          <Button
            variant="quiet"
            loading={pending === "untrack"}
            disabled={busy && pending !== "untrack"}
            onClick={() => onUntrack(selectedRows)}
          >
            Untrack
          </Button>
          <Button variant="quiet" disabled={busy} onClick={onSnooze}>
            Snooze
          </Button>
        </div>
      )}

      {skipped > 0 && (
        <div className="mt-3">
          <StatusMessage variant="status" compact>
            {skipped === 1
              ? "1 commitment couldn't be changed. It may need review first."
              : `${skipped} commitments couldn't be changed. They may need review first.`}
          </StatusMessage>
        </div>
      )}
      {error && (
        <div className="mt-3">
          <StatusMessage variant="error" compact>
            {error}
          </StatusMessage>
        </div>
      )}
    </section>
  );
}

/** The strip's group heading. Same eyebrow recipe as the queue's, without the
 * queue's own top margin: the strip sets its own rhythm. */
const TRIAGE_GROUP_LABEL =
  "font-data text-[10px] uppercase tracking-[0.22em] text-ink-faint";

/** The stagger counts one sequence over the whole page, labels included. */
function staggerPosition(
  groups: CommitmentGroup[],
  groupIndex: number,
  rowIndex: number,
): number {
  let position = 0;
  for (let index = 0; index < groupIndex; index += 1) {
    position += 1 + groups[index].rows.length;
  }
  return position + 1 + rowIndex;
}

type RowProps = {
  commitment: Commitment;
  pending: Pending | null;
  error?: string;
  showProject: boolean;
  onToggle: (commitment: Commitment, done: boolean) => void;
  onOpen: (commitment: Commitment) => void;
  onReopen: (commitment: Commitment) => void;
  onSnooze: (commitment: Commitment, until: string) => Promise<void>;
  onSnoozePick: (commitment: Commitment) => void;
  onWaive: (commitment: Commitment) => void;
  onUntrack: (commitment: Commitment) => void;
  onClaimMine: (commitment: Commitment) => void;
  onConfirm: (commitment: Commitment, evidenceId: string) => void;
  onDismiss: (commitment: Commitment, evidenceId: string) => void;
};

/**
 * One live commitment.
 *
 * Three regions, and the card is not a button: the checkbox commits the note,
 * the body is the click-through, and the rail holds the judgements. Two
 * affordances is the row's ceiling (docs/UI_CONVENTIONS.md §5), which is why
 * snooze and waive collapse behind one menu rather than lining up as buttons.
 */
function CommitmentRow({
  commitment,
  pending,
  error,
  showProject,
  onToggle,
  onOpen,
  onReopen,
  onSnooze,
  onSnoozePick,
  onWaive,
  onUntrack,
  onClaimMine,
  onConfirm,
  onDismiss,
}: RowProps) {
  const busy = pending?.entryId === commitment.entry_id;
  const otherRowBusy = pending !== null && !busy;
  const text = commitmentText(commitment);
  const item = commitment.item;
  const review = needsReview(commitment);
  const claim = commitment.evidence[0];
  // Fresh says nothing: it is the absence of a problem, and a row that reads
  // "fresh" spends a line telling you everything is fine.
  const tierLabel = commitment.tier === "fresh" ? null : commitment.tier;
  const heard = [tierLabel, `heard ${formatInstant(commitment.last_mention)}`]
    .filter(Boolean)
    .join(" · ");
  // Only stale earns the promotion, and only when overdue is not already
  // holding the row's one promoted slot.
  const promoteTier = commitment.tier === "stale" && item?.status !== "overdue";

  return (
    <>
      {review ? (
        // The left slot keeps the checkbox's width so the text column stays
        // aligned with every other row: a state change must not move the
        // layout box of the rows around it.
        <span aria-hidden className="mt-0.5 size-[17px] flex-none" />
      ) : (
        <span className="mt-0.5 flex-none">
          <Checkbox
            hideLabel
            label={`Mark "${text}" done`}
            checked={item?.done ?? false}
            busy={busy && pending?.verb === "done"}
            // A commitment whose source line is gone has no box to tick. The
            // ledger still holds it, which is the entire reason it is here.
            disabled={!item || otherRowBusy}
            onChange={(done) => onToggle(commitment, done)}
          />
        </span>
      )}

      <div className="min-w-0 flex-1">
        <button
          type="button"
          className="focus-ring-inset block w-full cursor-pointer rounded-[6px] text-left"
          disabled={!commitment.source}
          onClick={() => onOpen(commitment)}
        >
          <span
            className={`block text-[14.5px] font-semibold ${
              item?.done ? "text-ink-dim line-through" : "text-ink"
            }`}
          >
            {text}
          </span>
          {/* One faint run joined by middots, with the promoted segment beside
              it as its own child — the meta grammar `NeedsAttentionView` set. */}
          <span className={META_LINE}>
            {commitment.direction === "theirs" && (
              // Promoted one step out of the faint register: on this half of
              // the page, who owes it is the first thing you want. Only on this
              // half: an unassigned row would print "Unassigned" under a
              // heading that already says it.
              <span className="text-ink-dim">{commitmentOwner(commitment)}</span>
            )}
            {item?.status === "overdue" && item.due_date && (
              // Overdue reads as weight and position, never as marigold:
              // marigold is failure, and a late commitment is not a failure
              // (docs/DESIGN_SYSTEM.md §2).
              <span className="text-ink-dim">
                overdue · due {formatDay(item.due_date)}
              </span>
            )}
            {promoteTier && (
              // Stale takes the same promotion overdue does, and for the same
              // reason: age is weight and position, never a hue. A hue answers
              // which project and nothing else (docs/DESIGN_SYSTEM.md §2), so
              // there is deliberately no colour here. One promoted segment per
              // row, which is why an overdue row leaves this faint.
              <span className="text-ink-dim">{heard}</span>
            )}
            <span>
              {[
                item?.status !== "overdue" && item?.due_date
                  ? `due ${formatDay(item.due_date)}`
                  : null,
                showProject ? commitment.project : null,
                // What kind of room this came out of. Always faint, never
                // promoted: it is context for a commitment, not a claim on
                // attention, and the row's one promoted slot belongs to overdue
                // or stale.
                commitment.source?.category
                  ? categoryLabel(commitment.source.category)
                  : null,
                promoteTier ? null : heard,
              ]
                .filter(Boolean)
                .join(" · ")}
            </span>
          </span>
        </button>

        {review && (
          <>
            {commitment.review_reason && (
              <p className="mt-1.5 text-[12.5px] leading-[1.55] text-ink-dim">
                {commitment.review_reason}
              </p>
            )}
            {commitment.evidence.map((evidence) => (
              <p key={evidence.evidence_id} className={META_LINE}>
                <span>
                  {`${evidence.source} · ${formatConfidence(evidence.confidence)}`}
                </span>
                {evidence.reference && (
                  // A plain anchor, the treatment note-body links already get
                  // (`NoteMarkdown`): the app carries no opener plugin, so
                  // this is the app's one existing answer for an external URL
                  // rather than a second one invented here. `_blank` so a
                  // click can never navigate the shell away from itself.
                  <a
                    href={evidence.reference}
                    target="_blank"
                    rel="noreferrer"
                    className="focus-ring rounded-[3px] underline decoration-dotted underline-offset-2 hover:text-ink-dim"
                  >
                    evidence
                  </a>
                )}
                <span>seen {formatInstant(evidence.observed_at)}</span>
              </p>
            ))}
          </>
        )}

        {error && (
          <StatusMessage variant="error" compact className="mt-2">
            {error}
          </StatusMessage>
        )}
      </div>

      <div className="flex flex-none flex-col items-stretch gap-1.5 border-l border-edge pl-5">
        {review && claim ? (
          <>
            <Button
              disabled={otherRowBusy}
              loading={busy && pending?.verb === "confirm"}
              onClick={() => onConfirm(commitment, claim.evidence_id)}
            >
              Mark done
            </Button>
            <Button
              variant="quiet"
              disabled={otherRowBusy}
              loading={busy && pending?.verb === "dismiss"}
              onClick={() => onDismiss(commitment, claim.evidence_id)}
            >
              Keep open
            </Button>
          </>
        ) : (
          <Menu.Root>
            <Menu.Trigger
              render={
                <Button
                  variant="quiet"
                  aria-label={`Actions for "${text}"`}
                  disabled={otherRowBusy}
                  loading={
                    busy &&
                    (pending?.verb === "snooze" ||
                      pending?.verb === "waive" ||
                      pending?.verb === "untrack")
                  }
                >
                  ⋯
                </Button>
              }
            />
            <Menu.Content align="end">
              {review && (
                // A review with no claim behind it is sync saying "your source
                // line vanished, what happened?" — most often because someone
                // edited the wording. Saying it is still open answers that and
                // clears the flag; without this the entry would sit in review
                // forever, since the confirm/dismiss pair above needs a claim
                // to act on.
                <Menu.Item onClick={() => onReopen(commitment)}>
                  It is still open
                </Menu.Item>
              )}
              <Menu.Item onClick={() => void onSnooze(commitment, tomorrowIso())}>
                Snooze until tomorrow
              </Menu.Item>
              <Menu.Item onClick={() => void onSnooze(commitment, nextWeekIso())}>
                Snooze for a week
              </Menu.Item>
              <Menu.Item onClick={() => onSnoozePick(commitment)}>
                Snooze until a date…
              </Menu.Item>
              {commitment.direction !== "mine" && (
                <>
                  <Menu.Separator />
                  {/* A verdict about who this row belongs to, which is what
                      this slot holds. It also teaches the name, so the next
                      meeting files it right without being asked: every
                      correction is training data, the same loop routing
                      corrections run. */}
                  <Menu.Item onClick={() => onClaimMine(commitment)}>
                    This is mine
                  </Menu.Item>
                </>
              )}
              <Menu.Separator />
              {/* No confirmation on either: each writes one ledger row, touches
                  no note, and Reopen on the shelf below takes it straight back.
                  A confirm here would teach a danger that is not there. */}
              <Menu.Item onClick={() => onWaive(commitment)}>Waive</Menu.Item>
              {/* The two ways out of the working set, together, because the
                  choice between them is what the reader is making: waive says
                  this was mine and stopped mattering, untrack says it was never
                  my business. */}
              <Menu.Item onClick={() => onUntrack(commitment)}>Untrack</Menu.Item>
            </Menu.Content>
          </Menu.Root>
        )}
      </div>
    </>
  );
}

type ShelfRowProps = {
  commitment: Commitment;
  meta: string;
  evidence?: string | null;
  actionLabel: string;
  busy: boolean;
  disabled: boolean;
  error?: string;
  onAction: () => void;
  onOpen: () => void;
  muted?: boolean;
};

/** A row on a shelf: what it is, what settled it, and the one way back. */
function ShelfRow({
  commitment,
  meta,
  evidence,
  actionLabel,
  busy,
  disabled,
  error,
  onAction,
  onOpen,
  muted = false,
}: ShelfRowProps) {
  return (
    <>
      <div className="min-w-0 flex-1">
        <button
          type="button"
          className="focus-ring-inset block w-full cursor-pointer rounded-[6px] text-left"
          disabled={!commitment.source}
          onClick={onOpen}
        >
          <span
            className={`block text-[14px] ${muted ? "text-ink-dim" : "text-ink"}`}
          >
            {commitmentText(commitment)}
          </span>
          <span className={META_LINE}>
            <span>
              {[
                commitmentOwner(commitment),
                meta,
                // Same quiet segment the live rows carry, so a shelved row
                // still says what kind of room it came out of.
                commitment.source?.category
                  ? categoryLabel(commitment.source.category)
                  : null,
                formatInstant(commitment.updated_at),
              ]
                .filter(Boolean)
                .join(" · ")}
            </span>
          </span>
        </button>
        {evidence && (
          <p className={META_LINE}>
            <a
              href={evidence}
              target="_blank"
              rel="noreferrer"
              className="focus-ring rounded-[3px] underline decoration-dotted underline-offset-2 hover:text-ink-dim"
            >
              evidence
            </a>
          </p>
        )}
        {error && (
          <StatusMessage variant="error" compact className="mt-2">
            {error}
          </StatusMessage>
        )}
      </div>
      <div className="flex flex-none items-start border-l border-edge pl-5">
        <Button
          variant="quiet"
          disabled={disabled}
          loading={busy}
          onClick={onAction}
        >
          {actionLabel}
        </Button>
      </div>
    </>
  );
}

type ShelfProps = {
  label: string;
  count: number;
  open: boolean;
  onToggle: () => void;
  testId: string;
  children: ReactNode;
};

/** A collapsed register below the live groups. One disclosure per section,
 * never nested (docs/UI_CONVENTIONS.md §5). */
function Shelf({ label, count, open, onToggle, testId, children }: ShelfProps) {
  return (
    <div className="mt-[22px]">
      <Button
        variant="quiet"
        aria-expanded={open}
        data-testid={`show-${testId}`}
        onClick={onToggle}
      >
        {label}
        <span className="text-ink-faint"> · {count}</span>
      </Button>
      {open && (
        <ul className="mt-4 flex flex-col" data-testid={testId}>
          {children}
        </ul>
      )}
    </div>
  );
}

/** First-run copy: what a ledger is, and how anything gets into it. */
function EmptyLedger({ scoped }: { scoped: boolean }) {
  return (
    <div className="flex flex-col items-center gap-1.5 pt-24 pb-[70px] text-center">
      <SpiritMark mode="idle" size="26px" className="mb-[18px]" />
      <p className="text-[15px] font-semibold text-ink">Nothing promised yet.</p>
      <p className="max-w-[44ch] text-[12.5px] leading-[1.55] text-ink-dim">
        {scoped
          ? "No commitments in this project yet. Promises heard in its meetings land here on their own."
          : "Kodabi keeps a ledger of the commitments it hears in your conversations: what you owe people, and what they owe you. Capture a meeting and the promises in it land here on their own."}
      </p>
    </div>
  );
}

type SnoozeDialogProps = {
  /** The heading, which names how many commitments are in hand. */
  title: string;
  /** What is being snoozed, as the person reads it: one commitment's text, or
   * a count when the gesture covers several. */
  subject: string;
  busy: boolean;
  onClose: () => void;
  onConfirm: (until: string) => void;
};

/** The custom snooze date, for the two presets that do not fit.
 *
 * Takes its copy rather than a `Commitment` so the triage strip's bulk snooze
 * reuses it: the date field and its floor are the whole point of the dialog,
 * and neither depends on how many rows are behind it. */
function SnoozeDialog({
  title,
  subject,
  busy,
  onClose,
  onConfirm,
}: SnoozeDialogProps) {
  const [until, setUntil] = useState(tomorrowIso());

  return (
    <Dialog open onDismiss={onClose} label={title}>
      <p className="text-[13.5px] text-ink">{title}</p>
      <p className="mt-1.5 text-[12.5px] leading-[1.55] text-ink-dim">
        {subject}
      </p>
      <div className="mt-4">
        <Field
          label="Show it again on"
          type="date"
          min={tomorrowIso()}
          value={until}
          onChange={(event) => setUntil(event.target.value)}
        />
      </div>
      <div className="mt-5 flex justify-end gap-2.5">
        <Button variant="quiet" onClick={onClose}>
          Cancel
        </Button>
        <Button loading={busy} onClick={() => onConfirm(until)}>
          Snooze
        </Button>
      </div>
    </Dialog>
  );
}
