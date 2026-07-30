/**
 * Reading a webview's console buffer for the two complaints that mean the
 * shipping Content Security Policy refused something.
 *
 * Shared because more than one slice gates the policy, and each slice covers
 * directives the others cannot: quick capture exercises `connect-src` and
 * `font-src`, source pairing is the only place a `media-src` consumer is ever
 * mounted. The policy itself is annotated where it is asserted hardest, in
 * `e2e/quick-capture.test.mjs` — that comment documents the CSP, not these
 * helpers, so it stays there.
 */

/**
 * Console lines meaning the policy refused something, or that Tauri gave up on
 * it.
 *
 * Targeted rather than a blanket "no console errors": WebView2 emits noise that
 * varies by runtime build, and docs/UI_E2E_HARNESS.md commits to *retiring* this
 * tier if a flake that is not a real bug goes unfixed for one attempt. A gate a
 * WebView2 auto-update can turn red is a gate that gets deleted.
 *
 * Two patterns rather than one. The first is Chromium's wording for every
 * refusal whatever the directive, so it also catches a future img-src or
 * media-src regression. The second pins that specific symptom, and stays valid
 * if Chromium ever rewords the first.
 */
const CSP_COMPLAINTS = [/Content Security Policy/i, /IPC custom protocol failed/i];

/**
 * A refusal line with any inlined `data:` payload elided.
 *
 * Chromium quotes the refused URL, and for a font that URL *is* the base64
 * woff2. It caps its own quoting near a kilobyte, so this is not unbounded —
 * but a `font-src` regression refuses six faces per webview, each line is then
 * a kilobyte of base64, and the `after` hook prints the whole buffer again.
 * Left verbatim that buries the one part worth reading, and it is not the head:
 * the directive that names the cause comes *after* the URL, so a plain
 * truncation would drop exactly it. Eliding the blob in place keeps both ends
 * and the length, which is all the payload was ever going to tell anyone.
 */
export function elideDataUrls(line) {
  return line.replace(
    /data:[^'"\s)]{60,}/g,
    (blob) => `${blob.slice(0, 40)}…<${blob.length} chars elided>`,
  );
}

/** Every buffered console line from `session` that matches a complaint pattern. */
export function cspComplaints(session) {
  return session
    .consoleLog()
    .filter((line) => CSP_COMPLAINTS.some((re) => re.test(line)))
    .map(elideDataUrls);
}
