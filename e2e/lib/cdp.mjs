/**
 * A minimal Chrome DevTools Protocol client, just large enough to drive
 * Kodabi's WebView2 windows.
 *
 * Why hand-rolled: attaching to a target's own `webSocketDebuggerUrl` gives the
 * *flat* protocol — no `Target.attachToTarget`, no `sessionId` threading — which
 * collapses a general CDP client down to request/response over one socket. That
 * is small enough to own, and it keeps the E2E tier at zero dependencies (see
 * docs/UI_E2E_HARNESS.md for why that mattered).
 *
 * Two rules the page's CSP (`script-src 'self'`, src-tauri/tauri.conf.json)
 * imposes on every expression passed to `evaluate`:
 *   1. Never inject a <script> element. That script is page-originated and is
 *      blocked; debugger-originated evaluation is not.
 *   2. Never use `eval` or `new Function`. Plain expressions only.
 * Neither costs anything — everything the harness needs is a plain expression.
 */

/** Node 22+ ships a global WebSocket; this harness requires it. */
if (typeof WebSocket === "undefined") {
  throw new Error("this harness needs Node 22+ for the global WebSocket");
}

const DEFAULT_TIMEOUT_MS = 10_000;

async function getJson(port, path) {
  // 127.0.0.1, not localhost: on Windows the latter can resolve to ::1 first
  // and the WebView2 debugging endpoint only listens on IPv4.
  const response = await fetch(`http://127.0.0.1:${port}${path}`);
  if (!response.ok) {
    throw new Error(`GET ${path} failed: ${response.status} ${response.statusText}`);
  }
  return response.json();
}

/**
 * Polls the debugging endpoint until the WebView2 browser process answers.
 * Resolves to the /json/version payload, whose Browser string identifies the
 * exact WebView2 build — worth logging, because it is the first thing you want
 * when a run fails only on a CI runner.
 */
export async function waitForEndpoint(port, { timeoutMs = 30_000, intervalMs = 200 } = {}) {
  const deadline = Date.now() + timeoutMs;
  let lastError;
  while (Date.now() < deadline) {
    try {
      return await getJson(port, "/json/version");
    } catch (error) {
      lastError = error;
      await sleep(intervalMs);
    }
  }
  throw new Error(
    `no CDP endpoint on 127.0.0.1:${port} after ${timeoutMs}ms (last error: ${lastError?.message})`,
  );
}

export async function listTargets(port) {
  return getJson(port, "/json/list");
}

/**
 * Finds the one page target whose URL ends with `urlEndsWith`.
 *
 * Requires an *exact* single match and throws with the full target list
 * otherwise. Taking `[0]` from a filtered list is how a harness ends up
 * silently asserting against the wrong window for a week — Kodabi runs three
 * webviews (index.html, capture.html, overlay.html) in one process.
 *
 * Matches on a URL suffix rather than the whole string so the harness survives
 * a scheme change (http://tauri.localhost vs. a dev server), and never on
 * `title`, which two windows could share.
 *
 * Note the main window's suffix is `tauri.localhost/`, not `/index.html`: the
 * `main` window declares no `url` in tauri.conf.json, so it takes the
 * `WebviewUrl` default and the served URL keeps a bare path.
 */
export async function findTarget(port, { urlEndsWith, timeoutMs = 15_000, intervalMs = 200 }) {
  const deadline = Date.now() + timeoutMs;
  let targets = [];
  while (Date.now() < deadline) {
    targets = await listTargets(port);
    const matches = targets.filter(
      (target) => target.type === "page" && target.url.endsWith(urlEndsWith),
    );
    if (matches.length === 1) {
      return matches[0];
    }
    if (matches.length > 1) {
      throw new Error(
        `${matches.length} page targets end with ${urlEndsWith}; expected exactly one:\n${describe(matches)}`,
      );
    }
    await sleep(intervalMs);
  }
  throw new Error(
    `no page target ending with ${urlEndsWith} after ${timeoutMs}ms; saw:\n${describe(targets)}`,
  );
}

function describe(targets) {
  if (targets.length === 0) {
    return "  (no targets at all)";
  }
  return targets.map((target) => `  [${target.type}] ${target.url}`).join("\n");
}

/**
 * Opens a CDP session against one target. `Runtime` and `Log` are enabled
 * immediately and their entries buffered: a failure with no page console is
 * nearly undiagnosable on a CI runner, so every assertion helper can dump them.
 */
