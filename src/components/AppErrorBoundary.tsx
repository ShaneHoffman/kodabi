import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "./ui/Button";
import { ViewFrame } from "./ui/ViewFrame";
import { StatusMessage } from "./ui/StatusMessage";

type Props = {
  /** Remounts the boundary when it changes, so navigating away from a view
   * that threw clears the fallback instead of stranding the user on it. */
  resetKey: string;
  children: ReactNode;
};

type State = { message: string | null };

/**
 * The last line of defence around the routed view.
 *
 * React unmounts the whole tree when a render throws, so before this the one
 * thing a user saw for a bug anywhere in a view was an empty window: no
 * sidebar, no message, nothing to click. That is the worst possible failure
 * for an app whose whole promise is that it does not silently lose your work.
 *
 * It is a class because error boundaries have no hook form, which also means
 * it sits outside the no-useEffect rule rather than needing an exception to it.
 * Scoped to the main region on purpose: the sidebar keeps working, so the user
 * can navigate out of the broken view under their own power.
 */
export class AppErrorBoundary extends Component<Props, State> {
  state: State = { message: null };

  static getDerivedStateFromError(error: unknown): State {
    return { message: error instanceof Error ? error.message : String(error) };
  }

  componentDidUpdate(previous: Props) {
    // Navigating is the user's way out. Without this the fallback would
    // outlive the view that caused it.
    if (previous.resetKey !== this.props.resetKey && this.state.message !== null) {
      this.setState({ message: null });
    }
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    // Nothing collects this yet, so the console is the only record there is.
    console.error("View crashed:", error, info.componentStack);
  }

  render() {
    if (this.state.message === null) return this.props.children;
    return (
      <ViewFrame variant="health" eyebrow="System" title="This screen stopped">
        <div className="flex flex-col items-start gap-sm">
          <StatusMessage variant="error">
            This screen stopped while drawing itself. Your notes are files on
            disk and were not touched.
          </StatusMessage>
          {/* A way out that is not "go somewhere else". Clearing the message
              remounts the view, which is all a transient render failure needs,
              and it costs nothing when the failure is not transient — the
              boundary simply catches it again. Before this the only recovery
              was navigating away, i.e. the user had to know that `resetKey`
              existed. */}
          <Button onClick={() => this.setState({ message: null })}>
            Try this screen again
          </Button>
          <p className="text-body text-text-soft">
            If it keeps happening, pick another screen in the sidebar. Closing
            and reopening Kodabi is safe.
          </p>
          {/* --text-soft, and prefixed. This is the raw exception, and it is
              the one string on this screen a user might have to read back to
              somebody: it used to render bare (docs/DESIGN_SYSTEM.md §3 —
              "never leak an exception") at --text-faint, which §6 records as
              failing the 4.5:1 floor. Mono because it is machine text. */}
          <p className="font-mono text-cap text-text-soft">
            Reported by the app: {this.state.message}
          </p>
        </div>
      </ViewFrame>
    );
  }
}
