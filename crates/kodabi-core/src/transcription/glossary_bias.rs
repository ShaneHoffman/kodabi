//! Bridges [`crate::glossary`] into [`TranscriptionEngine::set_bias`].

use std::path::Path;

use crate::glossary::{Glossary, GlossaryError};

use super::{TranscriptionEngine, TranscriptionError};

/// Failure applying a project's glossary as transcription bias.
#[derive(Debug, thiserror::Error)]
pub enum GlossaryBiasError {
    #[error(transparent)]
    Glossary(#[from] GlossaryError),
    #[error(transparent)]
    Engine(#[from] TranscriptionError),
}

/// Canonical proper-noun spellings to bias an engine toward.
///
/// Only the canonical `term` of each entry — definitions are prose (noise in
/// a prompt) and aliases are the *non-canonical* spellings we deliberately do
/// not bias toward: alias normalization is the post-pass's job, so keeping
/// them out of the prompt preserves the two-layer split (bias = canonical
/// spelling, post-pass = alias fix-up).
pub fn glossary_bias_terms(glossary: &Glossary) -> Vec<String> {
    glossary.terms().iter().map(|t| t.term.clone()).collect()
}

/// Loads `project_dir`'s glossary and biases `engine` toward its proper nouns.
///
/// A project with no glossary file yields an empty glossary
/// ([`Glossary::load`]), so `engine` is biased with an empty term list and
/// transcription proceeds unbiased. A malformed or duplicate-term glossary
/// surfaces as [`GlossaryBiasError::Glossary`] instead of silently degrading,
/// since that's a user-fixable mistake worth surfacing. Engines without a
/// bias mechanism (Parakeet, [`super::MockEngine`]) ignore the terms via the
/// trait's no-op default.
pub fn apply_glossary_bias(
    engine: &mut dyn TranscriptionEngine,
    project_dir: &Path,
) -> Result<(), GlossaryBiasError> {
    let glossary = Glossary::load(project_dir)?;
    let terms = glossary_bias_terms(&glossary);
    engine.set_bias(&terms)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glossary::{GlossaryTerm, OnConflict};
    use crate::transcription::{AudioChunk, Result, Segment};
    use tempfile::tempdir;

    /// Records the terms handed to `set_bias`, unlike `MockEngine` which
    /// ignores them via the trait default.
    #[derive(Default)]
    struct SpyEngine {
        biased_terms: Option<Vec<String>>,
    }

    impl TranscriptionEngine for SpyEngine {
        fn accept(&mut self, _chunk: AudioChunk<'_>) -> Result<Vec<Segment>> {
            Ok(Vec::new())
        }

        fn finish(&mut self) -> Result<Vec<Segment>> {
            Ok(Vec::new())
        }

        fn set_bias(&mut self, terms: &[String]) -> Result<()> {
            self.biased_terms = Some(terms.to_vec());
            Ok(())
        }
    }

    fn term(term: &str, definition: &str, aliases: &[&str]) -> GlossaryTerm {
        GlossaryTerm {
            term: term.to_string(),
            definition: definition.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn glossary_bias_terms_keeps_only_canonical_terms() {
        let mut glossary = Glossary::default();
        glossary
            .upsert(
                term(
                    "MERIDIAN",
                    "A regional systems-migration project.",
                    &["meridian"],
                ),
                OnConflict::Error,
            )
            .unwrap();
        glossary
            .upsert(
                term("TeeTrack", "Tee-sheet / POS vendor.", &[]),
                OnConflict::Error,
            )
            .unwrap();

        let terms = glossary_bias_terms(&glossary);

        assert_eq!(terms, vec!["MERIDIAN".to_owned(), "TeeTrack".to_owned()]);
    }

    #[test]
    fn apply_glossary_bias_biases_engine_with_project_terms() {
        let dir = tempdir().unwrap();
        let mut glossary = Glossary::default();
        glossary
            .upsert(
                term("MERIDIAN", "A regional systems-migration project.", &[]),
                OnConflict::Error,
            )
            .unwrap();
        glossary.save(dir.path()).unwrap();

        let mut engine = SpyEngine::default();
        apply_glossary_bias(&mut engine, dir.path()).expect("bias applies");

        assert_eq!(engine.biased_terms, Some(vec!["MERIDIAN".to_owned()]));
    }

    #[test]
    fn apply_glossary_bias_degrades_cleanly_with_no_glossary_file() {
        let dir = tempdir().unwrap();

        let mut engine = SpyEngine::default();
        apply_glossary_bias(&mut engine, dir.path()).expect("bias applies");

        assert_eq!(engine.biased_terms, Some(Vec::new()));
    }

    #[test]
    fn apply_glossary_bias_surfaces_malformed_glossary() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("_glossary.yml"), "not: [valid, glossary").unwrap();

        let mut engine = SpyEngine::default();
        let err = apply_glossary_bias(&mut engine, dir.path()).unwrap_err();

        assert!(matches!(err, GlossaryBiasError::Glossary(_)));
    }
}
