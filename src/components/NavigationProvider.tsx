import { useMemo, useState, type ReactNode } from "react";
import { INITIAL_VIEW, NavigationContext, type View } from "../useNavigation";

type Props = {
  children: ReactNode;
};

export function NavigationProvider({ children }: Props) {
  const [view, setView] = useState<View>(INITIAL_VIEW);
  // setView's identity is stable, so the value changes only with the view.
  const value = useMemo(() => ({ view, navigate: setView }), [view]);

  return <NavigationContext value={value}>{children}</NavigationContext>;
}
