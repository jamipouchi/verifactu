//! The `FiscalSigner` port: enveloped `XAdES`-EPES signing under the
//! `Veri*FACTU` AGE policy — RSA-SHA256, C14N 1.0, no timestamps. The
//! signature travels INSIDE each record, never on the envelope; the
//! modality's signing policy (who signs what) lives in
//! [`crate::fiscal::VerifactuEmitter`].

#[derive(Debug, thiserror::Error)]
pub enum SignerError {
    /// No usable signing certificate (store locked, smartcard absent,
    /// unparseable key material).
    #[error("signing certificate unavailable")]
    CertificateUnavailable,
    #[error("signing failed: {0}")]
    SigningFailed(String),
}

/// The one signing port. Implementations: `XadesSigner` (feature
/// `signing`), `FakeSigner` for hermetic tests (feature `fakes`), or the consumer's own
/// over a KMS/HSM.
pub trait FiscalSigner: Send + Sync {
    /// Returns `doc` with its `ds:Signature` as the LAST CHILD of the
    /// document element.
    ///
    /// # Errors
    /// [`SignerError::CertificateUnavailable`] when no usable signing
    /// certificate is available; [`SignerError::SigningFailed`] when
    /// the signing operation fails.
    fn sign_xades_epes(&self, doc: &str) -> Result<String, SignerError>;
}

#[cfg(any(test, feature = "fakes"))]
/// Deterministic test fake: SHA-256 hex in a `ds:Signature` element at
/// the real signature's position. Never a fallback — production wires a
/// real signer.
#[derive(Debug, Default)]
pub struct FakeSigner {
    signed: std::sync::Mutex<Vec<String>>,
}

#[cfg(any(test, feature = "fakes"))]
impl FakeSigner {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The UNSIGNED documents the fake signed, in signing order.
    ///
    /// # Panics
    /// If the state lock is poisoned.
    #[must_use]
    pub fn signed_documents(&self) -> Vec<String> {
        self.signed.lock().expect("fake signer state lock").clone()
    }

    /// # Panics
    /// If the state lock is poisoned.
    #[must_use]
    pub fn signed_count(&self) -> usize {
        self.signed.lock().expect("fake signer state lock").len()
    }
}

#[cfg(any(test, feature = "fakes"))]
impl FiscalSigner for FakeSigner {
    fn sign_xades_epes(&self, doc: &str) -> Result<String, SignerError> {
        use sha2::{Digest as _, Sha256};

        let Some(split) = doc.rfind('<') else {
            return Err(SignerError::SigningFailed(String::from(
                "fake signer: document has no closing tag",
            )));
        };
        let digest = hex::encode(Sha256::digest(doc.as_bytes()));
        let signed = format!(
            "{}<ds:Signature xmlns:ds=\"http://www.w3.org/2000/09/xmldsig#\" \
             Id=\"fake-signer\"><ds:SignatureValue>{digest}</ds:SignatureValue>\
             </ds:Signature>{}",
            &doc[..split],
            &doc[split..],
        );
        self.signed
            .lock()
            .expect("fake signer state lock")
            .push(doc.to_owned());
        Ok(signed)
    }
}
