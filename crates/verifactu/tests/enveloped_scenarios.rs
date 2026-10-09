use std::path::PathBuf;

use base64::Engine as _;
use sha2::Digest as _;
use verifactu::engine::enveloped;
use verifactu::engine::policy;
use verifactu::engine::verify;
use verifactu::engine::Timestamp;

const FROZEN_SIGNING_INSTANT: Timestamp = Timestamp(1_790_598_896);

/// Inside the official AEAT example's certificate validity window.
const OFFICIAL_EXAMPLE_INSTANT: Timestamp = Timestamp(1_738_598_155);

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(rel: &str) -> String {
    let path = repo_root().join("fixtures/xades").join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture {}: {e}", path.display()))
}

fn load_signing_key() -> bergshamra_keys::Key {
    let key_pem = fixture("keys/test-key.pem");
    let cert_pem = fixture("keys/test-cert.pem");
    verifactu::engine::load_key_with_certificate(key_pem.as_bytes(), cert_pem.as_bytes())
        .expect("committed qualification key + cert load")
}

#[test]
fn golden_verifactu_signature_is_byte_stable_and_matches_committed_vector() {
    let unsigned = fixture("golden/verifactu-record-unsigned.xml");
    let key = load_signing_key();

    let first =
        enveloped::sign_enveloped(&unsigned, &key, FROZEN_SIGNING_INSTANT).expect("first signing");
    let second =
        enveloped::sign_enveloped(&unsigned, &key, FROZEN_SIGNING_INSTANT).expect("second signing");
    assert_eq!(
        first, second,
        "same inputs produce byte-identical signatures (frozen SigningTime, PKCS#1 v1.5)"
    );

    // Independent cross-check: SHA-256 over inclusive C14N computed
    // through bergshamra-c14n directly, not the signer's path.
    let expected_digest = {
        let canonical = bergshamra_c14n::canonicalize(
            &unsigned,
            bergshamra_c14n::C14nMode::Inclusive,
            None,
            &[] as &[&str],
        )
        .expect("independent C14N of the unsigned record");
        let digest = sha2::Sha256::digest(&canonical);
        base64::engine::general_purpose::STANDARD.encode(digest)
    };
    let data_ref_at = first
        .find("<ds:Reference Id=\"xmldsig-verifactu-ref0\" URI=\"\">")
        .expect("data reference");
    let ref_chunk = &first[data_ref_at..];
    let dv_at = ref_chunk
        .find("<ds:DigestValue>")
        .expect("data DigestValue")
        + "<ds:DigestValue>".len();
    let dv_end = ref_chunk[dv_at..]
        .find("</ds:DigestValue>")
        .expect("closing")
        + dv_at;
    assert_eq!(
        &ref_chunk[dv_at..dv_end],
        expected_digest,
        "data-reference digest equals an independently computed SHA-256 over inclusive C14N"
    );

    // Drift detector: the committed golden must equal fresh generation;
    // XADES_REGENERATE_GOLDEN rewrites it so the change lands as a
    // reviewable diff.
    let golden_path = repo_root().join("fixtures/xades/golden/verifactu-record-signed.xml");
    if std::env::var_os("XADES_REGENERATE_GOLDEN").is_some() {
        std::fs::write(&golden_path, &first).unwrap_or_else(|e| panic!("regenerate golden: {e}"));
    }
    let committed = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|e| panic!("committed golden missing ({}): {e}", golden_path.display()));
    assert_eq!(
        first, committed,
        "committed golden vector drifted from fresh generation (regenerate deliberately via XADES_REGENERATE_GOLDEN=1)"
    );
}

/// AEAT's own xades4j-signed `RegistroAlta` must pass our read-path
/// verifier — the example's certificates are EXPIRED; no trust-store
/// or time validation (deliberate scope).
#[test]
fn official_aeat_signed_example_verifies_against_verifactu_registry_entry() {
    let official = fixture("golden/aeat-official/ejemploRegistro-firmado-epes-xades4j.xml");
    let outcome = verify::verify_enveloped(
        &official,
        &policy::VERIFACTU_AGE,
        Some(OFFICIAL_EXAMPLE_INSTANT),
    )
    .expect("official AEAT example verifies");
    assert_eq!(outcome.policy_identifier, "urn:oid:2.16.724.1.3.1.1.2.1.9");
}
