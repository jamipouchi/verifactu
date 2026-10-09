# contracts/aeat-verifactu — SOURCES

Committed one-time snapshots of the authoritative AEAT Veri*FACTU artifacts
(ADR-0110 Phase-0 criterion via ADR-0115; ADR-0110 signing qualification).
**Never fetch these at test time** — this directory is the offline truth.

Retrieval date for every file: **2026-09-28** (the QR doc: **2026-10-05**,
URL supplied out-of-band). All fetches via `curl` (one-time, manual).
Evidence tags point into the frozen research vault
(`market_study/gates/…`, read-only).

## XAdES policy digest facts (for the `xades` policy registry)

Veri*FACTU reuses the **AGE (Administración General del Estado) signature
policy**, not an AEAT-specific document. Facts from
`EspecTecGenerFirmaElectRfact.pdf` §4 (v0.1.5, 2025-03-06) and its worked
example (§8.2 + `AnexosEjemplosFirmaRegFact.zip`):

| Fact | Value (verbatim) |
|---|---|
| Policy Identifier OID | `urn:oid:2.16.724.1.3.1.1.2.1.9` |
| Policy URL / SPURI | `https://sede.administracion.gob.es/politica_de_firma_anexo_1.pdf` |
| SigPolicyHash DigestMethod | `http://www.w3.org/2000/09/xmldsig#sha1` (SHA-1 — mandated by the AGE policy itself; applies **only** to the policy hash, not to the record signature) |
| SigPolicyHash DigestValue | `G7roucf600+f03r/o0bAOQ6WAs0=` (base64, verbatim from spec §4 and example §8.2) |
| SigPolicyId Description | empty (`<xades:Description/>`) in the official example |
| Signature class | XAdES-BES minimum, **EPES** (policy identifier present); ETSI EN 319 132 / ETSI TS 101 903, enveloped |
| CanonicalizationMethod | `http://www.w3.org/TR/2001/REC-xml-c14n-20010315` (C14N 1.0, inclusive) |
| SignatureMethod | `http://www.w3.org/2001/04/xmldsig-more#rsa-sha256` (official example; spec §4: RSA-based algos per ETSI TS 119 312 v1.4.3, **prefer RSA/SHA256**, RSA/SHA512 also admitted, key size ≥ 1024) |
| Data Reference | `URI=""` (whole record document), **Transforms: ONLY `http://www.w3.org/2000/09/xmldsig#enveloped-signature` — NO C14N transform** (confirms the ADR-0110 profile edge case: the C14N 1.0 canonicalization is applied at digest time, not declared as a Transform) |
| Data DigestMethod | `http://www.w3.org/2001/04/xmlenc#sha256` |
| SignedProperties Reference | `Type="http://uri.etsi.org/01903#SignedProperties"`, `URI="#…-signedprops"`, Transform `http://www.w3.org/TR/2001/REC-xml-c14n-20010315`, DigestMethod `http://www.w3.org/2001/04/xmlenc#sha256` |
| KeyInfo | `ds:X509Data` with signer **+ intermediate CA certs** (example carries 3 certs) **plus** `ds:KeyValue`/`ds:RSAKeyValue` |
| SigningCertificate | 3 `xades:Cert` entries (signer + chain), each `xades:CertDigest` with DigestMethod `http://www.w3.org/2000/09/xmldsig#sha1` + `xades:IssuerSerial` |
| SigningTime | present, timezone-aware (e.g. `2025-02-03T16:15:55.105+01:00`) |
| DataObjectFormat | `ObjectReference="#…-ref0"`, ObjectIdentifier `urn:oid:1.2.840.10003.5.109.10` (no Qualifier in example), `MimeType` `text/xml`, `Encoding` `UTF-8` |
| SignerRole | **not** used in the Veri*FACTU example |
| XAdES namespace | `http://uri.etsi.org/01903/v1.3.2#` (example also declares `xmlns:xades141="http://uri.etsi.org/01903/v1.4.1#"`, unused) |
| Timestamps | not required; optional XAdES-T only |
| Signature placement | enveloped **inside** the signed node: `RegistroFactura/RegistroAlta/ds:Signature`, `RegistroFactura/RegistroAnulacion/ds:Signature`, `RegistroEvento/Evento/ds:Signature`. Spec §5: sign the `RegistroAlta` / `RegistroAnulacion` / `RegistroEvento` node itself; **never** the enclosing `RegFactuSistemaFacturacion` or `RegistroFactura` nodes |
| Applicability | mandatory for non-VERI*FACTU systems (conservación) + event records; remission-by-requirement records |
| Certificates | qualified e-signature cert (eIDAS 910/2014), in force, from QTSP on the EU trusted list; holder = emitting company (obligado tributario) or authorised representative; AEAT accepts per Orden HAP/800/2014 |

