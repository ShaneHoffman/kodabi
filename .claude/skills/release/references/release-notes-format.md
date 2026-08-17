# Release notes format

The draft that `release.yml` creates carries a placeholder title (`Kodabi v<version>`) and
auto-generated notes (a PR list). Both are replaced before hand-off so every release reads the
same way. The models for everything here are the published releases — check the two most recent
with `gh release view` before writing, and prefer their current shape wherever this file and
reality have drifted.

## Title

The bare version, nothing else: `0.2.0`. No `v` prefix, no product name — the release page
already sits under the repo.

## Body skeleton

```markdown
<One- or two-sentence plain-language summary of the release's theme.>

**Status: pre-alpha.** Interfaces are still moving and rough edges are expected. See the
[roadmap](https://github.com/ShaneHoffman/kodabi/blob/main/docs/ROADMAP.md) for where it's headed.

## Highlights

* **<Feature, bold, ending in a period.>** <One to three sentences on what it does for the
  user.>
* …

## Polish

* <Smaller user-visible improvements and fixes, one or two lines each, no bold lead-in.>

## Installing and updating

* **Already on <previous>?** Kodabi checks for updates at startup and will offer <version>.
  Nothing downloads or installs without your click.
* **New install:** `Kodabi_<version>_x64-setup.exe` is a per-user NSIS installer for Windows
  x64; no administrator rights needed. First run downloads the models (about 796 MB) only when
  you ask, and the [`claude` CLI](https://docs.claude.com/en/docs/claude-code/overview) remains
  the prerequisite for the LLM features (distill, glossary cleanup, chat, terminal). Recording,
  transcription, and search work without it.
* **SmartScreen may still warn** while the signing certificate's reputation accrues. Choose
  *More info* → *Run anyway*; the installer's Properties → Digital Signatures tab shows the
  publisher.

**Full Changelog**: https://github.com/ShaneHoffman/kodabi/compare/v<previous>...v<version>
```

## Writing rules

- **The audience is users, not contributors.** Translate each PR into what changed for someone
  using the app; never paste commit subjects or PR titles. Internal-only work (CI, refactors,
  docs, tests, tooling) is omitted, not listed.
- **Highlights are features; Polish is everything smaller.** Drop the Polish section when there
  are too few small items to carry it, and a more fitting heading than "Polish" is fine when
  the items suggest one.
- **The standing paragraphs are living text, not boilerplate.** Re-read the status paragraph,
  the installing bullets, and the SmartScreen caveat against this release and update what has
  changed: drop pre-alpha when it stops being true, drop the SmartScreen bullet once the
  certificate's reputation has accrued, keep the model download size current.
- **Release notes are user-facing copy**, so [`copy-style`](../../../rules/copy-style.md)
  applies: no em dashes.
