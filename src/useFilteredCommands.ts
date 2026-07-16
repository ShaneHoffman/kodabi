import { useMemo } from "react";
import type { Command } from "./useCommands";

/**
 * Case-insensitive multi-term filter: every whitespace-separated term must
 * appear somewhere in the command's title or hint. Pure, so it stays
 * unit-testable independent of the palette.
 */
export function filterCommands(commands: Command[], query: string): Command[] {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return commands;
  return commands.filter((command) => {
    const haystack = `${command.title} ${command.hint ?? ""}`.toLowerCase();
    return terms.every((term) => haystack.includes(term));
  });
}

export function useFilteredCommands(commands: Command[], query: string): Command[] {
  return useMemo(() => filterCommands(commands, query), [commands, query]);
}
