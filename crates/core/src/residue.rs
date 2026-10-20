//! Volume / format / edition tokenizer. Vocabulary is §6.4, derived from
//! measured data — do not trim it.

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Volume(u32),
    Format(String),
    Edition(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Residue {
    pub tokens: Vec<Token>,
    /// Whatever the tokenizer could not claim — a subtitle, usually (D15).
    pub leftover: String,
}

pub fn eat(s: &str) -> Residue {
    todo!()
}

/// Removes distributor cruft such as `_optimized_1420484117`.
pub fn strip_cruft(s: &str) -> String {
    todo!()
}
