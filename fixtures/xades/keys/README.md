# XAdES signing fixtures — qualification test key

NON-PRODUCTION. RSA-2048 key + CA-signed self-generated certificate whose
only purpose is the `xades` crate's byte-stable golden vectors and the
qualification corpus: signatures are a function of (key, document, policy,
instant), so the drift detector needs the key as a committed constant.
Never valid for a real emission: test CN, fixed serial `01`, committed —
golden outputs are regenerated from THESE files, never from a fresh key
(regeneration is a visible diff: run tests with XADES_REGENERATE_GOLDEN=1).

- `test-key.pem` / `test-cert.pem` — the qualification pair (leaf, serial 01)
- `test.p12` — the leaf alone (password `secret123`)
- `two-cert-leaffirst.p12` / `two-cert-cafirst.p12` — the leaf + its CA in
  both bag orders (the live-learned FNMT shape: archives arrive CA-first;
  the parser must yield the user cert first either way)

Regeneration (OpenSSL 3.x): CA key → self-signed CA (serial 2) → leaf CSR
signed by the CA (serial 1, long validity) → `pkcs12 -export` for the
archives; the CA-first order needs a repack that puts the CA bag first with
the leaf's key riding.
