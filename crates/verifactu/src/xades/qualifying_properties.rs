//! `QualifyingProperties` fragment builder — element order per the
//! `XAdES` 1.3.2 schema (ETSI TS 101 903); X.509 facts parsed via
//! `x509-cert` because bergshamra-keys keeps the chain opaque.

use base64::Engine as _;

use crate::clock::Timestamp;
use crate::fiscal::xml::escape_text;

use crate::xades::iso8601;
use crate::xades::policy::{PolicyHash, PolicyProfile};
use crate::xades::XadesError;

pub(crate) struct SigningContext<'a> {
    pub profile: &'a PolicyProfile,
    pub signature_id: &'a str,
    pub data_reference_id: &'a str,
    pub signed_properties_id: &'a str,
    pub signing_time: Timestamp,
    /// DER certificate chain, signer first.
    pub certificate_chain: &'a [Vec<u8>],
}

pub(crate) fn qualifying_properties_fragment(
    ctx: &SigningContext<'_>,
) -> Result<String, XadesError> {
    let profile = ctx.profile;
    let policy_hash = &profile.policy_hash;

    let mut out = String::with_capacity(2048);
    crate::xades::push_fmt(
        &mut out,
        format_args!(
            "<xades:QualifyingProperties xmlns:xades=\"{}\" Target=\"#{}\">\
<xades:SignedProperties Id=\"{}\">\
<xades:SignedSignatureProperties>\
<xades:SigningTime>{}</xades:SigningTime>",
            escape_text(profile.xades_namespace),
            escape_text(ctx.signature_id),
            escape_text(ctx.signed_properties_id),
            iso8601::format_utc(ctx.signing_time)
        ),
    );
    push_signing_certificate(&mut out, profile, ctx.certificate_chain)?;
    push_policy_identifier(&mut out, profile, policy_hash);
    out.push_str("</xades:SignedSignatureProperties>");

    let dof = profile.data_object_format;
    crate::xades::push_fmt(&mut out,
        format_args!(
            "<xades:SignedDataObjectProperties><xades:DataObjectFormat ObjectReference=\"#{}\">\
<xades:ObjectIdentifier><xades:Identifier>{}</xades:Identifier><xades:Description/></xades:ObjectIdentifier>\
<xades:MimeType>{}</xades:MimeType><xades:Encoding>{}</xades:Encoding>\
</xades:DataObjectFormat></xades:SignedDataObjectProperties>\
</xades:SignedProperties></xades:QualifyingProperties>",
            escape_text(ctx.data_reference_id),
            escape_text(dof.object_identifier),
            escape_text(dof.mime_type),
            escape_text(dof.encoding)
        ),
    );
    Ok(out)
}

fn push_signing_certificate(
    out: &mut String,
    profile: &PolicyProfile,
    chain: &[Vec<u8>],
) -> Result<(), XadesError> {
    out.push_str("<xades:SigningCertificate>");
    for cert_der in chain {
        let (issuer, serial) = issuer_serial(cert_der)?;
        crate::xades::push_fmt(
            out,
            format_args!(
                "<xades:Cert><xades:CertDigest>\
<ds:DigestMethod Algorithm=\"{}\"/>\
<ds:DigestValue>{}</ds:DigestValue>\
</xades:CertDigest><xades:IssuerSerial>\
<ds:X509IssuerName>{}</ds:X509IssuerName>\
<ds:X509SerialNumber>{}</ds:X509SerialNumber>\
</xades:IssuerSerial></xades:Cert>",
                escape_text(profile.cert_digest_algorithm),
                cert_digest_base64(cert_der, profile.cert_digest_algorithm)?,
                escape_text(&issuer),
                escape_text(&serial)
            ),
        );
    }
    out.push_str("</xades:SigningCertificate>");
    Ok(())
}

fn push_policy_identifier(out: &mut String, profile: &PolicyProfile, policy_hash: &PolicyHash) {
    // No Qualifier attribute and an empty Description — the official
    // AEAT example's shape.
    crate::xades::push_fmt(
        out,
        format_args!(
            "<xades:SignaturePolicyIdentifier><xades:SignaturePolicyId>\
<xades:SigPolicyId><xades:Identifier>{}</xades:Identifier><xades:Description/>\
</xades:SigPolicyId><xades:SigPolicyHash>\
<ds:DigestMethod Algorithm=\"{}\"/>\
<ds:DigestValue>{}</ds:DigestValue>\
</xades:SigPolicyHash><xades:SigPolicyQualifiers><xades:SigPolicyQualifier>\
<xades:SPURI>{}</xades:SPURI>\
</xades:SigPolicyQualifier></xades:SigPolicyQualifiers>\
</xades:SignaturePolicyId></xades:SignaturePolicyIdentifier>",
            escape_text(profile.identifier),
            escape_text(policy_hash.algorithm),
            escape_text(policy_hash.value_base64),
            escape_text(profile.spuri)
        ),
    );
}

