import { useRef, useState } from "react";
import { DAY_CLASS } from "../../theme";
import { HC_CLASS } from "../../contrast";
import { SpiritMark } from "../capture/SpiritMark";
import { ListenPill } from "../shell/ListenPill";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Field } from "../ui/Field";
import { Menu } from "../ui/Menu";
import { Select } from "../ui/Select";
import { Switch } from "../ui/Switch";

/*
 * The Grove primitives, all of them, on one page — the acceptance harness for
 * the primitives ticket and the reference for every screen ticket after it.
 *
 * It is a DEV PAGE, not a screen: `gallery.html` is deliberately absent from
 * the Vite build inputs, so Vite serves it at /gallery.html while `pnpm dev` is
 * running and the packaged app never contains it. It exists because the things
 * this ticket builds are things you have to LOOK at — a blur, a lit edge, a
 * scrim light enough to blur through — across night, day, high contrast and
 * reduced transparency, which no assertion can check.
 *
 * The toggles write the same root classes and attribute the real app writes
 * (src/theme.ts, src/contrast.ts, src/reduceMotion.ts), so what you see here is
 * what the app does, not an imitation of it. They are imperative handlers, not
 * effects: this is a user action changing the document, which is exactly the
 * case .claude/rules/no-use-effect.md says belongs in the handler.
 */

/** A labelled block, so the page reads as a catalogue rather than a pile. */
function Section({ title, note, children }: { title: string; note?: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-3">
      <div className="flex items-baseline gap-3">
        <h2 className="font-data text-[10px] uppercase tracking-[0.22em] text-ink-faint">{title}</h2>
        {note && <p className="text-[11.5px] text-ink-faint">{note}</p>}
      </div>
      {children}
    </section>
  );
}

/** A switch on the row it is built for: name left, control flush right, a
 * hairline between siblings. The visible label and the accessible name are the
 * same string on purpose — what you read is what you can say to a voice-control
 * tool (see Switch.tsx). */
function SwitchRow({
  label,
  checked,
  onChange,
  busy = false,
}: {
  label: string;
  checked: boolean;
  onChange: (next: boolean) => void;
  busy?: boolean;
}) {
  return (
    <div className="flex items-center justify-between gap-5 border-t border-edge py-3 first:border-t-0">
      <span className="text-[13.5px] font-semibold text-ink">{label}</span>
      <Switch label={label} checked={checked} onChange={onChange} busy={busy} />
    </div>
  );
}

/** The root-class toggles, mirroring the three display preferences. */
function DisplayToggles() {
  const root = document.documentElement;
  const [day, setDay] = useState(root.classList.contains(DAY_CLASS));
  const [contrast, setContrast] = useState(root.classList.contains(HC_CLASS));
  const [stillness, setStillness] = useState(root.getAttribute("data-reduce-motion") === "on");

  return (
    <div className="flex items-center gap-2">
      <Button
        variant={day ? "action" : "quiet"}
        aria-pressed={day}
        onClick={() => {
          root.classList.toggle(DAY_CLASS);
          setDay(root.classList.contains(DAY_CLASS));
        }}
      >
        Day
      </Button>
      <Button
        variant={contrast ? "action" : "quiet"}
        aria-pressed={contrast}
        onClick={() => {
          root.classList.toggle(HC_CLASS);
          setContrast(root.classList.contains(HC_CLASS));
        }}
      >
        More contrast
      </Button>
      <Button
        variant={stillness ? "action" : "quiet"}
        aria-pressed={stillness}
        onClick={() => {
          const next = root.getAttribute("data-reduce-motion") !== "on";
          if (next) root.setAttribute("data-reduce-motion", "on");
          else root.removeAttribute("data-reduce-motion");
          setStillness(next);
        }}
      >
        Reduce motion
      </Button>
    </div>
  );
}

