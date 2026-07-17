import { PlaceholderView } from "./PlaceholderView";

type Props = {
  query: string;
};

export function SearchView({ query }: Props) {
  return (
    <PlaceholderView
      title="Search"
      caption={
        query
          ? `Results for “${query}” will appear here, across every project and meeting.`
          : "Search across every project and meeting."
      }
      detail="Arrives with Phase 3"
    />
  );
}