pub(crate) fn cert_digest_base64(
    cert_der: &[u8],
    algorithm_uri: &str,
) -> Result<String, XadesError> {
    let digest = bergshamra_crypto::digest::digest(algorithm_uri, cert_der)
        .map_err(|e| XadesError::Key(format!("cert digest ({algorithm_uri}): {e}")))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(digest))
}

/// `(X509IssuerName, X509SerialNumber)` in the `XAdES` wire convention:
/// RFC 4514-style DN, lowercase attribute names, most-specific-first
/// (`cn=AC FNMT Usuarios,ou=Ceres,o=FNMT-RCM,c=ES` — the official AEAT
/// example's shape), serial unsigned decimal.
pub(crate) fn issuer_serial(cert_der: &[u8]) -> Result<(String, String), XadesError> {
    use x509_cert::der::Decode;
    let cert = x509_cert::Certificate::from_der(cert_der)
        .map_err(|e| XadesError::Key(format!("certificate parse: {e}")))?;
    Ok((
        format_issuer_name(&cert.tbs_certificate.issuer),
        format_serial_decimal(cert.tbs_certificate.serial_number.as_bytes()),
    ))
}

fn format_issuer_name(name: &x509_cert::name::Name) -> String {
    use x509_cert::der::oid::db::rfc4519;
    let mut parts: Vec<String> = Vec::new();
    for rdn in &name.0 {
        for atv in rdn.0.iter() {
            let label = if atv.oid == rfc4519::CN {
                "cn"
            } else if atv.oid == rfc4519::O {
                "o"
            } else if atv.oid == rfc4519::OU {
                "ou"
            } else if atv.oid == rfc4519::C {
                "c"
            } else if atv.oid == rfc4519::ST {
                "st"
            } else if atv.oid == rfc4519::L {
                "l"
            } else if atv.oid == rfc4519::SERIAL_NUMBER {
                "serialnumber"
            } else {
                parts.push(atv.oid.to_string());
                continue;
            };
            parts.push(format!("{label}={}", decode_any_string(&atv.value)));
        }
    }
    parts.reverse();
    parts.join(",")
}

/// Best-effort decode, matching bergshamra's own keyinfo ordering.
fn decode_any_string(any: &x509_cert::der::Any) -> String {
    use x509_cert::der::asn1::{Ia5StringRef, PrintableStringRef, Utf8StringRef};
    use x509_cert::der::Decode;
    if let Ok(s) = Utf8StringRef::from_der(any.value()) {
        return s.as_str().to_string();
    }
    if let Ok(s) = PrintableStringRef::from_der(any.value()) {
        return s.as_str().to_string();
    }
    if let Ok(s) = Ia5StringRef::from_der(any.value()) {
        return s.as_str().to_string();
    }
    String::from_utf8_lossy(any.value()).into_owned()
}

/// Big-endian magnitude bytes → unsigned decimal; serials exceed `u64`
/// (the official example's is ~8·10³⁷).
fn format_serial_decimal(bytes: &[u8]) -> String {
    /// `acc < 2560`, so the fit is structural, not hope.
    fn byte_digit(value: u32) -> u8 {
        u8::try_from(value).expect("byte-digit arithmetic stays under 256")
    }
    let mut digits: Vec<u8> = Vec::new();
    let mut remainder: Vec<u8> = bytes.to_vec();
    while !remainder.is_empty() {
        let mut quotient: Vec<u8> = Vec::with_capacity(remainder.len());
        let mut carry: u32 = 0;
        for &byte in &remainder {
            let acc = (carry << 8) | u32::from(byte);
            quotient.push(byte_digit(acc / 10));
            carry = acc % 10;
        }
        digits.push(byte_digit(carry) + b'0');
        while quotient.first() == Some(&0) {
            quotient.remove(0);
        }
        remainder = quotient;
    }
    if digits.is_empty() {
        return "0".to_string();
    }
    digits.reverse();
    String::from_utf8(digits).expect("serial digits are ASCII by construction")
}
