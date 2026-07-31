/**
 * In-page helpers, expressed as plain CDP expressions.
 *
 * Everything here selects by `data-testid` rather than by copy or DOM shape,
 * per the discipline in docs/UI_CONVENTIONS.md. Nothing here uses `eval`,
 * `new Function`, or an injected <script> — all three are either blocked by the
 * page CSP or needlessly close to it (see the header of cdp.mjs).
 */

/** Embeds a JS value into an expression safely. */
const lit = (value) => JSON.stringify(value);

export const byTestId = (testId) => `document.querySelector('[data-testid="${testId}"]')`;

/**
 * Types into a React-controlled input or textarea.
 *
 * React installs its own value setter on the element instance to track changes,
 * so assigning `.value` directly updates the DOM but leaves React's state
 * untouched — the component re-renders straight back over it. The fix is to
 * call the *prototype's* native setter and then dispatch the event React
 * listens for.
 *
 * preview-mock.js has the same trick for the command palette, but reads the
 * descriptor off `HTMLInputElement.prototype` specifically, which throws
 * `Illegal invocation` on a <textarea>. Taking it from the element's own
 * prototype works for both, which is why this is the general form.
 */
export async function typeInto(session, testId, text) {
  return session.evaluate(`
    (() => {
      const element = ${byTestId(testId)};
      if (!element) {
        throw new Error("no element with data-testid=" + ${lit(testId)});
      }
      const descriptor = Object.getOwnPropertyDescriptor(
        Object.getPrototypeOf(element),
        "value",
      );
      descriptor.set.call(element, ${lit(text)});
      element.dispatchEvent(new Event("input", { bubbles: true }));
      return true;
    })()
  `);
}

/**
 * Waits for a control to become enabled, then clicks it.
 *
 * The wait is the load-bearing half. React schedules its re-render rather than
 * running it synchronously with the dispatched `input` event, so a click issued
 * in the same evaluate as the typing lands on a still-disabled button and does
 * nothing — the run then fails much later, somewhere unrelated.
 */
export async function clickWhenEnabled(session, testId, { timeoutMs = 5_000 } = {}) {
  await session.waitFor(`!!${byTestId(testId)} && !${byTestId(testId)}.disabled`, {
    timeoutMs,
    label: `${testId} enabled`,
  });
  return session.evaluate(`(${byTestId(testId)}.click(), true)`);
}

/**
 * Clicks the one element carrying `testId` whose descendant `labelTestId` reads
 * exactly `label`.
 *
 * `clickWhenEnabled` takes `querySelector`'s first match, which is right for a
 * singleton control and wrong for a list: a seeded vault puts nine rows in the
 * Inbox and a slice has to open a named one. Throws rather than no-op'ing when
 * nothing matches, because a click that lands nowhere fails much later,
 * somewhere unrelated.
 */
export async function clickRowLabelled(session, testId, labelTestId, label) {
  await session.waitFor(
    `[...document.querySelectorAll('[data-testid="${testId}"]')]
       .some((row) => row.querySelector('[data-testid="${labelTestId}"]')?.textContent.trim() === ${lit(label)})`,
    { label: `a ${testId} labelled ${lit(label)}` },
  );
  return session.evaluate(`
    (() => {
      const row = [...document.querySelectorAll('[data-testid="${testId}"]')].find(
        (candidate) =>
          candidate.querySelector('[data-testid="${labelTestId}"]')?.textContent.trim() === ${lit(label)},
      );
      if (!row) {
        throw new Error("no " + ${lit(testId)} + " labelled " + ${lit(label)});
      }
      row.click();
      return true;
    })()
  `);
}

/**
 * Waits out two animation frames plus `ms`, for the one thing a `waitFor`
 * cannot express: proving something never appears.
 *
 * Used only for the two "this note must NOT pair" assertions. The artifact
 * section is absent both while its fetch is in flight and when it was never
 * mounted at all, so there is no positive signal to wait on — only a bounded
 * window after which a section that was coming would have arrived. The window
 * is defensible rather than arbitrary: every other scenario in that file
 * observes its own section resolve, and they do it in a small fraction of this.
 */
export async function settle(session, ms = 400) {
  return session.evaluate(`
    new Promise((resolve) =>
      requestAnimationFrame(() =>
        requestAnimationFrame(() => setTimeout(() => resolve(true), ${ms})),
      ),
    )
  `);
}

/** The trimmed text of a testid'd element, or null when it is absent. */
export async function textOf(session, testId) {
  return session.evaluate(`${byTestId(testId)}?.textContent.trim() ?? null`);
}

/** Waits until a testid'd element's trimmed text equals `expected` exactly. */
export async function waitForText(session, testId, expected, { timeoutMs = 5_000 } = {}) {
  return session.waitFor(`${byTestId(testId)}?.textContent.trim() === ${lit(expected)}`, {
    timeoutMs,
    label: `${testId} text === ${lit(expected)}`,
  });
}

/** Every trimmed text for a repeated testid, in document order. */
export async function allTextOf(session, testId) {
  return session.evaluate(
    `[...document.querySelectorAll('[data-testid="${testId}"]')].map((node) => node.textContent.trim())`,
  );
}

/**
 * Waits until some element carrying `testId` has exactly this text.
 *
 * Exact equality, not a substring: a note's row title is the verbatim first
 * line of what was captured, so an exact match is what catches a truncation or
 * slug regression.
 */
export async function waitForOneOf(session, testId, expected, { timeoutMs = 10_000 } = {}) {
  return session.waitFor(
    `[...document.querySelectorAll('[data-testid="${testId}"]')]
       .map((node) => node.textContent.trim())
       .includes(${lit(expected)})`,
    { timeoutMs, label: `a ${testId} reading ${lit(expected)}` },
  );
}
