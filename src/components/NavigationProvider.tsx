import { useCallback, useMemo, useReducer, type ReactNode } from "react";
import {
  INITIAL_VIEW,
  NavigationContext,
  navigationReducer,
  type View,
} from "../useNavigation";

type Props = {
  children: ReactNode;
};

export function NavigationProvider({ children }: Props) {
  const [view, dispatch] = useReducer(navigationReducer, INITIAL_VIEW);
  const navigate = useCallback(
    (next: View) => dispatch({ type: "navigate", view: next }),
    [],
  );
  const value = useMemo(() => ({ view, navigate }), [view, navigate]);

  return <NavigationContext value={value}>{children}</NavigationContext>;
}
