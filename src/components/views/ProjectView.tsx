import { formatSlug } from "../../useProjects";
import { PlaceholderView } from "./PlaceholderView";

type Props = {
  slug: string;
};

export function ProjectView({ slug }: Props) {
  return (
    <PlaceholderView
      title={formatSlug(slug)}
      caption="This project's notes and meetings will be browsable here."
      detail="Arrives with #46"
    />
  );
}
