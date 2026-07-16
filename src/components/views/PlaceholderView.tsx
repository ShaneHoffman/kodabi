type Props = {
  title: string;
  caption: string;
  /** Quiet provenance line, e.g. the ticket that fills this seam. */
  detail?: string;
};

/**
 * The shared Ma-forward placeholder every unbuilt destination renders:
 * one thing per view, hierarchy from type and space alone.
 */
export function PlaceholderView({ title, caption, detail }: Props) {
  return (
    <section className="flex min-h-full flex-col items-center justify-center gap-md p-xl text-center">
      <h2 className="font-serif text-h2 text-text">{title}</h2>
      <p className="max-w-measure text-body text-text-soft">{caption}</p>
      {detail && (
        <p className="text-cap uppercase tracking-wide text-text-faint">{detail}</p>
      )}
    </section>
  );
}
