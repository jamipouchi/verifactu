//! The record-signing seam (`VERIFACTU_AGE` XAdES-EPES). Fiscal never
//! links the signing stack itself; the modality-driven signing policy
//! (FS §2) lives in [`crate::fiscal::VerifactuEmitter`].

use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::signer::{FiscalSigner, SignerError};

/// The signature travels INSIDE each record, never on the envelope.
pub trait RecordSigner: Send + Sync {
    /// Returns the record with its `ds:Signature` as the LAST CHILD of
    /// the record root.
    ///
    /// # Errors
    /// [`SignerError`] from the underlying signer.
    fn sign_record(&self, record_xml: &str) -> Result<String, SignerError>;
}

pub struct XadesRecordSigner {
    inner: Arc<dyn FiscalSigner + Send + Sync>,
}

impl std::fmt::Debug for XadesRecordSigner {
    /// Redacted: never prints signer internals.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XadesRecordSigner")
            .field("inner", &"<qualified signer>")
            .finish()
    }
}

impl XadesRecordSigner {
    #[must_use]
    pub fn new(inner: Arc<dyn FiscalSigner + Send + Sync>) -> Self {
        Self { inner }
    }
}

impl RecordSigner for XadesRecordSigner {
    fn sign_record(&self, record_xml: &str) -> Result<String, SignerError> {
        self.inner.sign_xades_epes(record_xml)
    }
}

/// Deterministic test fake: SHA-256 hex in a `ds:Signature` envelope at
/// the real signature's position. Never a fallback — production wires
/// [`XadesRecordSigner`].
#[derive(Debug, Default)]
pub struct FakeSigner {
    signed: Mutex<Vec<String>>,
}

impl FakeSigner {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The UNSIGNED record inputs the fake signed, in signing order —
    /// the retention tests' oracle (the stored artifact must equal the
    /// fake's re-signing of its own recorded input).
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

impl RecordSigner for FakeSigner {
    fn sign_record(&self, record_xml: &str) -> Result<String, SignerError> {
        let Some(split) = record_xml.rfind('<') else {
            return Err(SignerError::SigningFailed(String::from(
                "fake signer: record document has no closing tag",
            )));
        };
        let digest = hex::encode(Sha256::digest(record_xml.as_bytes()));
        let signed = format!(
            "{}<ds:Signature xmlns:ds=\"http://www.w3.org/2000/09/xmldsig#\" \
             Id=\"fake-signer\"><ds:SignatureValue>{digest}</ds:SignatureValue>\
             </ds:Signature>{}",
            &record_xml[..split],
            &record_xml[split..],
        );
        self.signed
            .lock()
            .expect("fake signer state lock")
            .push(record_xml.to_owned());
        Ok(signed)
    }
}

/// The facade's hermetic door: `CertificateSource::Custom` speaks the
/// `FiscalSigner` port, so the fake implements it too — a hermetic
/// consumer wires `engine::FakeSigner` + `engine::FakeVerifactuTransport`
/// with zero hand-rolled stubs.
impl FiscalSigner for FakeSigner {
    fn sign_xades_epes(&self, doc: &str) -> Result<String, SignerError> {
        self.sign_record(doc)
    }
}
