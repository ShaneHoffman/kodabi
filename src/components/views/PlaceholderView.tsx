import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";

type Props = {
  title: string;
  caption: string;
  /** Quiet provenance line, e.g. the ticket that fills this seam. */
  detail?: string;
};

/**
 * The shared Ma-forward placeholder every unbuilt destination renders:
 * one thing per view, hierarchy from type and space alone.
 *
 * It used to centre itself, which made it a fourth empty-state treatment
 * alongside three left-aligned ones. A destination that exists but has nothing
 * in it yet is an empty state like any other, so it now sits in the same
 * scaffold and reads down the same column as every other view
 * (docs/DESIGN_SYSTEM.md §3).
 */
export function PlaceholderView({ title, caption, detail }: Props) {
  return (
    <ViewFrame eyebrow={detail} title={title}>
      <StatusMessage variant="empty" className="max-w-measure">
        {caption}
      </StatusMessage>
    </ViewFrame>
  );
}
