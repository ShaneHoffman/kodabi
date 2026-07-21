/*
 * Dev-only preview harness. Stands in for the Tauri IPC boundary so the real
 * React app renders in a plain browser for design review, seeded with the same
 * fixture data the design references use. Never shipped — `preview.html` and
 * `preview-capture.html` are the only pages that load it, and neither is a
 * Vite build entry.
 *
 * Query parameters:
 *   ?theme=light|dark   force a theme (through the settings mock, because
 *                       src/theme.ts applies the stored preference at boot and
 *                       would overwrite an attribute set here)
 *   ?still=1            remove every transition and animation
 *   ?view=<label>       click the sidebar/nav button whose label starts with it
 *   ?open=<label>       then click a second control (a note row)
 *   ?open2=<label>      and a third ("Edit"), for a two-step screen
 *   ?palette=1          open the command palette
 *   ?search=<query>     open the palette, type the query, run the search row
 *   ?recording=1        report a live capture, so the listening states render
 */
(function () {
  const params = new URLSearchParams(location.search);
  const forcedTheme = params.get("theme");
  const recording = params.has("recording");

  const notes = [
    {
      id: "n_a1b2c3", path: "Inbox/kickoff.md", title: "Kickoff with Jane",
      type: "meeting", project: null, date: "2026-07-20",
      tags: ["kickoff", "q3"], source: "meeting", confidence: 0.62,
      snippet: "Scoped the Q3 launch: Jane owns the vendor shortlist, budget review lands Thursday, and the portal copy still needs an owner.",
    },
    {
      id: "n_d4e5f6", path: "Inbox/irrigation.md", title: "Irrigation vendor call",
      type: "meeting", project: null, date: "2026-07-19",
      tags: ["vendors"], source: "meeting", confidence: 0.55,
      snippet: "Two quotes in: Rainworks at 48k with a six-week lead, GreenFlow at 52k but they can install in July.",
    },
    {
      id: "n_g7h8i9", path: "Inbox/portal.md", title: "Member portal onboarding idea",
      type: "note", project: null, date: "2026-07-19",
      tags: [], source: "manual", confidence: null,
      snippet: "Walk new members through tee-time booking on first login instead of dropping them on the settings page.",
    },
    {
      id: "n_j1k2l3", path: "Inbox/facilities.md", title: "Facilities sync",
      type: "meeting", project: null, date: "2026-07-18",
      tags: ["facilities"], source: "meeting", confidence: 0.41,
      snippet: "Cart barn roof repair slips to August; the range netting quote came in under budget.",
    },
  ];

  const projectNotes = [
    { id: "n_p1", path: "paradise-golf/budget.md", title: "Budget review with owners", type: "meeting", project: "paradise-golf", date: "2026-07-19", tags: ["budget"], source: "meeting", confidence: null, snippet: "Owners walked the Q3 budget; the clubhouse line is the swing item this quarter." },
    { id: "n_p2", path: "paradise-golf/renovation.md", title: "Course renovation standup", type: "meeting", project: "paradise-golf", date: "2026-07-18", tags: ["renovation"], source: "meeting", confidence: null, snippet: "Fairway regrading starts on the back nine." },
    { id: "n_p3", path: "paradise-golf/clubhouse.md", title: "Clubhouse opening punch list", type: "note", project: "paradise-golf", date: "2026-07-17", tags: ["clubhouse"], source: "manual", confidence: null, snippet: "Signage, furniture delivery, and the final inspection." },
    { id: "n_p4", path: "paradise-golf/financing.md", title: "Owner call: financing options", type: "meeting", project: "paradise-golf", date: "2026-07-16", tags: ["finance"], source: "meeting", confidence: null, snippet: "A refinance would free budget for the range netting and cart-barn roof." },
    { id: "n_p5", path: "paradise-golf/grounds.md", title: "Groundskeeping schedule notes", type: "note", project: "paradise-golf", date: "2026-07-15", tags: [], source: "manual", confidence: null, snippet: "Mowing cadence through August." },
  ];

  const body = [
    "## Summary",
    "",
    "Kickoff for the Q3 member-experience push. Jane walked through the scope, the two open budget questions, and who owns what through August.",
    "",
    "## Decisions",
    "",
    "- The vendor shortlist closes Friday; Jane owns the final three.",
    "- Portal copy review moves to the Thursday budget slot.",
    "",
    "## Action items",
    "",
    "- [ ] Jane: circulate the vendor shortlist by Friday",
    "- [ ] Sam: draft portal onboarding copy for review",
    "",
  ].join("\n");

  const project = (slug, name, parent, count) => ({
    id: slug, slug, display_name: name, parent, note_count: count,
    meeting_count: count, last_activity: "2026-07-20T09:00:00Z",
  });

  const capturePhase = recording
    ? { phase: "listening", sources: { loopback: "live", microphone: "live" } }
    : { phase: "idle", sources: { loopback: "off", microphone: "off" } };

  const responses = {
    list_projects: () => ({
      inbox_note_count: notes.length,
      projects: [
        project("paradise-golf", "Paradise Golf", null, 14),
        project("paradise-golf/pro-shop", "Pro Shop", "paradise-golf", 4),
        project("atlas-crm", "Atlas CRM", null, 9),
        project("team-weekly", "Team Weekly", null, 21),
      ],
    }),
    list_notes: (args) => (args && args.project === "Inbox" ? notes : projectNotes),
    read_note: (args) => {
      const all = [...notes, ...projectNotes];
      const found = all.find((note) => note.id === (args && args.id)) ?? notes[0];
      return { ...found, body_markdown: body };
    },
    capture_phase: () => capturePhase,
    list_failed_sessions: () => [
      { path: "sessions/s_1.jsonl", slug: "team-weekly", captured_at: "2026-07-19T16:42:00Z" },
      { path: "sessions/s_2.jsonl", slug: null, captured_at: "2026-07-18T08:05:00Z" },
    ],
    get_settings: () => ({
      consent_acknowledged: true,
      retention: { policy: "keep_days", days: 30 },
      overlay: { manual_captures: true, auto_captures: false },
      appearance: { theme: forcedTheme || "system" },
    }),
  };

  let nextCallback = 1;
  window.__TAURI_INTERNALS__ = {
    transformCallback(callback) {
      const id = nextCallback++;
      window[`_tauri_cb_${id}`] = callback;
      return id;
    },
    invoke(cmd, args) {
      const handler = responses[cmd];
      if (handler) return Promise.resolve(handler(args));
      // Event-plugin calls and anything unmocked resolve benignly, so a
      // missing mock never breaks the render.
      return Promise.resolve(cmd.startsWith("plugin:event|") ? 0 : null);
    },
  };

  if (forcedTheme) document.documentElement.setAttribute("data-theme", forcedTheme);

  // Headless screenshots run on a virtual clock that fast-forwards timers but
  // never advances the CSS animation clock, so a plane that transitions over
  // 150ms is frozen at frame zero — the outgoing row still lit, the incoming
  // one not yet. Shortening the duration does not help (a transition that
  // never ticks never finishes, however short); only removing them lets the
  // settled state be photographed.
  if (params.has("still")) {
    const style = document.createElement("style");
    style.textContent =
      "*,*::before,*::after{transition:none!important;animation:none!important}";
    document.head.appendChild(style);
  }

  // Drive the app to a screen by clicking the same controls a person would, so
  // what gets captured is a state the user could actually have navigated to.
  const clickByText = (text) => {
    // Buttons and listbox options: the palette's rows are <li role="option">,
    // not buttons, because focus stays on the combobox input.
    for (const node of document.querySelectorAll('button, [role="option"]')) {
      if (node.textContent.trim().startsWith(text)) {
        node.click();
        return true;
      }
    }
    return false;
  };

  const setInput = (input, value) => {
    // React tracks the DOM value to skip redundant renders, so a plain
    // assignment is ignored. Go through the native setter it patched over.
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      "value",
    ).set;
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  };

  const script = [];
  const view = params.get("view");
  const open = params.get("open");
  const search = params.get("search");
  if (view) script.push(() => clickByText(view));
  if (open) script.push(() => clickByText(open));
  const open2 = params.get("open2");
  if (open2) script.push(() => clickByText(open2));
  if (params.has("palette") || search) {
    script.push(() =>
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }),
      ),
    );
  }
  if (search) {
    script.push(() => {
      const input = document.querySelector(String.raw`input[role="combobox"]`);
      if (input) setInput(input, search);
    });
    script.push(() => clickByText(`Search for`));
  }

  if (script.length > 0) {
    window.addEventListener("load", () => {
      script.forEach((step, index) => setTimeout(step, 400 + index * 300));
    });
  }
})();
