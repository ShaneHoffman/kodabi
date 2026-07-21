/**
 * Where a textarea's selection actually sits on screen.
 *
 * `window.getSelection()` returns nothing useful inside a form control, so the
 * only way to anchor UI to a textarea's selection is to measure it: build a
 * div that copies every property affecting text layout, fill it with the text
 * up to the selection, and read the offset of a marker span placed at that
 * point. It is the standard technique, and it is exact as long as the copied
 * property list is complete.
 *
 * Used by the note editor's format toolbar, which has to appear just above
 * whatever the user has selected — chrome on demand, anchored to the thing it
 * acts on rather than parked at the top of the screen.
 */

/** Every property that can move a glyph. Missing one puts the mirror out of
 * step with the real control, which shows up as a toolbar drifting a few
 * pixels further off with every line. */
const MIRRORED = [
  "boxSizing",
  "width",
  "paddingTop",
  "paddingRight",
  "paddingBottom",
  "paddingLeft",
  "borderTopWidth",
  "borderRightWidth",
  "borderBottomWidth",
  "borderLeftWidth",
  "fontFamily",
  "fontSize",
  "fontWeight",
  "fontStyle",
  "letterSpacing",
  "lineHeight",
  "textTransform",
  "textIndent",
  "whiteSpace",
  "wordBreak",
  "overflowWrap",
  "tabSize",
] as const;

export type SelectionAnchor = {
  /** Offsets from the textarea's own top-left, in CSS pixels. */
  left: number;
  top: number;
};

/**
 * The top-centre of the selected range, relative to the textarea's padding
 * box. Returns null when nothing is selected — the caller uses that to take
 * the toolbar away, which is what makes it chrome on demand.
 *
 * For a multi-line selection this anchors to the FIRST line. A toolbar
 * centred on the bounding box of a five-line selection floats in the middle
 * of the user's own text; hanging it off the start keeps it out of the way
 * and next to where the eye already is.
 */
export function selectionAnchor(textarea: HTMLTextAreaElement): SelectionAnchor | null {
  const { selectionStart, selectionEnd, value } = textarea;
  if (selectionStart === selectionEnd) return null;

  const style = window.getComputedStyle(textarea);
  const mirror = document.createElement("div");
  for (const property of MIRRORED) {
    mirror.style[property] = style[property];
  }
  mirror.style.position = "absolute";
  mirror.style.visibility = "hidden";
  mirror.style.whiteSpace = "pre-wrap";
  mirror.style.overflowWrap = "break-word";
  // The mirror must not be able to scroll: it has to lay out at full height so
  // a selection below the fold still measures correctly.
  mirror.style.height = "auto";

  // Everything before the selection lays out the same way it does in the real
  // control; the marker then sits exactly where the selection begins.
  mirror.textContent = value.slice(0, selectionStart);
  const marker = document.createElement("span");
  // A zero-width space, so the marker has a box to measure but adds no glyph
  // that could push the following text and skew the very offset being read.
  marker.textContent = "​";
  mirror.appendChild(marker);

  document.body.appendChild(mirror);
  const left = marker.offsetLeft;
  const top = marker.offsetTop;
  document.body.removeChild(mirror);

  // Selecting past the fold scrolls the textarea; the anchor is relative to
  // the visible box, so the scroll offset comes back out.
  return { left, top: top - textarea.scrollTop };
}

/** Wrap or prefix the current selection, then hand back the new value and
 * where the selection should land. Pure, so the caller stays a plain event
 * handler and nothing here touches React state. */
export function applyMarkup(
  value: string,
  start: number,
  end: number,
  markup: { wrap?: string; prefix?: string },
): { value: string; start: number; end: number } {
  const selected = value.slice(start, end);

  if (markup.wrap) {
    const { wrap } = markup;
    // Toggling: wrapping something already wrapped unwraps it, so pressing B
    // twice is a no-op rather than `****bold****`.
    const alreadyWrapped =
      value.slice(start - wrap.length, start) === wrap &&
      value.slice(end, end + wrap.length) === wrap;
    if (alreadyWrapped) {
      return {
        value:
          value.slice(0, start - wrap.length) + selected + value.slice(end + wrap.length),
        start: start - wrap.length,
        end: end - wrap.length,
      };
    }
    return {
      value: value.slice(0, start) + wrap + selected + wrap + value.slice(end),
      start: start + wrap.length,
      end: end + wrap.length,
    };
  }

  // A prefix applies per line, so selecting three lines and pressing List
  // makes three bullets rather than one bullet and two orphans.
  const prefix = markup.prefix ?? "";
  const lineStart = value.lastIndexOf("\n", start - 1) + 1;
  const block = value.slice(lineStart, end);
  const prefixed = block
    .split("\n")
    .map((line) => (line.startsWith(prefix) ? line : prefix + line))
    .join("\n");
  return {
    value: value.slice(0, lineStart) + prefixed + value.slice(end),
    start: lineStart,
    end: lineStart + prefixed.length,
  };
}