### Huella (hash-chain) formula — from `Veri-Factu_especificaciones_huella_hash_registros.pdf` (v0.1.2)

Algorithm: **SHA-256 only** (Lista L12); output = 64-char UPPERCASE hex.
Input: UTF-8 bytes of `nombreCampo1=valor1&nombreCampo2=valor2&…`, fields
concatenated in the exact order below; values trimmed of leading/trailing
spaces; numeric fields normalize 1–2 decimals (trailing zeros insignificant);
missing/empty field ⇒ `nombre=` with nothing after the `=`.

- RegistroAlta (8 fields): `IDEmisorFactura`, `NumSerieFactura`,
  `FechaExpedicionFactura`, `TipoFactura`, `CuotaTotal`, `ImporteTotal`,
  `Huella` (of the previous record), `FechaHoraHusoGenRegistro`
- RegistroAnulacion (5 fields): `IDEmisorFacturaAnulada`,
  `NumSerieFacturaAnulada`, `FechaExpedicionFacturaAnulada`, `Huella`,
  `FechaHoraHusoGenRegistro`
- RegistroEvento (9 fields): `NIF` (SistemaInformatico), `ID` (IDOtro),
  `IdSistemaInformatico`, `Version`, `NumeroInstalacion`, `NIF`
  (ObligadoEmision), `TipoEvento`, `HuellaEvento` (previous event),
  `FechaHoraHusoGenEvento`

Official test vectors (verified locally 2026-09-28, all MATCH):
1. `IDEmisorFactura=89890001K&NumSerieFactura=12345678/G33&FechaExpedicionFactura=01-01-2024&TipoFactura=F1&CuotaTotal=12.35&ImporteTotal=123.45&Huella=&FechaHoraHusoGenRegistro=2024-01-01T19:20:30+01:00`
   → `3C464DAF61ACB827C65FDA19F352A4E3BDC2C640E9E9FC4CC058073F38F12F60`
2. (case 1 output as `Huella=`, NumSerieFactura `12345679/G34`, ts `19:20:35+01:00`)
   → `F7B94CFD8924EDFF273501B01EE5153E4CE8F259766F88CF6ACB8935802A2B97`
3. Anulación case: `IDEmisorFacturaAnulada=89890001K&NumSerieFacturaAnulada=12345679/G34&FechaExpedicionFacturaAnulada=01-01-2024&Huella=F7B94CFD…2A2B97&FechaHoraHusoGenRegistro=2024-01-01T19:20:40+01:00`
   → `177547C0D57AC74748561D054A9CEC14B4C4EA23D1BEFD6F2E69E3A388F90C68`

## QR cotejo-URL facts (for the facade's `qr` module)

From `DetalleEspecificacTecnCodigoQRfactura.pdf` (v0.5.0, 35 pages):

| Fact | Value |
|---|---|
| Bases (§5.1 verifiable / §5.2 no-verificable) | pruebas `https://prewww2.aeat.es/wlpl/TIKE-CONT/ValidarQR{NoVerifactu}`, production `https://www2.agenciatributaria.gob.es/wlpl/TIKE-CONT/ValidarQR{NoVerifactu}` |
| Parameters (§5) | EXACTLY four, mandatory: `nif`, `numserie`, `fecha` (`DD-MM-AAAA`), `importe` (max 9 int digits + `.DD`) |
| Encoding (§4) | URL-encoding of parameter CONTENT, UTF-8; printable ASCII 32..=126 only; worked example encodes `&` inside numserie as `%26` |
| Optional params (§6–§7) | `idioma` (default `ES`) + `formato` (e.g. `json`) — machine cotejo by art. 20.2 receivers, NEVER the printed QR |
| Print law (art. 21 via §3) | 30×30–40×40 mm; error-correction level **M**; `QR tributario:` above; verifiable-phrase below (remission modality only): `Factura verificable en la sede electrónica de la AEAT` or `VERI*FACTU` |
| Anuladas | no anulación QR shape exists — cotejo of the factura's own URL answers the annulment state |

