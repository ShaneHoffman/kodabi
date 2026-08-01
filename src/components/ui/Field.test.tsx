import { useRef, useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { Field } from "./Field";

/*
 * What a caller is promised beyond "it renders an input": the label names the
 * control, the hint and the error are both ANNOUNCED rather than merely
 * visible, `aria-invalid` travels with the error message, and the ref reaches
 * the input itself — which is what `Dialog`'s `initialFocus` needs to open a
 * form dialog on its field.
 */
function ControlledField(props: Partial<Parameters<typeof Field>[0]>) {
  const [value, setValue] = useState("");
  return (
    <Field
      label="Project name"
      value={value}
      onChange={(event) => setValue(event.target.value)}
      {...props}
    />
  );
}

describe("Field", () => {
  it("renders a labelled textbox that accepts typing", async () => {
    const user = userEvent.setup();
    render(<ControlledField />);

    const input = screen.getByLabelText("Project name");
    await user.type(input, "briarwood-golf");

    expect(input).toHaveValue("briarwood-golf");
  });

  it("announces the hint through aria-describedby", () => {
    render(<ControlledField hint="Nested paths work too." />);

    const input = screen.getByLabelText("Project name");
    const hint = screen.getByText("Nested paths work too.");

    expect(input).toHaveAttribute("aria-describedby", hint.id);
    expect(hint.id).toBeTruthy();
  });

  it("marks the field invalid and announces the error before the hint", () => {
    render(<ControlledField error="That name is taken." hint="Nested paths work too." />);

    const input = screen.getByLabelText("Project name");
    const alert = screen.getByRole("alert");

    expect(alert).toHaveTextContent("That name is taken.");
    expect(input).toHaveAttribute("aria-invalid", "true");
    // Error first: it is heard before the hint the value just contradicted.
    const [errorId, hintId] = (input.getAttribute("aria-describedby") ?? "").split(" ");
    expect(screen.getByText("That name is taken.").id).toBe(errorId);
    expect(screen.getByText("Nested paths work too.").id).toBe(hintId);
  });

  it("is neither invalid nor announcing when there is no error", () => {
    render(<ControlledField />);

    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Project name")).not.toHaveAttribute("aria-invalid");
  });

  it("forwards its ref to the input, so a dialog can open focused on it", () => {
    function RefHarness() {
      const fieldRef = useRef<HTMLInputElement>(null);
      return (
        <>
          <ControlledField ref={fieldRef} />
          <button type="button" onClick={() => fieldRef.current?.focus()}>
            Focus it
          </button>
        </>
      );
    }
    render(<RefHarness />);

    screen.getByRole("button", { name: "Focus it" }).click();

    expect(screen.getByLabelText("Project name")).toHaveFocus();
  });

  it("passes the input type through, for the number field the consent nudge needs", () => {
    render(<ControlledField label="Days to keep" type="number" min={1} />);

    const input = screen.getByRole("spinbutton", { name: "Days to keep" });
    expect(input).toHaveAttribute("min", "1");
  });

  it("stays focusable while read-only, which is how a field goes inert in flight", async () => {
    const user = userEvent.setup();
    render(<ControlledField readOnly />);

    const input = screen.getByLabelText("Project name");
    await user.click(input);

    expect(input).toHaveFocus();
    await user.type(input, "nope");
    expect(input).toHaveValue("");
  });
});
