//! The `Veri*FACTU` AGE signature-policy profile: identifiers and
//! digest values verbatim from AEAT's signing specification (facts in
//! `contracts/aeat-verifactu/SOURCES.md`), never from re-hashing live
//! downloads.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyHash {
    pub algorithm: &'static str,
    pub value_base64: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataObjectFormat {
    pub object_identifier: &'static str,
    pub mime_type: &'static str,
    pub encoding: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyProfile {
    pub identifier: &'static str,
    pub policy_hash: PolicyHash,
    pub spuri: &'static str,
    pub xades_namespace: &'static str,
    /// `CertDigest` — SHA-1, as the official AEAT example carries.
    pub cert_digest_algorithm: &'static str,
    pub data_object_format: DataObjectFormat,
}

/// Facts from `contracts/aeat-verifactu/SOURCES.md`; the record
/// signature itself stays RSA-SHA256.
pub const VERIFACTU_AGE: PolicyProfile = PolicyProfile {
    identifier: "urn:oid:2.16.724.1.3.1.1.2.1.9",
    policy_hash: PolicyHash {
        algorithm: "http://www.w3.org/2000/09/xmldsig#sha1",
        value_base64: "G7roucf600+f03r/o0bAOQ6WAs0=",
    },
    spuri: "https://sede.administracion.gob.es/politica_de_firma_anexo_1.pdf",
    xades_namespace: "http://uri.etsi.org/01903/v1.3.2#",
    cert_digest_algorithm: "http://www.w3.org/2000/09/xmldsig#sha1",
    data_object_format: DataObjectFormat {
        object_identifier: "urn:oid:1.2.840.10003.5.109.10",
        mime_type: "text/xml",
        encoding: "UTF-8",
    },
};
