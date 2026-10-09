//! The workspace-wide failure classes: every adapter maps its failures
//! onto these via its own `class()`; `Regulatory` is constructed by
//! exactly one adapter (the fiscal response classifier).

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::AsRefStr)]
#[strum(serialize_all = "kebab-case")]
pub enum ErrorClass {
    Transient,
    OperatorAction,
    /// Authority rejection/flag: never auto-retried; subsanación is a
    /// NEW record (Veri*FACTU semantics).
    Regulatory,
    /// The caller's own draft/context is off-contract: fix the input,
    /// never a retry, never an operator ticket.
    InvalidInput,
    Bug,
}

impl ErrorClass {
    /// The storage vocabulary spelling (`last_error_class`,
    /// `operator_exception.error_class`).
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.as_ref()
    }
}
