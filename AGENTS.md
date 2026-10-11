# AGENTS.md

How to work on `verifactu`, a standalone Rust crate for sending
Veri*FACTU invoice records to Spain's AEAT. It is a general-purpose
library. No consumer's needs override the AEAT law or the crate's own
design. When a consumer hits friction, fix it in the general API (and
say so in the commit), never with a special case.

## Layout

```
contracts/aeat-verifactu/   AEAT's official XSDs + WSDL (vendored) and SOURCES.md
fixtures/xades/             signing test keys, golden signed records, AEAT's signed example
crates/verifactu/src/
  lib.rs                    the flat (product) face: builder, custody models, InvoiceDraft,
                            the production gate; `pub mod engine` re-exports the ledger face
  domain/                   pure core: chain (huella, ChainRecord, verify_chain), Money, Series
  fiscal/                   the emission engine
    mod.rs                  vocabularies, EmissionContext, EmitError, VerifactuEmitter
    xml.rs                  wire serializer, every field gate, the read-side tree parser
    consulta.rs             the consulta operation (request, read-back, huella audit)
    response.rs             respuesta parsing + the Disposition classifier
    transport.rs            the VerifactuTransport port, the read edge, FakeVerifactuTransport
    huso.rs, events.rs      instant rendering; event records (no public XSD)
  signer.rs                 the FiscalSigner port + FakeSigner
  xades/                    XAdES-EPES signing and verification   (feature `signing`)
  aeat/, wire/              the mTLS HTTP transport              (feature `http`)
  clock/, qr.rs             time port + civil calendar; the cotejo QR URL
crates/verifactu/tests/     integration tests (see "Testing")
```

Two public faces, and only two. The **flat** face (crate root) is for
products without their own ledger: `emit_invoice` / `cancel_invoice` /
`consulta`. The **engine** face is for ledger owners who seal records
themselves. Everything else is `pub(crate)`. A new public item needs a
reason a consumer outside this repo would recognise.

## The gate

Run all of it before every commit. Every line must pass.

```sh
cargo fmt --all -- --check
for f in "--no-default-features" \
         "--no-default-features --features http" \
         "--no-default-features --features signing" \
         "--no-default-features --features edge" \
         "--no-default-features --features serde" \
         "--no-default-features --features http,signing" \
         "" "--all-features"; do
  cargo clippy -p verifactu --all-targets $f -- -D warnings
  cargo test   -p verifactu $f
done
RUSTDOCFLAGS="-D warnings" cargo doc -p verifactu --no-deps --all-features
```

Lints come from the workspace: `clippy::all` is deny and `pedantic` is
warn, so with `-D warnings` both are errors. Fix the lint; only add an
`#[allow]` with a comment that says why the lint is wrong *there*. The
feature matrix matters: `cfg` mistakes only show up in the combination
that has them (for example, `http` without `signing`).

When a change touches the text, length or charset laws, the XML
serializer or the generators in `tests/schema_conformance.rs`, also
run the property tests hard:

```sh
PROPTEST_CASES=4096 cargo test -p verifactu --all-features --test schema_conformance
PROPTEST_CASES=20000 cargo test -p verifactu --all-features --lib char_law
```

## The one rule of the gates

A gate refuses exactly what AEAT would refuse. The cost is asymmetric:

- **Too loose**: a record is committed to the chain and only then
  rejected by AEAT. An original is immutable, so there is no clean
  remedy.
- **Too strict**: a legitimate invoice cannot be issued at all. This
  is how a client named with an emoji (U+1F338) once dead-lettered,
  because the `Char` predicate's last range was `0x10_0000..` instead of
  `0x1_0000..`.

So every gate cites its law (an SI.xsd type, an AEAT validation code,
an HS/SW/QR spec section, or "live-confirmed <date>" for behaviour
observed at AEAT pruebas). Every gate is tested on **both** edges: what
it admits and what it refuses. When AEAT's documents and AEAT's
behaviour differ, follow the behaviour and record it in the comment.

Laws worth knowing before you touch `fiscal/xml.rs`:

