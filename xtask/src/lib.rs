//! Grading logic lives in the lib because a bin-only crate cannot be imported
//! by its own tests.

/// Normalises both sides before comparing — Humble writes `&` where the corpus
/// writes "and", prefixes promos with "Free Issue" including a zero-width
/// character, and inserts `Vol 1:` mid-title. A naive comparison overstates the
/// error rate five-fold.
///
/// It must **not** drop leading articles or fold case: the corpus's own
/// `name_key` does, which would silently pass `MASK Omnibus` against *The Mask
/// Omnibus*. That is the gate's whole job.
pub fn grade_ok(answer: &str, truth: &str) -> bool {
    todo!()
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rates {
    pub located: f64,
    pub ready: f64,
    pub silent_false_positive: f64,
}

/// located >= 92.3%, ready >= 66%, silent FP <= 0.48% (D26, D31).
pub fn gates_pass(rates: Rates) -> bool {
    todo!()
}
