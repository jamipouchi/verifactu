//! The PKCS#12 door for the TLS client identity — the same
//! `bergshamra-pkcs12` parser the signing leg rides, so the `XAdES`
//! key and the TLS key come from one archive.

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

pub struct ClientIdentity {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

impl ClientIdentity {
    #[must_use]
    pub fn new(chain: Vec<CertificateDer<'static>>, key: PrivateKeyDer<'static>) -> Self {
        Self { chain, key }
    }

    #[must_use]
    pub fn chain(&self) -> &[CertificateDer<'static>] {
        &self.chain
    }

    #[must_use]
    pub fn key(&self) -> PrivateKeyDer<'static> {
        self.key.clone_key()
    }

    #[must_use]
    pub fn into_parts(self) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
        (self.chain, self.key)
    }
}

impl std::fmt::Debug for ClientIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientIdentity")
            .field("chain_len", &self.chain.len())
            .finish_non_exhaustive()
    }
}

/// Variants never echo archive-derived material.
#[cfg(feature = "signing")]
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("PKCS#12 archive does not parse (wrong password or not a PKCS#12 file)")]
    Archive,
    #[error("PKCS#12 archive carries no private key")]
    NoKey,
    #[error("PKCS#12 archive carries no certificate")]
    NoCertificate,
}

/// Extracts the TLS client identity from a PKCS#12 archive, chain
/// LEAF FIRST (the leaf is the cert that is no other cert's issuer).
/// Live-learned: the FNMT representación export carries the user cert
/// PLUS the FNMT CA — a topmost-first ordering handed rustls the CA
/// as the end entity (`KeyMismatch`).
///
/// # Errors
/// [`IdentityError`] per its variants.
#[cfg(feature = "signing")]
pub fn identity_from_pkcs12(pfx: &[u8], password: &str) -> Result<ClientIdentity, IdentityError> {
    let contents =
        bergshamra_pkcs12::parse_pkcs12(pfx, password).map_err(|_| IdentityError::Archive)?;
    let key = contents.private_keys.first().ok_or(IdentityError::NoKey)?;
    let chain: Vec<CertificateDer<'static>> = contents
        .certificates
        .iter()
        .cloned()
        .map(CertificateDer::from)
        .collect::<Vec<_>>();
    if chain.is_empty() {
        return Err(IdentityError::NoCertificate);
    }
    Ok(ClientIdentity {
        chain: order_leaf_first(chain),
        key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.as_ref().to_vec())),
    })
}

#[cfg(feature = "signing")]
fn order_leaf_first(chain: Vec<CertificateDer<'static>>) -> Vec<CertificateDer<'static>> {
    if chain.len() < 2 {
        return chain;
    }
    let names: Vec<Option<(String, String)>> = chain.iter().map(cert_name).collect();
    let mut candidates: Vec<usize> = (0..chain.len())
        .filter(|&i| {
            let Some((subject_i, _)) = &names[i] else {
                return false;
            };
            (0..chain.len()).all(|j| {
                i == j
                    || names[j]
                        .as_ref()
                        .is_none_or(|(_, issuer_j)| issuer_j != subject_i)
            })
        })
        .collect();
    if candidates.len() != 1 {
        return chain;
    }
    let leaf = candidates.remove(0);
    let mut ordered = vec![chain[leaf].clone()];
    ordered.extend(
        (0..chain.len())
            .filter(|&i| i != leaf)
            .map(|i| chain[i].clone()),
    );
    ordered
}

#[cfg(feature = "signing")]
fn cert_name(cert: &CertificateDer<'_>) -> Option<(String, String)> {
    use x509_cert::der::Decode as _;
    let parsed = x509_cert::Certificate::from_der(cert.as_ref()).ok()?;
    Some((
        parsed.tbs_certificate.subject.to_string(),
        parsed.tbs_certificate.issuer.to_string(),
    ))
}

#[cfg(all(test, feature = "signing"))]
mod tests {
    use super::identity_from_pkcs12;

    const PFX: &[u8] = include_bytes!("../../../../fixtures/xades/keys/test.p12");
    const PASSWORD: &str = "secret123";

    #[test]
    fn the_committed_archive_yields_an_identity() {
        let identity = identity_from_pkcs12(PFX, PASSWORD).expect("the archive loads");
        assert_ne!(identity.chain().len(), 0);
        assert!(
            identity.chain()[0].as_ref().starts_with(&[0x30]),
            "the leaf parses as DER (a SEQUENCE)"
        );
    }

    /// The FNMT representación regression: either bag order must yield
    /// the user cert first.
    #[test]
    fn multi_certificate_archives_order_the_leaf_first_either_bag_order() {
        let leaffirst: &[u8] =
            include_bytes!("../../../../fixtures/xades/keys/two-cert-leaffirst.p12");
        let cafirst: &[u8] = include_bytes!("../../../../fixtures/xades/keys/two-cert-cafirst.p12");
        for (label, archive) in [("leaf-first bag", leaffirst), ("CA-first bag", cafirst)] {
            let identity = identity_from_pkcs12(archive, PASSWORD)
                .unwrap_or_else(|e| panic!("{label} loads: {e}"));
            assert_eq!(
                identity.chain().len(),
                2,
                "{label}: both certs ride the chain"
            );
            rustls::sign::CertifiedKey::from_der(
                identity.chain().to_vec(),
                identity.key(),
                &rustls::crypto::ring::default_provider(),
            )
            .unwrap_or_else(|e| {
                panic!("{label}: the leaf is first (the key matches the end entity): {e}")
            });
        }
    }
}