- **Text**: XML 1.0 `Char` (`check_xml_chars`, with the same predicate
  on the read side, `is_char_code`). `maxLength` counts characters, an
  emoji being one, as XSD defines it (`check_max`). Whether AEAT's Java
  validator counts an emoji as two is unverified: settle it at pruebas
  before changing the rule, never on a guess.
- **NIF**: 9 ASCII alphanumerics (`check_nif`).
- **NumSerieFactura**: 1..=60 printable ASCII (the QR law), with no edge
  whitespace, because the huella cadena trims what the wire sends
  verbatim (`check_num_serie`).
- **CodigoPais**: SI.xsd's `CountryType2` list (`CODIGOS_PAIS`,
  pinned to the XSD by a test).
- **Amounts**: `Money` holds at most 2 decimals; `render_amount` is the
  one renderer for both wire and huella, and zero always renders
  unsigned.

## Testing

Everything is offline and deterministic: no test touches the network,
the wall clock or AEAT. Live AEAT runs (the pruebas endpoint, real
certificates) happen outside this repo, so **do not write a test that a
live run would answer better**: specific AEAT error codes, endpoint
behaviour, the certificate gate. Test what a live run *cannot* cover:

| Test | What it pins | Oracle |
|---|---|---|
| `fiscal::xml` `char_law` tests | the text law equals XML 1.0, at every range edge and on arbitrary Unicode | uppsala (a conformant parser) |
| `tests/schema_conformance.rs` | every envío/consulta the gates admit validates against AEAT's XSDs; so does every document the fake can answer | uppsala's XSD validator over `contracts/` |
| `tests/xsd_enumerations.rs` | every closed vocabulary equals its XSD enumeration | SI.xsd |
| `domain::chain` anchors | the huella over AEAT's published vectors (HS §6.1–6.3, the signed example) | AEAT's own numbers |
| `tests/enveloped_scenarios.rs` | signing is byte-stable, digests match an independent C14N, AEAT's signed example verifies | golden files + AEAT's example |
| unit tests in `lib.rs`, `fiscal/` | the facade's laws: production gate, draft gates, custody wiring, retry classes | hand-checked expectations |
| `tests/loopback_*.rs` | transport failure classes no live endpoint produces on demand | owned TLS loopback (`test-util`) |

Rules for new tests:

- **Prefer an independent oracle over a hand-written expectation.** If a
  law has a spec artifact (an XSD, a published vector), test against
  the artifact.
- **Property tests must actually reach the oracle.** Generators produce
  mostly-valid input (`WILD` controls the rare defect). After changing
  a generator, check that most cases pass the gates, and mutation-check
  a gate (loosen it and watch the property fail).
- **Hermetic tests go through the real read edge.** `FakeSigner` and
  `FakeVerifactuTransport` parse everything they produce through the
  production parsers, and the fake's documents are schema-checked. Do
  not hand-roll stubs.
- A golden file changes only on purpose:
  `XADES_REGENERATE_GOLDEN=1` for the XAdES vector, `cargo insta review`
  for snapshots. Explain the diff in the commit.

## Conventions

- **Errors**: `EmitError` variants map to an `ErrorClass` (`Transient`
  retries; `OperatorAction`, `InvalidInput`, `Regulatory` and `Bug`
  never auto-retry). Error codes (`fiscal.invalid-record`, …) are a
  stable contract. Never panic on input; a panic documents a bug class
  with `# Panics`.
- **Vocabularies**: the AEAT spelling lives once, in the `wire()` /
  `as_str()` arms; `parse` is the reverse lookup (`parse_wire`,
  `vocabulary`).
- **Comments** state the law or the reason, never the history. Match
  the existing voice: short, precise, citing the source.
- **Secrets**: `Debug` impls redact signer, key and certificate
  material, and errors never echo archive bytes.
- **Contracts**: refreshing an AEAT artifact follows
  `contracts/aeat-verifactu/SOURCES.md`. The hash changes in the same
  commit as the bytes and the code the diff implies.

## Changing the public API

Consumers pin this crate by git rev, so a rev must be pushed to be
usable. A breaking change to the flat or engine face goes in its own
commit, whose message lists the migration (old item → new item).
Prefer removing a redundant item over keeping two ways to do one thing.
