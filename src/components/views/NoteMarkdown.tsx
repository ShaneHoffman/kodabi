import { clsx } from "clsx";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

/**
 * A note's body, rendered in the Grove reading ramp.
 *
 * Every element is styled by a `components` override rather than by descendant
 * selectors in a stylesheet, because Grove's rule is that a component carries
 * its own classes (docs/UI_CONVENTIONS.md §6). The pre-Grove view did the
 * opposite — it handed the markdown to react-markdown bare and let
 * `markdownReading.css` reach into the output — which is why that file is
 * SHARED with ChatView and cannot be deleted here. This view simply stops
 * consuming it; the chat's own Grove ticket retires the rest.
 *
 * The GFM class names below (`contains-task-list`, `task-list-item`) are
 * mdast-util-to-hast's, not ours: it stamps them on the list and the item of
 * every task list, which is the only hook distinguishing an action item from an
 * ordinary bullet. They are read here, never written.
 */
export function NoteMarkdown({ body }: { body: string }) {
  return (
    <ReactMarkdown remarkPlugins={[remarkGfm]} components={NOTE_COMPONENTS}>
      {body}
    </ReactMarkdown>
  );
}

/** A section head INSIDE a note: the caps register, not the document register.
 *
 * The note's own title is the only 26px thing on the screen, so a `##` in the
 * body cannot also be a heading-sized heading — it would compete with the title
 * for the same job. It becomes an eyebrow instead: Bahnschrift, small, tracked
 * wide, one step down the ink ladder. Every level draws the same, because a
 * distilled note is one level deep in practice and a reader gains nothing from
 * telling an `h3` from an `h4` by eye. The tag stays semantic regardless — the
 * outline a screen reader builds is the markdown's, not the styling's. */
const SECTION_HEAD =
  "mt-7 font-ui text-[12px] font-semibold uppercase tracking-[0.18em] leading-none text-ink-dim";

const NOTE_COMPONENTS: Components = {
  h1: ({ children }) => <h1 className={SECTION_HEAD}>{children}</h1>,
  h2: ({ children }) => <h2 className={SECTION_HEAD}>{children}</h2>,
  h3: ({ children }) => <h3 className={SECTION_HEAD}>{children}</h3>,
  h4: ({ children }) => <h4 className={SECTION_HEAD}>{children}</h4>,
  h5: ({ children }) => <h5 className={SECTION_HEAD}>{children}</h5>,
  h6: ({ children }) => <h6 className={SECTION_HEAD}>{children}</h6>,

  p: ({ children }) => <p className="mt-3 text-pretty">{children}</p>,
  a: ({ children, href }) => (
    <a href={href} className="text-ink underline underline-offset-2">
      {children}
    </a>
  ),

  // A task list is laid out as rows rather than bullets: the checkbox IS the
  // marker, so a disc beside it would be two markers for one item. Both list
  // tags are handled — GFM allows an ordered task list.
  ul: ({ children, className }) => (
    <ul className={listClass(className, "list-disc")}>{children}</ul>
  ),
  ol: ({ children, className }) => (
    <ol className={listClass(className, "list-decimal")}>{children}</ol>
  ),

  li: ({ children, className }) =>
    className?.includes("task-list-item") ? (
      // The done state hangs off the real `:checked` of the input below, so
      // there is nothing to track and nothing to keep in sync. `line-through`
      // cannot leak into the box: the checkbox is an atomic inline-level box
      // (`grid`), and text-decoration does not propagate into one.
      <li className="flex items-start gap-2.5 has-[:checked]:text-ink-faint has-[:checked]:line-through">
        {children}
      </li>
    ) : (
      <li className="mt-1">{children}</li>
    ),

  input: ({ type, checked }) =>
    type === "checkbox" ? <TaskCheckbox checked={checked === true} /> : null,

  code: ({ children }) => (
    <code className="rounded-[6px] bg-wash px-1 font-data text-[12px]">{children}</code>
  ),
  pre: ({ children }) => (
    <pre className="mt-3 overflow-x-auto rounded-[10px] bg-wash p-3 [&>code]:bg-transparent [&>code]:p-0">
      {children}
    </pre>
  ),
  blockquote: ({ children }) => (
    <blockquote className="mt-3 border-l-2 border-edge pl-3 text-ink-dim">
      {children}
    </blockquote>
  ),
  hr: () => <hr className="my-6 border-t border-edge" />,
};

/** A list's classes, branching on whether GFM called it a task list. */
function listClass(className: string | undefined, marker: string): string {
  return className?.includes("contains-task-list")
    ? "mt-5 list-none space-y-1.5 p-0"
    : clsx("mt-3 space-y-1 pl-5", marker);
}

/**
 * An action item's box.
 *
 * Unchecked is a ring and nothing else. Checked is a filled square in the
 * supporting-text ink with a ground-coloured check drawn from two rotated
 * borders — no icon font, no SVG asset — and the label beside it goes faint and
 * struck, so done work recedes without disappearing. Value, never hue.
 *
 * It renders DISABLED, and honestly so: mdast-util-to-hast hard-codes
 * `disabled: true` on a task-list item, and there is no write path behind a
 * click even if there were a handler. Making these live means writing the body
 * back through `save_note`, which is a feature and not a style fix. Until then
 * the box states the item's status and takes no hover and no pointer cursor: a
 * control that cannot be operated must not advertise that it can.
 *
 * `readOnly` alongside `checked` is React's requirement, not the DOM's — a
 * `checked` input with no `onChange` warns otherwise.
 */
function TaskCheckbox({ checked }: { checked: boolean }) {
  return (
    <input
      type="checkbox"
      checked={checked}
      disabled
      readOnly
      className={clsx(
        "mt-[3px] grid size-3.5 flex-none appearance-none place-content-center",
        "rounded-[4px] border border-ink-faint",
        "checked:border-ink-dim checked:bg-ink-dim",
        "before:h-[5px] before:w-2 before:-translate-y-px before:-rotate-45",
        "before:border-b-2 before:border-l-2 before:border-ground before:opacity-0",
        "before:content-[''] checked:before:opacity-100",
      )}
    />
  );
}
