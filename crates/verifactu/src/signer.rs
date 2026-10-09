//! The `FiscalSigner` port: enveloped `XAdES`-EPES signing under the
//! `Veri*FACTU` AGE policy — RSA-SHA256, C14N 1.0, no timestamps.
//!
//! Port-only by design: isolation and fake-based testing of callers —
//! *not* a fallback in case a real signer is wrong.

#[derive(Debug, thiserror::Error)]
pub enum SignerError {
    /// No usable signing certificate (store locked, smartcard absent,
    /// unparseable key material).
    #[error("signing certificate unavailable")]
    CertificateUnavailable,
    #[error("signing failed: {0}")]
    SigningFailed(String),
}

/// Port for enveloped `XAdES`-EPES signing under the `Veri*FACTU` AGE
/// policy. `Send + Sync` ride the port itself (like the transport
/// port), so `Arc<dyn FiscalSigner>` is the full spelling at the
/// `Custom` door.
pub trait FiscalSigner: Send + Sync {
    /// # Errors
    /// Returns [`SignerError::CertificateUnavailable`] when no usable
    /// signing certificate is available, and
    /// [`SignerError::SigningFailed`] when the signing operation fails.
    fn sign_xades_epes(&self, doc: &str) -> Result<String, SignerError>;
}