export async function attach(target, { timeoutMs = DEFAULT_TIMEOUT_MS } = {}) {
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  const pending = new Map();
  const consoleEntries = [];
  let nextId = 1;
  let closedReason = null;

  await new Promise((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`timed out opening a socket to ${target.url}`)),
      timeoutMs,
    );
    socket.addEventListener("open", () => {
      clearTimeout(timer);
      resolve();
    });
    socket.addEventListener("error", () => {
      clearTimeout(timer);
      reject(new Error(`failed to open a socket to ${target.url}`));
    });
  });

  socket.addEventListener("close", () => {
    closedReason = `the CDP socket to ${target.url} closed`;
    for (const { reject, timer } of pending.values()) {
      clearTimeout(timer);
      reject(new Error(closedReason));
    }
    pending.clear();
  });

  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    if (message.id === undefined) {
      recordEvent(consoleEntries, message);
      return;
    }
    const waiter = pending.get(message.id);
    if (!waiter) {
      return;
    }
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) {
      waiter.reject(new Error(`${message.error.message} (${message.method})`));
    } else {
      waiter.resolve(message.result);
    }
  });

  function send(method, params = {}, { timeoutMs: callTimeout = DEFAULT_TIMEOUT_MS } = {}) {
    if (closedReason) {
      return Promise.reject(new Error(closedReason));
    }
    const id = nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(new Error(`${method} timed out after ${callTimeout}ms`));
      }, callTimeout);
      pending.set(id, { resolve, reject, timer, method });
      socket.send(JSON.stringify({ id, method, params }));
    });
  }

  /**
   * Evaluates a plain expression in the page and returns its value.
   *
   * `awaitPromise` means an expression may end in a promise — that is how the
   * harness calls straight through the real IPC bridge when it needs to.
   */
  async function evaluate(expression, { timeoutMs: callTimeout = DEFAULT_TIMEOUT_MS } = {}) {
    const result = await send(
      "Runtime.evaluate",
      { expression, returnByValue: true, awaitPromise: true },
      { timeoutMs: callTimeout },
    );
    if (result.exceptionDetails) {
      const details = result.exceptionDetails;
      const text = details.exception?.description ?? details.text;
      throw new Error(`evaluate threw in ${target.url}: ${text}\n  expression: ${expression}`);
    }
    return result.result.value;
  }

  /**
   * Polls `expression` until it returns a truthy value.
   *
   * Polling rather than an event subscription is deliberate: it has no ordering
   * hazards, and 100ms of granularity is irrelevant to anything asserted here.
   * The last observed value is reported on timeout, which is usually the whole
   * diagnosis.
   */
  async function waitFor(expression, { timeoutMs: waitTimeout = 5_000, intervalMs = 100, label } = {}) {
    const deadline = Date.now() + waitTimeout;
    const name = label ?? expression;
    let lastValue;
    let lastError;
    while (Date.now() < deadline) {
      try {
        lastValue = await evaluate(expression);
        if (lastValue) {
          return lastValue;
        }
        lastError = undefined;
      } catch (error) {
        lastError = error;
      }
      await sleep(intervalMs);
    }
    const tail = lastError
      ? `last error: ${lastError.message}`
      : `last value: ${JSON.stringify(lastValue)}`;
    throw new Error(`${name}: timed out after ${waitTimeout}ms; ${tail}`);
  }

  await send("Runtime.enable");
  await send("Log.enable");

  return {
    target,
    send,
    evaluate,
    waitFor,
    consoleLog: () => consoleEntries.slice(),
    close: () => socket.close(),
  };
}

function recordEvent(entries, message) {
  if (message.method === "Runtime.consoleAPICalled") {
    const text = (message.params.args ?? [])
      .map((arg) => arg.value ?? arg.description ?? arg.type)
      .join(" ");
    entries.push(`[console.${message.params.type}] ${text}`);
    return;
  }
  if (message.method === "Log.entryAdded") {
    const entry = message.params.entry;
    entries.push(`[log.${entry.level}] ${entry.text}`);
    return;
  }
  if (message.method === "Runtime.exceptionThrown") {
    const details = message.params.exceptionDetails;
    entries.push(`[exception] ${details.exception?.description ?? details.text}`);
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