export function PrimitiveGallery() {
  const [confirming, setConfirming] = useState(false);
  const cancelRef = useRef<HTMLButtonElement>(null);
  // Controlled, unlike the fields above: a switch's whole appearance IS its
  // value, so an uncontrolled one would show a single frozen state and hide
  // the only thing worth looking at.
  const [reduceMotion, setReduceMotion] = useState(false);
  const [increaseContrast, setIncreaseContrast] = useState(true);
  const [retention, setRetention] = useState("keep_days");

  return (
    <div className="grove-ground min-h-dvh font-ui text-ink">
      <header className="glass-top flex items-center gap-6 px-6 py-3.5">
        <h1 className="text-[15px] font-semibold tracking-[0.02em]">Grove primitives</h1>
        <p className="text-[12.5px] text-ink-dim">
          Every primitive, in every variant. Toggle the grounds and check the material.
        </p>
        <div className="ml-auto">
          <DisplayToggles />
        </div>
      </header>

      <div className="flex gap-5 p-5">
        <nav className="glass-dock flex w-56 flex-none flex-col gap-1 p-3" aria-label="Sections">
          {[
            "Buttons",
            "Fields",
            "Switches",
            "Glass",
            "Overlays",
            "Kodama",
            "Scrollbars",
            "Focus",
          ].map((name) => (
            <a
              key={name}
              href={`#${name.toLowerCase()}`}
              className="focus-ring-inset rounded-button px-2.5 py-2 text-[13.5px] text-ink-dim hover:bg-wash hover:text-ink"
            >
              {name}
            </a>
          ))}
        </nav>

        <main className="glass-panel flex min-w-0 flex-1 flex-col gap-10 p-9">
          <Section
            title="Buttons"
            note="Rectangles act, pills represent. One press spec: scale 97 over 140ms."
          >
            <div className="flex flex-wrap items-center gap-2.5" id="buttons">
              <Button>File it</Button>
              <Button variant="danger">Delete note</Button>
              <Button variant="quiet">Dismiss</Button>
              <Button variant="pill">transcript.md</Button>
            </div>
            {/* `chip` is the only variant with a width to show: it exists
                because a pill's ends leave ragged gutters in the note's 272px
                rail, so it is shown in a box that width rather than inline
                beside the others, with the label/value split it was made for. */}
            <div className="flex w-[272px] flex-col gap-2">
              <Button variant="chip">
                <span className="flex w-full items-center justify-between gap-2">
                  <span>
                    <span aria-hidden="true">▶ </span>Audio
                  </span>{" "}
                  <span className="font-data text-[10.5px] font-normal tabular-nums text-ink-faint">
                    1:06
                  </span>
                </span>
              </Button>
            </div>
            <div className="flex flex-wrap items-center gap-2.5">
              <Button loading loadingLabel="Filing…">
                File it
              </Button>
              <Button variant="danger" loading loadingLabel="Deleting…">
                Delete note
              </Button>
              <Button variant="quiet" disabled>
                Dismiss
              </Button>
              <Button variant="pill" disabled>
                transcript.md
              </Button>
              <Button variant="chip" disabled>
                Audio
              </Button>
            </div>
          </Section>

          <Section
            title="Fields"
            note="The same glass a button is, at input size. Focus moves the border; the caret is the kodama's. The select is that glass as a value button."
          >
            {/* Uncontrolled on purpose: a catalogue page has no business
                holding the value of every control it displays. */}
            <div className="flex max-w-sm flex-col gap-5" id="fields">
              <Field
                label="Project name"
                placeholder="project-name"
                hint={
                  <>
                    Nested paths like <span className="font-data">household/garage</span>{" "}
                    work too.
                  </>
                }
              />
              <Field
                label="Project name"
                defaultValue="inbox"
                error="A project cannot be called inbox."
              />
              <Field label="Days to keep" type="number" min={1} defaultValue={30} />
              {/* Controlled, like the switches below: the trigger's whole job
                  is to read back the chosen value. Open it to check that the
                  list wears the same material the menu above does, that it
                  anchors to this trigger rather than to the last one on the
                  page, and that the trigger does not scale while it is an
                  anchor (the press would drag the list sideways). */}
              <Select
                label="Retention"
                value={retention}
                onChange={setRetention}
                options={[
                  { value: "keep_all", label: "Keep all transcripts and recordings" },
                  { value: "keep_days", label: "Keep for a number of days" },
                  { value: "discard_after_distill", label: "Discard after distilling" },
                ]}
              />
              <Select
                label="Retention"
                value={retention}
                onChange={setRetention}
                options={[]}
                busy
              />
            </div>
          </Section>

          <Section
            title="Switches"
            note="The knob carries the state: 16px on a 38x22 track, 18px of travel over 180ms. Toggle Reduce motion — it still arrives, at once."
          >
            {/* Shown on the row they live on, because a switch's whole job is
                to sit at the right edge of a settings row and be scanned down
                a column with its siblings. */}
            <div className="glass-card flex max-w-md flex-col px-5" id="switches">
              <SwitchRow
                label="Reduce motion"
                checked={reduceMotion}
                onChange={setReduceMotion}
              />
              <SwitchRow
                label="Increase contrast"
                checked={increaseContrast}
                onChange={setIncreaseContrast}
              />
              {/* Busy, the only inert form: still focusable, still in the tab
                  order, and it declines its own press. Tab to it and try. */}
              <SwitchRow label="Pill for captures you start" checked busy onChange={() => {}} />
            </div>
          </Section>

          <Section
            title="Glass"
            note="Card carries no backdrop blur — glass on glass is mud, so depth is the lit edge and the shadow."
          >
            <div className="grid grid-cols-2 gap-4" id="glass">
              <div className="glass-card p-4">
                <p className="text-[15.5px] font-semibold">A card</p>
                <p className="mt-1.5 text-[13px] text-ink-dim">
                  Near-solid, inside a panel that is already blurring the ground.
                </p>
              </div>
              <div className="glass-overlay p-4">
                <p className="text-[15.5px] font-semibold">An overlay</p>
                <p className="mt-1.5 text-[13px] text-ink-dim">
                  Menus and toasts: the deepest blur, the strongest shadow.
                </p>
              </div>
              <div className="glass-dialog p-4">
                <p className="text-[15.5px] font-semibold">A dialog</p>
                <p className="mt-1.5 text-[13px] text-ink-dim">
                  The overlay material at the dialog radius, with the longest shadow.
                </p>
              </div>
              <div className="glass-scrim rounded-card border border-edge p-4">
                <p className="text-[15.5px] font-semibold">The scrim</p>
                <p className="mt-1.5 text-[13px] text-ink-dim">
                  A veil, not a blackout: the app has to stay visible enough to blur.
                </p>
              </div>
              {/* The two that float over the desktop rather than over the app.
                  They read wrong here on purpose: behind them is a panel, not
                  someone's wallpaper, which is the whole reason their fill and
                  border are literals instead of ground-tuned tokens. */}
              <div className="glass-pill flex items-center px-5 py-3">
                <p className="text-[15.5px] font-semibold">The capture pill</p>
              </div>
              <div className="glass-sheet p-4">
                <p className="text-[15.5px] font-semibold">The capture sheet</p>
                <p className="mt-1.5 text-[13px] text-ink-dim">
                  Quick capture: panel-round, dialog-deep, over the desktop.
                </p>
              </div>
            </div>
          </Section>

          <Section
            title="Overlays"
            note="Menus materialize from the corner they hang off; a dialog from its centre. Exits are faster."
          >
            <div className="flex items-center gap-2.5" id="overlays">
              <Menu.Root>
                <Menu.Trigger render={<Button>File to…</Button>} />
                <Menu.Content>
                  <Menu.Item>Briarwood Golf</Menu.Item>
                  <Menu.Item>Riverbend Deck</Menu.Item>
                  <Menu.Item>Little League</Menu.Item>
                  <Menu.Separator />
                  <Menu.Item>New project…</Menu.Item>
                </Menu.Content>
              </Menu.Root>

              <Button variant="quiet" aria-haspopup="dialog" onClick={() => setConfirming(true)}>
                Open a dialog
              </Button>
            </div>

            {/* Driven by `open` rather than by mounting, so the EXIT animation
                plays — the one thing a conditionally-mounted dialog cannot
                show you. That is also why this is the raw `Dialog` and not
                `DestructiveConfirmDialog`, which is mounted-is-open: the body
                below is a hand-copy of that component's structure, and the two
                have to be kept in step by hand. The subject is deliberately
                long enough to truncate. */}
            <Dialog
              open={confirming}
              onDismiss={() => setConfirming(false)}
              label="Delete this note?"
              initialFocus={cancelRef}
              className="flex flex-col gap-4"
            >
              <h2 className="text-[15px] font-semibold text-ink">Delete this note?</h2>
              <p
                title="Sprinkler quotes, the 9th green rebuild, and what Miguel wants doing before the fall tournament"
                className="overflow-hidden text-ellipsis whitespace-nowrap rounded-button border border-edge bg-wash px-3.5 py-2.5 text-[13.5px] font-semibold text-ink shadow-[inset_0_1px_0_var(--color-edge-lit)]"
              >
                Sprinkler quotes, the 9th green rebuild, and what Miguel wants doing before
                the fall tournament
              </p>
              <p className="text-[13px] leading-relaxed text-ink-dim">
                Its recording and transcript are deleted with it.
              </p>
              <p className="text-[13px] leading-relaxed text-danger">This cannot be undone.</p>
              <div className="flex items-center justify-end gap-2.5">
                <Button ref={cancelRef} variant="quiet" onClick={() => setConfirming(false)}>
                  Cancel
                </Button>
                <Button variant="danger" onClick={() => setConfirming(false)}>
                  Delete note
                </Button>
              </div>
            </Dialog>
          </Section>

          <Section
            title="Kodama"
            note="The one green: audio is reaching disk. Toggle Reduce motion — the lobes settle round and the aura keeps breathing."
          >
            {/* The marks at gallery size. On the page they sit at 14px, where
                the lobes are a suggestion rather than a shape; large is the
                only way to see whether the aura is undulating or throbbing. */}
            <div className="flex flex-wrap items-center gap-9" id="kodama">
              {(["idle", "starting", "listening", "degraded", "reconnecting"] as const).map(
                (mode) => (
                  <div key={mode} className="flex flex-col items-center gap-3">
                    <SpiritMark mode={mode} size="1.5rem" halo="1.4rem" />
                    <span className="font-data text-[10px] tracking-[0.14em] text-ink-faint uppercase">
                      {mode}
                    </span>
                  </div>
                ),
              )}
              <div className="flex flex-col items-center gap-3">
                <SpiritMark mode="listening" variant="ring" size="1.5rem" />
                <span className="font-data text-[10px] tracking-[0.14em] text-ink-faint uppercase">
                  ring
                </span>
              </div>
            </div>

            <div className="mt-2 flex flex-wrap items-center gap-2.5">
              <ListenPill mode="listening" label="Listening" elapsedSeconds={154} />
              <ListenPill mode="degraded" label="Mic only" elapsedSeconds={154} />
              <ListenPill mode="starting" label="Starting" elapsedSeconds={154} />
              <ListenPill mode="reconnecting" label="Reconnecting" elapsedSeconds={154} />
              <ListenPill mode="idle" label="Not listening" />
            </div>
          </Section>

          <Section title="Scrollbars" note="Transparent track, pill thumb inset 3px from it.">
            <div className="glass-card h-40 overflow-y-auto p-4" id="scrollbars">
              {Array.from({ length: 14 }, (_, index) => (
                <p key={index} className="py-1.5 text-[13px] text-ink-dim">
                  Row {index + 1} — scroll this box to see the thumb.
                </p>
              ))}
            </div>
          </Section>

          <Section
            title="Focus"
            note="Tab through. Outward on a control, inward where the control fills its container."
          >
            <div className="flex items-center gap-2.5" id="focus">
              <Button variant="quiet">Outward ring</Button>
              <div className="glass-card w-64 overflow-hidden p-1.5">
                <button
                  type="button"
                  className="focus-ring-inset flex w-full items-center rounded-[9px] px-2.5 py-2 text-[13px] text-ink-dim hover:bg-wash hover:text-ink"
                >
                  Inward ring, on a full-bleed row
                </button>
              </div>
            </div>
          </Section>
        </main>
      </div>
    </div>
  );
}
