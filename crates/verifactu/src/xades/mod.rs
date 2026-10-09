pub mod enveloped;
pub mod policy;
pub mod verify;

mod iso8601;
mod qualifying_properties;

use std::sync::Arc;

use crate::clock::Clock;
use bergshamra_keys::Key;

/// `format_args!` into a `String` — a clippy-pedantic-clean `write!`.
pub(crate) fn push_fmt(out: &mut String, args: std::fmt::Arguments<'_>) {
    use std::fmt::Write as _;
    out.write_fmt(args)
        .expect("writing to an in-memory String cannot fail");
}

#[derive(Debug, thiserror::Error)]
pub enum XadesError {
    #[error("key or certificate problem: {0}")]
    Key(String),
    #[error("XML problem: {0}")]
    Xml(String),
    #[error("signing failed: {0}")]
    Signing(String),
}

/// # Errors
///
/// [`XadesError::Key`] when either PEM fails to parse.
pub fn load_key_with_certificate(key_pem: &[u8], cert_pem: &[u8]) -> Result<Key, XadesError> {
    let mut key = bergshamra_keys::loader::load_rsa_private_pem(key_pem)
        .map_err(|e| XadesError::Key(format!("private key: {e}")))?;
    let cert_key = bergshamra_keys::loader::load_x509_cert_pem(cert_pem)
        .map_err(|e| XadesError::Key(format!("certificate: {e}")))?;
    if cert_key.x509_chain.is_empty() {
        return Err(XadesError::Key(
            "certificate loader produced no DER chain".into(),
        ));
    }
    key.x509_chain = cert_key.x509_chain;
    Ok(key)
}

/// The [`crate::signer::FiscalSigner`] implementation over bergshamra. Time
/// comes exclusively from the injected [`Clock`]: a frozen clock
/// reproduces byte-identical signatures.
pub struct XadesSigner {
    clock: Arc<dyn Clock>,
    key: Key,
}

impl std::fmt::Debug for XadesSigner {
    /// Redacted: never prints key material.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XadesSigner")
            .field("clock", &"<clock>")
            .field("key", &self.key.algorithm_name())
            .finish()
    }
}

impl XadesSigner {
    /// # Errors
    /// [`crate::signer::SignerError::CertificateUnavailable`] on any key/cert
    /// load problem.
    pub fn new(
        clock: Arc<dyn Clock>,
        key_pem: &[u8],
        cert_pem: &[u8],
    ) -> Result<Self, crate::signer::SignerError> {
        let key = load_key_with_certificate(key_pem, cert_pem)
            .map_err(|_| crate::signer::SignerError::CertificateUnavailable)?;
        Ok(Self { clock, key })
    }

    /// Loads from a PKCS#12/PFX archive — **RSA-only, deliberately**:
    /// the signing path is validated for RSA and the Spanish FNMT
    /// install base is RSA; anything else answers
    /// `CertificateUnavailable` rather than sign under an unvalidated
    /// family.
    ///
    /// # Errors
    /// [`crate::signer::SignerError::CertificateUnavailable`] when the archive
    /// does not parse (wrong password included) or is not RSA.
    pub fn from_pkcs12(
        clock: Arc<dyn Clock>,
        pfx: &[u8],
        password: &str,
    ) -> Result<Self, crate::signer::SignerError> {
        // The loader's error is dropped, never echoed: it could carry
        // archive-derived material.
        let key = bergshamra_keys::loader::load_pkcs12(pfx, password)
            .map_err(|_| crate::signer::SignerError::CertificateUnavailable)?;
        // String compare, not the provider's enum: avoids a direct
        // dependency; bergshamra's own tests pin the spelling.
        if key.data.algorithm_name() != "RSA" {
            return Err(crate::signer::SignerError::CertificateUnavailable);
        }
        Ok(Self { clock, key })
    }
}

impl crate::signer::FiscalSigner for XadesSigner {
    fn sign_xades_epes(&self, doc: &str) -> Result<String, crate::signer::SignerError> {
        enveloped::sign_enveloped(doc, &self.key, self.clock.now_utc())
            .map_err(|e| crate::signer::SignerError::SigningFailed(e.to_string()))
    }
}
