import { Fragment, useState, type CSSProperties, type ReactNode } from "react";

import {
  arrangeCommitments,
  commitmentOwner,
  commitmentText,
  formatConfidence,
  formatDay,
  formatInstant,
  needsReview,
  nextWeekIso,
  settledBy,
  tomorrowIso,
  workloadSummary,
  type CommitmentGroup,
} from "../../commitmentGroups";
import { backendCopy } from "../../errorCopy";
import { useNavigation, type View } from "../../useNavigation";
import {
  confirmCommitmentEvidence,
  dismissCommitmentEvidence,
  reopenCommitment,
  setCommitmentDone,
  snoozeCommitment,
  untrackCommitment,
  useCommitments,
  waiveCommitment,
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
  }

  const { groups, snoozed, settled: settledRows } = arrangeCommitments(
    entries,
    settled,
  );
  const isEmpty =
    groups.length === 0 && snoozed.length === 0 && settledRows.length === 0;

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
          commitment={snoozeTarget}
          busy={pending?.entryId === snoozeTarget.entry_id}
          onClose={() => setSnoozeTarget(null)}
          onConfirm={async (until) => {
            await snooze(snoozeTarget, until);
            setSnoozeTarget(null);
          }}
        />
      )}
    </ViewFrame>
  );
}

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
                formatInstant(commitment.updated_at),
              ].join(" · ")}
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
  commitment: Commitment;
  busy: boolean;
  onClose: () => void;
  onConfirm: (until: string) => void;
};

/** The custom snooze date, for the two presets that do not fit. */
function SnoozeDialog({
  commitment,
  busy,
  onClose,
  onConfirm,
}: SnoozeDialogProps) {
  const [until, setUntil] = useState(tomorrowIso());

  return (
    <Dialog open onDismiss={onClose} label="Snooze this commitment">
      <p className="text-[13.5px] text-ink">Snooze this commitment</p>
      <p className="mt-1.5 text-[12.5px] leading-[1.55] text-ink-dim">
        {commitmentText(commitment)}
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
