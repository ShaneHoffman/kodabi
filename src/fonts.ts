// Self-hosted Source trio (design/tokens.css --sans/--serif/--mono) so the
// app renders in the chosen typefaces offline, with no network fetch.
//
// THE IMPORTS ARE THE --fw-* SCALE, EXACTLY. That is the whole rule here, and
// it was not being kept: the scale is 400 regular / 500 medium / 600 semibold
// / 700 bold, and this file used to load 300–600. Both ends were wrong.
//
//   - 700 was MISSING from both text faces while it had live consumers in
//     each: `--fw-bold` on the format toolbar's B (sans), and every `**bold**`
//     word inside a note body (serif — Tailwind's preflight sets `strong` to
//     `font-weight: bolder`, which resolves to 700 against a 400 parent). A
//     weight with no face loaded does not fall back to 600; the engine
//     synthesises one by smearing the 400 outlines, which is why those read
//     blurry and slightly too wide.
//   - 300 was LOADED and referenced by nothing. No token names it, so it was
//     four font files (~150 kB across the two faces and both subsets) that no
//     glyph in the app could ever be set in.
//
// Italic is the same story and the same fix: the toolbar's I is styled
// `italic`, @fontsource ships italics as separate `*-italic.css` entries, and
// none was imported — so that glyph was a synthetic oblique, a 400 roman
// sheared over. Source Sans 3's real italic is a different drawing.
//
// The mono face carries neither 300 nor an italic: monospace text in this app
// is metadata and paths, which are never emphasised.

// PRE-GROVE, AND THE WHOLE FILE IS ON THE WAY OUT. Grove's three faces
// (Bahnschrift, Cascadia Mono, Georgia) all ship with Windows, so the Grove app
// fetches no font and needs no @fontsource. These stay only because the
// unmigrated screens are still set in the Source trio; the final cleanup ticket
// deletes this module along with the three dependencies.
//
// One block disable rather than fifteen line comments: it is one exception with
// one reason, and fifteen copies of that reason would read as fifteen decisions.
/* eslint-disable no-restricted-syntax -- pre-Grove webfonts; the Grove cleanup ticket deletes this module */
import "@fontsource/source-sans-3/400.css";
import "@fontsource/source-sans-3/400-italic.css";
import "@fontsource/source-sans-3/500.css";
import "@fontsource/source-sans-3/600.css";
import "@fontsource/source-sans-3/700.css";

import "@fontsource/source-serif-4/400.css";
import "@fontsource/source-serif-4/500.css";
import "@fontsource/source-serif-4/600.css";
import "@fontsource/source-serif-4/700.css";

import "@fontsource/source-code-pro/400.css";
import "@fontsource/source-code-pro/500.css";
import "@fontsource/source-code-pro/600.css";
/* eslint-enable no-restricted-syntax */