## Files

| File | Bytes | sha256 | Source URL (served) | Status | Evidence |
|---|---|---|---|---|---|
| `SistemaFacturacion.wsdl` | 8780 | `05919120708ff7650612fa6683c9336eaf919335d9a4db10e86759190af48602` | `https://prewww2.aeat.es/static_files/common/internet/dep/aplicaciones/es/aeat/tikeV1.0/cont/ws/SistemaFacturacion.wsdl` | 200, direct (no redirect) | readiness.md §03 row "WSDL" (`200, 8.8KB`); 03-verifactu.md §Minimum protocol |
| `SuministroInformacion.xsd` | 49540 | `ee4c1655175644de44c4c25055ffeb8e5f4bb4bc3834ce8254d4222ef18c8aa1` | `https://prewww2.aeat.es/…/tikeV1.0/cont/ws/SuministroInformacion.xsd` | 200, direct | readiness.md §03 (`200, 49.5KB`); 03-verifactu.md `[STR]` Farmatic WSDL/XSD list |
| `SuministroLR.xsd` | 1573 | `cbdac8d427cc5ab5d77ca48974cab0f35d6bb819c4c66db361681e3710aeba36` | same directory, `SuministroLR.xsd` | 200, direct | readiness.md §03 (`200, 1.6KB`) |
| `ConsultaLR.xsd` | 3886 | `bf2cdb8fc4b95b291757a72b76d8fffca06a6d30d9329122ca2fd6b2d5f8f1b1` | same directory, `ConsultaLR.xsd` | 200, direct | readiness.md §03 row "`ConsultaLR` / `Respuesta*` same path, 200" |
| `RespuestaConsultaLR.xsd` | 10058 | `de35063acb8d9ba0d6ae51acc6b595de9c2b12333250e95e13108ef5f2670d45` | same directory (referenced by WSDL `xs:import`) | 200, direct | same |
| `RespuestaSuministro.xsd` | 6259 | `82acf80f785643caac13087aae66808ed721a13f08ca5218cf8ae81b695549ef` | same directory (referenced by WSDL `xs:import`) | 200, direct | same |
| `Veri-Factu_Descripcion_SWeb.pdf` | 1712899 | `b3570f6a308ce98a5f52001a0dc427310ad6cf7bccd60a9ee98720a59e553c02` | `https://sede.agenciatributaria.gob.es/static_files/AEAT_Desarrolladores/EEDD/IVA/VERI-FACTU/Veri-Factu_Descripcion_SWeb.pdf` | 200, direct; PDF 1.7, 101 pages, v1.0.3 (2025-07-28) | readiness.md §03 row "Technical PDF (Anexo II) `200, 1.7MB`"; 03-verifactu.md §Test env |
| `EspecTecGenerFirmaElectRfact.pdf` | 1497921 | `60953acd5d437a745db9076377739d8ef3f05c31967c1424cd4f6a8ac04af481` | `https://www.agenciatributaria.es/static_files/AEAT_Desarrolladores/EEDD/IVA/VERI-FACTU/Espec-Tecnicas/EspecTecGenerFirmaElectRfact.pdf` | 200, direct; PDF 1.5, 15 pages, v0.1.5 (2025-03-06) | linked from AEAT page `informacion-tecnica/especificaciones-tecnicas-firma-electronica-registros-evento.html` (hub documented in 03-verifactu.md §Official documentation) |
| `Veri-Factu_especificaciones_huella_hash_registros.pdf` | 1174110 | `f4334c254bb875b417247b54315199f89d75a8c4814dfd1e86efec562653d7de` | `https://www.agenciatributaria.es/static_files/AEAT_Desarrolladores/EEDD/IVA/VERI-FACTU/Veri-Factu_especificaciones_huella_hash_registros.pdf` | 200, direct; PDF 1.5, 13 pages, v0.1.2 | linked from AEAT page `informacion-tecnica/algoritmo-calculo-codificacion-huella-hash.html` (same hub) |
| `AnexosEjemplosFirmaRegFact.zip` | 5881 | `66fac533bef6c93c041b08704bf538843fe552052a2994899aba0875c2c4b24e` | `https://www.agenciatributaria.es/static_files/AEAT_Desarrolladores/EEDD/IVA/VERI-FACTU/Espec-Tecnicas/AnexosEjemplosFirmaRegFact.zip` | 200, direct; contains `ejemploRegistro.xml` + `ejemploRegistro-firmado-epes-xades4j.xml` (golden vectors for the signer) | same page as `EspecTecGenerFirmaElectRfact.pdf` |
| `xmldsig-core-schema.xsd` | 10292 | `d102ad3df7664c307e0c2c776ba4a90513b1969974d8a940bae1a77f9f21e15d` | `http://www.w3.org/TR/xmldsig-core/xmldsig-core-schema.xsd` | 200, direct (no TLS upgrade performed by W3C on this URL); this is the exact `schemaLocation` imported by `SuministroInformacion.xsd` and `Facturaev3_2_2.xml` | transitive import of the AEAT XSD (ADR-0110: snapshot transitively-imported schemas) |
| `XAdES.xsd` | 21309 | `f8dca3a10ccd5b8662729b044384daa96a1866d905e498065bd09bf0a1b79f9c` | `https://raw.githubusercontent.com/esig/dss/master/specs-xades/src/main/resources/xsd/XAdES.xsd` (EU DSS — ADR-0110's qualification oracle) | 200, direct; targetNamespace `http://uri.etsi.org/01903/v1.3.2#` | ETSI TS 101 903 v1.3.2 schema (see provenance note) |
| `DetalleEspecificacTecnCodigoQRfactura.pdf` | 786534 | `f86b3c260d8a4963dbc18c5007732b53199156c5d1db63242e68db71501b49eb` | `https://www.agenciatributaria.es/static_files/AEAT_Desarrolladores/EEDD/IVA/VERI-FACTU/DetalleEspecificacTecnCodigoQRfactura.pdf` | 200, direct; PDF 1.7, 35 pages, v0.5.0 | URL supplied out-of-band 2026-10-05; the QR law facts table above |

### XAdES.xsd provenance note (mirror, not ETSI direct)

ETSI's own delivery (`https://www.etsi.org/deliver/etsi_ts/101900_101999/101903/01.03.02_60/ts_101903v010302p0.zip`)
and the canonical `https://uri.etsi.org/01903/v1.3.2/XAdES01903v132-1.xsd`
both rejected automated retrieval on 2026-09-28 (HTTP 403 WAF page with
default UA; 404 with browser UA). The snapshot is the copy carried by the
**EU Digital Signature Service** reference project (`esig/dss`,
`specs-xades/src/main/resources/xsd/XAdES.xsd`, mapped in its `catalog.cat`
to canonical system ID `http://uri.etsi.org/01903/v1.3.2/XAdES.xsd`).
Cross-verified byte-identical (modulo one `schemaLocation` line and comments)
against a second independent mirror (Norwegian agency heritage:
`github.com/felleslosninger/efm-asic` ← `difi/begrep-SikkerDigitalPost`).
The only difference: DSS keeps the canonical
`schemaLocation="http://www.w3.org/TR/2002/REC-xmldsig-core-20020212/xmldsig-core-schema.xsd"`
import. The dated W3C URL serves a file whose sole difference from the
snapshot `xmldsig-core-schema.xsd` is the RCS comment line
(`$Revision: 1.1 $ 2002` vs `$Revision: 1.2 $ 2013`); schema content is
identical.

### Host notes (recorded per instructions)

- The WSDL + all 5 XSDs were served by `prewww2.aeat.es` at the `tikeV1.0`
  path (the WSDL's own import namespace URIs point at `www2.agenciatributaria.gob.es/…/tike/cont/ws/…`).
  Spot-check 2026-09-28: `https://www2.agenciatributaria.gob.es/…/tike/cont/ws/SuministroInformacion.xsd`
  and `https://prewww2.aeat.es/…/tike/cont/ws/SuministroInformacion.xsd`
  (non-V1.0 path) both return 200 with byte-identical content
  (sha256 `ee4c1655…c8aa1`).
- `Veri-Factu_Descripcion_SWeb.pdf` ("Anexo II" per the vault) documents the
  remission operativa (record lifecycle, SOAP examples, validations) — it does
  **not** contain the XAdES profile. The profile lives in
  `EspecTecGenerFirmaElectRfact.pdf`; the hash-chain algorithm in
  `Veri-Factu_especificaciones_huella_hash_registros.pdf` ("documento de
  huella" that Anexo II §9.1.1 defers to). All three are snapshotted.
