//! Read-path verifier for the enveloped `XAdES`-EPES signatures this
//! crate produces. No trust-store or validity-date checking,
//! deliberately: the official AEAT example's certificates are expired,
//! and trust decisions are outside this verifier's scope.

use crate::clock::Timestamp;
use bergshamra_dsig::verify::VerifyResult;
use bergshamra_dsig::DsigContext;
use bergshamra_keys::KeysManager;
use uppsala::{Document, NodeId};

use crate::xades::iso8601;
use crate::xades::policy::PolicyProfile;
use crate::xades::qualifying_properties::{cert_digest_base64, issuer_serial};

const SIGNED_PROPERTIES_TYPE: &str = "http://uri.etsi.org/01903#SignedProperties";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VerificationFailure {
    #[error("structural: {0}")]
    Structural(String),
    #[error("cryptographic: {0}")]
    Cryptographic(String),
    #[error("policy: {0}")]
    Policy(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDocument {
    pub policy_identifier: String,
}

/// # Errors
///
/// [`VerificationFailure`], classified per its variants.
pub fn verify_enveloped(
    signed_doc: &str,
    expected: &PolicyProfile,
    at: Option<Timestamp>,
) -> Result<VerifiedDocument, VerificationFailure> {
    let structural = |msg: String| VerificationFailure::Structural(msg);

    let mut ctx = DsigContext::new_permissive(KeysManager::new());
    if let Some(instant) = at {
        ctx = ctx.with_verification_time(iso8601::format_verification_time(instant));
    }
    let core = bergshamra_dsig::verify::verify(&ctx, signed_doc)
        .map_err(|e| structural(format!("XML-DSig parse/verify: {e}")))?;
    let (references, key_info) = match core {
        VerifyResult::Valid {
            references,
            key_info,
            ..
        } => (references, key_info),
        VerifyResult::Invalid { reason } => {
            return Err(VerificationFailure::Cryptographic(reason));
        }
    };

    let doc = uppsala::parse(signed_doc).map_err(|e| structural(format!("document parse: {e}")))?;

    let signed_props_uri = signed_properties_reference_uri(&doc)
        .ok_or_else(|| structural("no Reference with the SignedProperties Type".into()))?;
    let digested = references
        .iter()
        .any(|r| r.uri == signed_props_uri && r.digest_verified);
    if !digested {
        return Err(structural(format!(
            "SignedProperties reference {signed_props_uri} was not digested locally"
        )));
    }

    let signed_properties =
        signed_properties_for_reference(&doc, signed_props_uri).ok_or_else(|| {
            structural(format!(
                "no xades:SignedProperties with the verified reference's Id {signed_props_uri}"
            ))
        })?;
    let ssp = child(&doc, signed_properties, "SignedSignatureProperties")
        .ok_or_else(|| structural("no xades:SignedSignatureProperties".into()))?;

    let identifier = policy_identifier(&doc, ssp)
        .ok_or_else(|| structural("no xades:SignaturePolicyIdentifier/Identifier".into()))?;
    if identifier != expected.identifier {
        return Err(VerificationFailure::Policy(format!(
            "identifier {identifier} does not match expected {}",
            expected.identifier
        )));
    }
    let actual_hash =
        sig_policy_hash(&doc, ssp).ok_or_else(|| structural("no xades:SigPolicyHash".into()))?;
    if actual_hash != expected.policy_hash.value_base64 {
        return Err(VerificationFailure::Policy(format!(
            "SigPolicyHash {actual_hash} does not match the pinned {}",
            expected.policy_hash.value_base64
        )));
    }

    let Some(signer_cert) = key_info.x509_chain.first() else {
        return Err(structural("no certificate in KeyInfo".into()));
    };
    let (cert_digest_alg, cert_digest) = signing_certificate_digest(&doc, ssp)
        .ok_or_else(|| structural("no xades:SigningCertificate/CertDigest".into()))?;
    let actual_digest = cert_digest_base64(signer_cert, &cert_digest_alg)
        .map_err(|e| VerificationFailure::Cryptographic(format!("computing CertDigest: {e}")))?;
    if actual_digest != cert_digest {
        return Err(VerificationFailure::Cryptographic(format!(
            "CertDigest {cert_digest} does not match the KeyInfo certificate"
        )));
    }
    let (claimed_issuer, claimed_serial) = signing_certificate_issuer_serial(&doc, ssp)
        .ok_or_else(|| structural("no xades:SigningCertificate/IssuerSerial".into()))?;
    let (issuer, serial) = issuer_serial(signer_cert).map_err(|e| {
        VerificationFailure::Cryptographic(format!("parsing KeyInfo certificate: {e}"))
    })?;
    if issuer != claimed_issuer || serial != claimed_serial {
        return Err(VerificationFailure::Cryptographic(
            "IssuerSerial does not match the KeyInfo certificate".into(),
        ));
    }

    Ok(VerifiedDocument {
        policy_identifier: identifier,
    })
}

fn signed_properties_reference_uri<'d>(doc: &'d Document<'_>) -> Option<&'d str> {
    for id in doc.descendants(doc.root()) {
        if let Some(elem) = doc.element(id) {
            let ns = elem.name.namespace_uri.as_deref().unwrap_or("");
            if ns != "http://www.w3.org/2000/09/xmldsig#" || &*elem.name.local_name != "Reference" {
                continue;
            }
            if elem.get_attribute("Type") == Some(SIGNED_PROPERTIES_TYPE) {
                return elem.get_attribute("URI");
            }
        }
    }
    None
}

fn policy_identifier(doc: &Document<'_>, ssp: NodeId) -> Option<String> {
    let spi = child(doc, ssp, "SignaturePolicyIdentifier")?;
    let spid = child(doc, spi, "SignaturePolicyId")?;
    let sig_policy_id = child(doc, spid, "SigPolicyId")?;
    let identifier = child(doc, sig_policy_id, "Identifier")?;
    Some(doc.text_content_deep(identifier))
}

fn sig_policy_hash(doc: &Document<'_>, ssp: NodeId) -> Option<String> {
    let spi = child(doc, ssp, "SignaturePolicyIdentifier")?;
    let spid = child(doc, spi, "SignaturePolicyId")?;
    let hash = child(doc, spid, "SigPolicyHash")?;
    let value = child(doc, hash, "DigestValue")?;
    Some(doc.text_content_deep(value).trim().to_owned())
}

fn signing_certificate_digest(doc: &Document<'_>, ssp: NodeId) -> Option<(String, String)> {
    let sc = child(doc, ssp, "SigningCertificate")?;
    let cert = child(doc, sc, "Cert")?;
    let cert_digest = child(doc, cert, "CertDigest")?;
    let method = child(doc, cert_digest, "DigestMethod")?;
    let algorithm = doc
        .element(method)
        .and_then(|e| e.get_attribute("Algorithm"))
        .map(str::to_owned)?;
    let value = child(doc, cert_digest, "DigestValue")?;
    Some((algorithm, doc.text_content_deep(value).trim().to_owned()))
}

fn signing_certificate_issuer_serial(doc: &Document<'_>, ssp: NodeId) -> Option<(String, String)> {
    let sc = child(doc, ssp, "SigningCertificate")?;
    let cert = child(doc, sc, "Cert")?;
    let issuer_serial = child(doc, cert, "IssuerSerial")?;
    let issuer = child(doc, issuer_serial, "X509IssuerName")?;
    let serial = child(doc, issuer_serial, "X509SerialNumber")?;
    Some((
        doc.text_content_deep(issuer).trim().to_owned(),
        doc.text_content_deep(serial).trim().to_owned(),
    ))
}

/// The `SignedProperties` the verified reference's `#Id` names —
/// namespace-PREFIX matched so the 1.3.2 and 1.2.2 namespaces both
/// resolve; fragment binding, not document order, stays on the VERIFIED
/// signature in multi-signature documents.
fn signed_properties_for_reference(doc: &Document<'_>, uri: &str) -> Option<NodeId> {
    let id = uri.strip_prefix('#')?;
    doc.descendants(doc.root()).into_iter().find(|&node| {
        doc.element(node).is_some_and(|elem| {
            elem.name
                .namespace_uri
                .as_deref()
                .unwrap_or("")
                .starts_with("http://uri.etsi.org/01903/")
                && &*elem.name.local_name == "SignedProperties"
                && elem.get_attribute("Id") == Some(id)
        })
    })
}

/// Local-name navigation: `QualifyingProperties` mixes namespaces by
/// design.
fn child(doc: &Document<'_>, parent: NodeId, local_name: &str) -> Option<NodeId> {
    doc.children(parent).into_iter().find(|&id| {
        doc.element(id)
            .is_some_and(|e| e.name.local_name.as_ref() == local_name)
    })
}
