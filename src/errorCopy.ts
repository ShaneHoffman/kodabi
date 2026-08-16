/**
 * What to render when one of **Kodabi's own** commands rejects.
 *
 * A `#[tauri::command]` returns `Result<T, String>`, and since
 * `src-tauri/src/user_errors.rs` that `String` is finished user-facing copy: a
 * sentence naming what failed and what happens next (`docs/DESIGN_SYSTEM.md`
 * §3), with the raw detail already logged on the Rust side. So a rejection that
 * *is* a string is the thing to show, whole and unprefixed.
 *
 * Anything else did not come from that boundary. A `TypeError` from our own
 * code, a thrown object: those carry stack text and internal names, which §3
 * says never to render. They go to the console, and the caller's own sentence
 * goes on screen instead.
 *
 * The split matters because it is the only thing standing between a future
 * untranslated backend error and the screen. Prefixing here (`Couldn't X: ${…}`)
 * would defeat it: a raw string would read as though it had been written for the
 * reader.
 *
 * **Not for third-party plugin commands.** The string test is a proxy for "we
 * wrote this", and it only holds for commands in `src-tauri`. Tauri plugins
 * serialize their own errors to strings too — `tauri-plugin-updater`'s are
 * mostly `#[error(transparent)]` over `reqwest`/`io`/`serde_json`, so a string
 * from `check()` is as raw as any exception. Use [`opaqueFailure`] there.
 */
export function backendCopy(error: unknown, fallback: string): string {
  if (typeof error === "string") {
    return error;
  }
  // Nothing collects this yet, so the console is the only record there is (the
  // same position `AppErrorBoundary` takes).
  console.error(error);
  return fallback;
}

/**
 * The same job for a rejection nobody wrote for a reader: a third-party plugin
 * command, or any other source whose text is a machine fact whatever its type.
 * Always logs, always renders the caller's sentence.
 */
export function opaqueFailure(error: unknown, sentence: string): string {
  console.error(error);
  return sentence;
}
