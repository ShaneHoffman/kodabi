import { formatDay } from "../../commitmentGroups";
import { useNavigation } from "../../useNavigation";
import type { DigestItem } from "../../useDigest";
import { useDailyDigest } from "../../useDigest";
import { Button } from "../ui/Button";

/** The entrance, and its reduced partner. Matches the triage strip's. */
const RISES_IN = "animate-rise-in motion-reduce:animate-fade-in";

/** The meta register: dates and counts, never a sentence to read. */
const META_LINE =
  "mt-1 font-data text-[10.5px] text-ink-faint tabular-nums";

/**
 * The daily digest: what moved in the commitment ledger since it last ran.
 *
 * **Contextual chrome, and news rather than work.** A bordered rectangle, not
 * a plane: the cards in this app are things you are meant to clear, and
 * nothing here is. Every row is already live and already counted wherever it
 * belongs; the digest only says it *changed*, and it disappears the moment
 * there is nothing new. That is what keeps it from becoming a second inbox
 * that goes stale, which is the failure the triage strip is shaped around too.
 *
 * **No per-row actions, on purpose.** The one affordance is the way to the
 * Commitments view, and getting around is not acting (UI_CONVENTIONS §5). A
 * digest that could snooze and waive would be the Commitments view wearing a
 * smaller hat, and the point of the surface is that it costs a glance.
 *
 * Renders nothing at all when the day has no news, when the digest has not
 * arrived, or when the ledger could not answer. All three are correctly the
 * same on screen: no card. The Commitments view is where a broken ledger says
 * so.
 */
export function DigestCard() {
  const digest = useDailyDigest();
  const { navigate } = useNavigation();

  if (!digest || digest.items.length === 0) return null;

  const total = digest.items.length + digest.more;

  return (
    <section
      className={`mt-4 mb-6 rounded-card border border-edge px-5 py-4 ${RISES_IN}`}
      aria-label="What changed in your commitments"
      data-testid="digest-card"
    >
      <div className="flex items-baseline gap-4">
        <p className="text-[13px] text-ink">
          {total === 1 ? "1 change" : `${total} changes`} since{" "}
          {formatDay(digest.since)}
        </p>
        <div className="ml-auto flex-none">
          <Button
            variant="quiet"
            onClick={() => navigate({ kind: "commitments", slug: null })}
          >
            Open commitments
          </Button>
        </div>
      </div>

      <ul className="mt-3 flex flex-col">
        {digest.items.map((item) => (
          <li
            key={item.entry_id}
            className="border-t border-edge py-2.5 first:border-t-0 first:pt-0"
          >
            <p className="text-[13px] leading-[1.45] text-ink">
              {item.description}
            </p>
            <p className={META_LINE}>{metaFor(item)}</p>
          </li>
        ))}
      </ul>

      {digest.more > 0 && (
        <p className="mt-2.5 font-data text-[10.5px] text-ink-faint tabular-nums">
          {digest.more === 1
            ? "1 more not shown"
            : `${digest.more} more not shown`}
        </p>
      )}
    </section>
  );
}

/**
 * The one meta line a row gets: why it is here, and the single fact that makes
 * that concrete.
 *
 * Each kind names its own change rather than sharing a generic label, because
 * the label IS the news: "overdue" and "gone quiet" ask for different things
 * from the reader, and a row that only said "changed" would send them to the
 * Commitments view to find out which.
 */
function metaFor(item: DigestItem): string {
  const owner = item.owner;
  switch (item.kind) {
    case "newly_overdue":
      return item.due_date
        ? `Now overdue · was due ${formatDay(item.due_date)}`
        : "Now overdue";
    case "parked_in_review":
      return item.review_reason
        ? `Needs review · ${item.review_reason}`
        : "Needs review";
    case "went_stale":
      return item.last_mention
        ? `Went stale · last mentioned ${formatDay(item.last_mention.slice(0, 10))}`
        : "Went stale";
    case "theirs_quiet":
      return item.quiet_days === null
        ? `Waiting on ${owner}`
        : `Waiting on ${owner} · quiet ${item.quiet_days} ${
            item.quiet_days === 1 ? "day" : "days"
          }`;
  }
}
