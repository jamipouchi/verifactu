//! Enveloped-signature builder: the `ds:Signature` template is a plain
//! string (empty `DigestValue`s) inserted as the LAST child of the
//! document element by DOM surgery, never string splicing;
//! deterministic RSA PKCS#1 v1.5 makes identical inputs byte-identical
//! (golden-pinned).

use crate::clock::Timestamp;
use bergshamra_dsig::DsigContext;
use bergshamra_keys::Key;
use bergshamra_keys::KeysManager;

use crate::xades::policy::VERIFACTU_AGE;
use crate::xades::qualifying_properties::{qualifying_properties_fragment, SigningContext};
use crate::xades::XadesError;

/// Fixed IDs for deterministic output; bergshamra's duplicate-ID
/// detection fails loudly on a clash. The golden vectors pin these
/// spellings.
const SIGNATURE_ID: &str = "xmldsig-verifactu";
const DATA_REFERENCE_ID: &str = "xmldsig-verifactu-ref0";
const SIGNED_PROPERTIES_ID: &str = "xmldsig-verifactu-signedprops";

const RSA_SHA256_URI: &str = "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256";
const SHA256_URI: &str = "http://www.w3.org/2001/04/xmlenc#sha256";
const C14N_URI: &str = "http://www.w3.org/TR/2001/REC-xml-c14n-20010315";
const ENVELOPED_URI: &str = "http://www.w3.org/2000/09/xmldsig#enveloped-signature";
const DS_NS: &str = "http://www.w3.org/2000/09/xmldsig#";
const SIGNED_PROPERTIES_TYPE: &str = "http://uri.etsi.org/01903#SignedProperties";

/// # Errors
/// - [`XadesError::Xml`] when the document does not parse.
/// - [`XadesError::Signing`] for bergshamra digest/signature failures.
pub fn sign_enveloped(
    unsigned_doc: &str,
    key: &Key,
    signing_time: Timestamp,
) -> Result<String, XadesError> {
    let template = signature_template(unsigned_doc, key, signing_time)?;
    let mut manager = KeysManager::new();
    manager.add_key(key.clone());
    let ctx = DsigContext::new(manager);
    bergshamra_dsig::sign::sign_owned(&ctx, template)
        .map_err(|e| XadesError::Signing(format!("bergshamra template signing: {e}")))
}

fn signature_template(
    unsigned_doc: &str,
    key: &Key,
    signing_time: Timestamp,
) -> Result<String, XadesError> {
    let profile = &VERIFACTU_AGE;
    let fragment = qualifying_properties_fragment(&SigningContext {
        profile,
        signature_id: SIGNATURE_ID,
        data_reference_id: DATA_REFERENCE_ID,
        signed_properties_id: SIGNED_PROPERTIES_ID,
        signing_time,
        certificate_chain: &key.x509_chain,
    })?;

    let mut sig = String::with_capacity(2048 + fragment.len());
    crate::xades::push_fmt(
        &mut sig,
        format_args!("<ds:Signature xmlns:ds=\"{DS_NS}\" Id=\"{SIGNATURE_ID}\">"),
    );
    sig.push_str("<ds:SignedInfo>");
    crate::xades::push_fmt(
        &mut sig,
        format_args!(
            "<ds:CanonicalizationMethod Algorithm=\"{C14N_URI}\"/>\
<ds:SignatureMethod Algorithm=\"{RSA_SHA256_URI}\"/>"
        ),
    );

    crate::xades::push_fmt(
        &mut sig,
        format_args!(
            "<ds:Reference Id=\"{DATA_REFERENCE_ID}\" URI=\"\">\
<ds:Transforms><ds:Transform Algorithm=\"{ENVELOPED_URI}\"/>"
        ),
    );
    // The data reference carries ONLY the enveloped transform — the
    // official AEAT example's shape; C14N still happens at digest time
    // per XML-DSig's same-document reference model.
    crate::xades::push_fmt(
        &mut sig,
        format_args!(
            "</ds:Transforms>\
<ds:DigestMethod Algorithm=\"{SHA256_URI}\"/><ds:DigestValue/></ds:Reference>"
        ),
    );

    // SignedProperties reference: C14N transform + the XAdES Type.
    crate::xades::push_fmt(
        &mut sig,
        format_args!(
            "<ds:Reference Type=\"{SIGNED_PROPERTIES_TYPE}\" URI=\"#{SIGNED_PROPERTIES_ID}\">\
<ds:Transforms><ds:Transform Algorithm=\"{C14N_URI}\"/></ds:Transforms>\
<ds:DigestMethod Algorithm=\"{SHA256_URI}\"/><ds:DigestValue/></ds:Reference>"
        ),
    );

    sig.push_str("</ds:SignedInfo>");
    sig.push_str("<ds:SignatureValue/>");

    // Self-closing X509Data/KeyValue: bergshamra fills them from the
    // signing key — the official AEAT example's KeyInfo shape.
    sig.push_str("<ds:KeyInfo><ds:X509Data/><ds:KeyValue/></ds:KeyInfo>");

    crate::xades::push_fmt(&mut sig, format_args!("<ds:Object>{fragment}</ds:Object>"));
    sig.push_str("</ds:Signature>");

    insert_as_last_child(unsigned_doc, &sig)
}

fn insert_as_last_child(doc_xml: &str, fragment_xml: &str) -> Result<String, XadesError> {
    let err = |e: uppsala::XmlError| XadesError::Xml(format!("document parse: {e}"));
    let mut doc = uppsala::parse(doc_xml).map_err(err)?;
    let frag = uppsala::parse(fragment_xml)
        .map_err(|e| XadesError::Xml(format!("signature template parse: {e}")))?;
    let frag_root = frag
        .document_element()
        .ok_or_else(|| XadesError::Xml("signature template has no document element".into()))?;
    let imported = doc
        .import_subtree(&frag, frag_root)
        .ok_or_else(|| XadesError::Xml("cannot import signature subtree".into()))?;
    let root = doc
        .document_element()
        .ok_or_else(|| XadesError::Xml("unsigned document has no document element".into()))?;
    doc.append_child(root, imported);
    Ok(doc.to_xml())
}
