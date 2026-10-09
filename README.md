# verifactu

The one-crate facade for sending Veri*FACTU records to Spain's AEAT
(Suministro Inmediato de Información de facturas). A product declares
this crate — plus its async runtime — and nothing else.

Live-proven against the AEAT *pruebas* endpoint: the facade stage
(builder → tenant custody → `connect()` → chained `emit_invoice`
calls) answered `Correcto` / disposition `Complete` with a real FNMT
representación certificate, and the stored records answer
`Encontrada` through the public `ValidarQR` cotejo door.

## The two custody models

| Model | Certificate | Cabecera | Scale shape |
|---|---|---|---|
| `Verifactu::tenant` | the tenant's OWN | obligado = the tenant | one pooled transport PER tenant; cache the handles |
| `Verifactu::vendor` | OUR one certificate | obligado = the tenant, `Representante` = us | ONE pool shared by every tenant; emitters are cached |

## The production gate

`Environment::Pruebas` is the default. Production demands the consent
token AND the deployment key `VERIFACTU_ALLOW_PRODUCTION=1` — miss
either and `VerifactuBuilder::build` fails closed before any pool,
emitter, or certificate parse exists. The consenting line is grep-able
in review:

```ignore
Environment::production(ProductionOptIn::these_emissions_are_legally_binding())
```

## The product loop

One invoice is one `emit_invoice`; the return carries the emission AND
the chain cursor — the only durable state a product owns:

```ignore
let first = tenant.emit_invoice(draft, None).await?;    // primer registro
persist(&first.cursor);                                  // YOUR storage
let next = tenant.emit_invoice(draft2, Some(&first.cursor)).await?;
// ...and an invoice's stored cursor is what later cancels it:
let head = tenant.cancel_invoice(&next.cursor, Some(&next.cursor)).await?;
```

The printed QR is the emission's own data: `first.qr_url(env,
modality)` renders the cotejo URL (the four AEAT parameters). The
image is the consumer's print pipeline — render it at
error-correction level M and 30×30–40×40 mm per the QR spec's print
law (a generic QR library's default level L silently violates it).

The verdicts ride `verifactu::response` and the retry taxonomy is
`EmitError::class` over `ErrorClass`.

The eight documented AEAT endpoints (pruebas/production ×
personal-or-company/sello × remission/requerimiento) live in this
crate and nowhere else; the facade exposes no raw `.endpoint(url)`
door.

## Features

| Feature | Default | What it compiles |
|---|---|---|
| `http` | yes | the mTLS AEAT transport (rustls + hyper-util) |
| `signing` | yes | XAdES-EPES over the bergshamra family: the `Pkcs12` door, `engine::xades` |
| `edge` | no | `?Send` transport futures for single-threaded runtimes |
| `serde` | no | JSON shapes for `Emission`, the durable cursors, the sealed kinds, the consulta answers |
| `test-util` | no | owned-loopback TLS listeners + throwaway CAs |

### Bringing your own transport

`CertificateSource::Custom` supplies both legs — your `XAdES` signer
and your own `VerifactuTransport` implementation (an edge runtime
riding an mTLS-certificate binding, a hermetic fake). Build with
`default-features = false` and neither the HTTP stack nor the signing
tree is compiled. `endpoint_table` still names the URLs your
transport dials, and `engine::transport::parse_soap_answer` is the
read edge it should hand the raw answer body to.

### Edge runtimes (wasm32: Cloudflare Workers and kin)

- `default-features = false` compiles no host HTTP stack and no
  signing tree — the custom door takes your runtime's fetch and your
  signer.
- the `edge` feature drops the `+ Send` bound from `SendFuture`, so a
  runtime fetch (a JS `Promise`, structurally `!Send`) implements
  `VerifactuTransport` directly.
- wasm has no `SystemTime`: inject your runtime's date API through
  `VerifactuBuilder::clock` and the flat `emit_invoice` loop runs
  unmodified.

## Building

```sh
cargo build            # default features: http + signing
cargo test             # the test lattice
```

The `signing` feature rides the vendored
[bergshamra](https://github.com/kushaldas/bergshamra) XML-security
family under `third_party/vendor/`, pinned through `[patch.crates-io]`
in the root `Cargo.toml` so the exact source is committed here.
Consumers depending on this crate by git URL resolve the family from
crates.io instead (a dependency's `[patch]` is not inherited); add the
same `[patch.crates-io]` entries in your root manifest if you want the
vendored pins.

`fixtures/xades/` and
`contracts/aeat-verifactu/SuministroInformacion.xsd` are test
fixtures — certificate archives for the PKCS#12 unit tests and the
official SI.xsd the enumeration test pins — not runtime data.

## License

MIT OR Apache-2.0.
