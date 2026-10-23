# Changelog

Notable changes to `verifactu`. Consumers pin the crate by git rev; each
entry states its semver impact against the last pinned rev. While the
crate is 0.x, a breaking change bumps the minor version.

## [0.3.0] — 2026-10-23

Measured against `8234d1a` (0.2.0). Semver: minor bump, 0.2 → 0.3, for
one breaking change. Error codes unchanged.

### Added — breaking

- **The consulta filters by issue date** (`FiltroConsulta/FechaExpedicionFactura`).
  `ConsultaFilter::fecha_expedicion(&fecha)` asks for one day;
  `ConsultaFilter::rango_fecha_expedicion(desde, hasta)` for a range,
  either end open (`None`), both ends inclusive. It narrows within the
  period and does not replace it. A range whose `Desde` is not before its
  `Hasta` is refused as `fiscal.invalid-record` (AEAT 1250).

  Migration: a `ConsultaFilter { .. }` literal adds
  `fecha_expedicion: None`; `ConsultaFilter::new` already sets it.

### Live-confirmed at AEAT pruebas, 2026-10-23

- `Desde` and `Hasta` include their own day; `Desde` equal to `Hasta`
  answers 1250, as the code reads.
- A range with neither end narrows nothing; a future `Desde` is admitted.
- The issue date filters a record filed under an earlier operation's
  month by its own date.

## [0.2.0] — 2026-10-22

Measured against `94767d6`, the last pinned rev. A consumer pinned
earlier also needs the migrations listed in the breaking (`!`) commits
since its rev.

Semver: minor bump, 0.1 → 0.2. Every breaking change below lists its
migration. Error codes stay a stable contract: none changed meaning, and
two were added.

### Changed — breaking

- `lines::sujeta_iva_recargo` takes the issuer's regime instead of a bare
  recargo rate, and `ClaveRegimen` follows the issuer, not the recargo.
  AEAT ties 18 to a seller that is itself in recargo de equivalencia (L8A;
  the libros registro FAQ §2.17), whose own sales carry no recargo. A
  general-regime seller charging recargo to a client in the regime declares
  01 with `TipoRecargoEquivalencia` / `CuotaRecargoEquivalencia`. Before
  this change the helper declared 18 on every recargo line, so a wholesaler,
  or a pharmacy invoicing a recargo client, declared its client's regime as
  its own.

  Migration:

  - `lines::sujeta_iva_recargo(rate, recargo, base)` (declared 18) →
    `lines::sujeta_iva_recargo(rate, lines::RegimenEmisor::General {
    recargo_rate_cents: recargo }, base)` (declares 01, same recargo
    amounts).
  - An issuer in recargo de equivalencia declaring its own sale →
    `lines::sujeta_iva_recargo(rate, lines::RegimenEmisor::RecargoEquivalencia, base)`
    (declares 18, no recargo fields).

  The gates did not change. AEAT accepts recargo under either key
  (live-confirmed at pruebas: under 18 on 2026-10-05, under 01 on
  2026-10-16), so records already sealed under 18 stand. The §15.3 rate
  pairing (and 1281, 1284) still refuses what AEAT refuses. A hand-built
  `DetalleDesglose` still reaches every combination AEAT admits.

- **A transport failure is typed, and a refusal never carries AEAT's
  `Location`.** When AEAT's certificate gate refused an exchange, it
  answered with a redirect, and the failure quoted the redirect's
  `Location`. That URL carries the gate's session, so a credential reached
  every log that printed the error. `TransportFailure` is now an enum
  (`Display` + `std::error::Error`), and `Refused` holds the HTTP status
  only, so it cannot carry a URL:

  | Variant | When | `EmitError::class()` | `EmitError::code()` |
  |---|---|---|---|
  | `Wire { detail }` | the leg failed before any answer: unreachable, timeout, TLS (a certificate refused in the handshake included) | `Transient` | `fiscal.transport` |
  | `Unreadable { detail }` | an answer the exchange cannot read: no expected shape, the other operation's respuesta, broken consulta pagination | `Transient` | `fiscal.transport` |
  | `Refused { status }` | AEAT's door answered HTTP 301/302/401/403: the client certificate was not accepted | **`OperatorAction`** | **`fiscal.transport.refused`** |
  | `StandIn { mark }` | the answer carried the mark `refusing_stand_ins` refuses | **`OperatorAction`** | **`fiscal.transport.stand-in`** |

  A certificate that AEAT's door refused by HTTP status used to be
  `Transient`, so a retry loop kept retrying it. Retrying never fixes a
  certificate, so it now reaches an operator.
  `engine::transport::parse_http_answer(status, body)` is the single read
  edge for every transport: the built-in one and a consumer's own (an edge
  runtime's fetch, with redirects never followed).

  Migration:

  - `TransportFailure { detail }` → `TransportFailure::Wire { detail }` in
    a custom transport whose leg failed.
  - `engine::transport::parse_soap_answer(body)` (no longer public) →
    `engine::transport::parse_http_answer(status, body)`. It answers
    `Refused { status }` on 301/302/401/403 and
    `Unreadable { detail }` on a body it cannot read.
  - `failure.detail` → `failure.to_string()`. `Wire` keeps the same
    prose. An `Unreadable` built from an unreadable body names its status
    by number ("HTTP 500", no longer "HTTP 500 Internal Server Error").
  - `EmitError::Transport { detail }` → `EmitError::Transport(failure)`.
  - An exhaustive `match` on `engine::ScriptedOutcome` (feature `fakes`)
    → add an `HttpStatus(_)` arm.
  - A retry policy keyed on `class()` needs no change. One keyed on
    `code()` should treat `fiscal.transport.refused` and
    `fiscal.transport.stand-in` as the operator's.

- **A record declares its `FechaOperacion`, and the dated rules read it.**
  Before this change no record could state when its operation happened.
  Its IVA rates were checked against its own expedition date, so a
  rectificativa by substitution issued after a rate ended, for a sale made
  while that rate was in force, was refused. AEAT reads FechaOperacion
  (else FechaExpedicionFactura) for the rates and their recargo (§15.1,
  §15.3) and for the UK NIF-IVA prefix (Nota 1, 1254/1255). The crate now
  does the same. The element goes out in its SI.xsd place in every regime.
  It is in neither the huella nor the QR.

  Migration:

  - Engine face: an exhaustive `EmissionContext { .. }` literal → add
    `fecha_operacion: None`. A literal that ends in
    `..EmissionContext::default()` compiles as is.
  - Flat face: nothing to change; `InvoiceDraft::fecha_operacion(fecha)`
    is new.
  - A record that declares a FechaOperacion is checked against it. One
    that doesn't is checked exactly as before.
  - `is_nif_iva(id, fecha)` takes the operation's date: when a record
    declares a FechaOperacion, pass that date, not the issue date.

- **The consulta reads back each record's `SistemaInformatico`.** Under one
  NIF, a number issued by another system in the same format used to read
  as one's own. `ConsultaFilter::mostrar_sistema_informatico()` asks AEAT
  (`MostrarSistemaInformatico=S`), and
  `RegistroConsulta::sistema_informatico` carries the stored block as
  `SistemaInformaticoAlmacenado`: the producer, the system's name, id,
  version and installation, and its use flags. The producer's
  identification (NIF or IDOtro), `IdSistemaInformatico` and
  `NumeroInstalacion` identify an installation. `Version` changes across
  upgrades, so leave it out of the comparison. A stored block that is
  missing a required child, or holds an off-vocabulary code, makes the
  answer `Unreadable`, the same as any other required field.

  Migration:

  - An exhaustive `ConsultaFilter { .. }` literal → add
    `mostrar_sistema_informatico: false`. `ConsultaFilter::new` sets it
    false.
  - An exhaustive `RegistroConsulta { .. }` literal → add
    `sistema_informatico: None`. Its serde shape gains the field.
  - An exhaustive `engine::consulta::RegistroSpec { .. }` literal (feature
    `fakes`) → add `sistema_informatico: None`.

### Added

- `lines::RegimenEmisor`: the issuer's regime on a recargo line, `General {
  recargo_rate_cents }` (01) or `RecargoEquivalencia` (18). Because the
  recargo rate lives only on `General`, the helper cannot build a clave-18
  line with recargo amounts.
- `InvoiceDraft::fecha_operacion(FechaExpedicion)` and
  `EmissionContext::fecha_operacion`. In remisión voluntaria, the gate
  refuses what AEAT rejects:
  - more than twenty years before the record's generation (1134);
  - a date in any year after the one following the generation's (1125,
    read by the year);
  - after the generation (1173) or after the expedition (1146), unless
    every line is IVA or IGIC under `ClaveRegimen` 14 or 15. An IPSI or
    Otros line rules it out, as AEAT's codes read, though §3.1.3.1 names
    only IVA and IGIC.

  Bajo requerimiento admits all of them, because these are business rules.
- `ClaveRegimen` 14 (IVA pendiente de devengo, AAPP) is admitted under
  §15.6.9. A record with any line under 14 needs a FechaOperacion after
  the expedition (1147), an F1 or R1–R4 (1148), and every destinatario
  named by a NIF starting P, Q, S or V (1149). Until now it was refused
  because the crate could not emit a FechaOperacion. 06 stays refused:
  the crate does not emit `BaseImponibleACoste`.
- `engine::transport::parse_http_answer`, and
  `engine::ScriptedOutcome::HttpStatus(u16)` (feature `fakes`), which
  lets a hermetic suite drive a refused certificate through
  `emit_invoice` / `cancel_invoice` to the flat face.
- `ConsultaFilter::mostrar_sistema_informatico`,
  `RegistroConsulta::sistema_informatico`, `SistemaInformaticoAlmacenado`
  (crate root and `engine::consulta`), and `UsoPosible::parse` with
  `IntoEnumIterator`. `UsoPosible`, `IdentificacionDestinatario`, `IdOtro`
  and `IdOtroType` derive `Serialize` under the `serde` feature, like the
  answer that carries them.

### Fixed

- **No error echoes a party's identifier.** `check_nif` answered with the
  refused NIF, `check_destinatario` with the IDOtro's ID and CodigoPais,
  and the issuer gates printed both NIFs. An error is filed in incident
  records and logs, where data-protection law keeps a person's NIF out. A
  refusal now names the field and the rule, never the value. This holds at
  every NIF position, for IDOtro, NombreRazon, and other free text the
  caller supplies. Some things may still appear: one offending character,
  a length, a count, and the record's own fiscal content (amounts, rates,
  dates, vocabulary codes), because none of them identifies anyone.
  `response::line_note`, which builds the disposition notes, names AEAT's
  line by its NumSerieFactura and date instead of the issuer's NIF.
  No gate moved: each admits and refuses exactly what it did, and only the
  detail text changed.
- **The opt-in `Recorder` no longer prints AEAT's session.** A recording
  keeps every header verbatim, because it is evidence: that includes a
  refusal's `Location` and `Set-Cookie`. Its `Debug` now shows only the
  count of exchanges, so a struct that logs one prints no session.
- **The IVA rate is checked against the operation's date** (§15.1,
  §15.3); see FechaOperacion above.
- **0 % takes recargo 0 alone, and only for an operation from 01-01-2023
  to 30-09-2024.** The gate admitted any listed recargo on 0 % outside
  that window. AEAT rejects all of them: 1165 for recargo 0 outside the
  window, and 1277 for anything else, including the 0,26 that 1170's
  text names from 01-10-2024.
- **IPSI's `ClaveRegimen`, as AEAT applies it.** §15.6 and §4.3.1
  promise an aviso until 31-12-2026, and rejection from 2027, for an IPSI
  line outside 01, 08, 11, 18, 19 or 20, or without a clave. The gate
  admitted both until 2027, dated by the factura's expedition. AEAT splits
  them:
  - **A clave off the list** is rejected already (1246), so the gate
    refuses it now.
  - **A missing clave** is the aviso AEAT answers (2009). The gate
    admits it on a record generated by 31-12-2026 and refuses it from
    01-01-2027 (1245), dated by the record's generation, which AEAT's
    reception never precedes.
- **`ConsultaFilter` names the right period.** Its doc said a record lands
  in its expedition's month. AEAT files a record under the month of its
  FechaOperacion, else of its expedition (SW description §6.4). A restore
  that consults by month must look for a record that declares an earlier
  operation under that operation's month.

### Not shipped, on purpose

- **A `SistemaInformatico` filter on the consulta.** AEAT resolves the
  filter against its registry of systems and refuses an ambiguous one
  (1285, "debe filtrar la consulta por más campos del Sistema
  Informático"). Only a live run can say which fields make two systems
  one, for example whether a new `Version` registers a new system. A wrong
  guess either drops the records of earlier versions or fails every
  consulta. The read-back above answers the same question exactly.

### Live-confirmed at AEAT pruebas, 2026-10-22

The bench (`bench/`, outside git) ran this rev's every new shape and
every reading against AEAT. Each record went as its own installation's
first record, refused ones sent past the gate, and AEAT agreed with the
gate on every row:

| Question | AEAT |
|---|---|
| R1 sustitutiva at 5 % for an operation of 30-09-2024 | Correcto; without FechaOperacion, 1194 |
| FechaOperacion before the expedition, at 21 % | Correcto |
| Clave 14 to Q2826000H, later FechaOperacion | Correcto |
| Clave 15, later FechaOperacion within the next year | Correcto |
| Clave 15, FechaOperacion on 31-12 of the next year / 01-01 of the year after | Correcto / 1125 |
| IPSI-only record, or IVA-15 + IPSI, with a later FechaOperacion | 1146 |
| 18 % for an operation of 2011 | 1124 |
| 0 % with recargo 0, for an operation of 15-06-2024 / of today | Correcto / 1165 |
| 0 % with recargo 0,5 or 0,26, today or for 15-11-2024 | 1277 |
| IPSI under 01 / under 03 or 14 / without a clave | Correcto / 1246 / AceptadoConErrores 2009 |
| `MostrarSistemaInformatico=S` on a record of October for an operation of January | found under January only; the block names the installation that sent it, before and after its anulación |
| The door without a client certificate | HTTP 302 → `Refused { status: 302 }`, `fiscal.transport.refused`, no URL in the error |

The six earlier stages (sweep, saga, remission, requerimiento, facade,
consulta) passed on the same rev.

### Known edges

- **Pruebas is the evidence.** For IPSI's clave list, pruebas rejects what
  §15.6 promises production only warns about until 2027. The gate follows
  pruebas.
- **The generation stands for AEAT's reception.** A record received
  days after its generation can be rejected for a date the gate
  admitted. That applies to 1134 (an operation no more than twenty years
  back) and to a missing IPSI clave around 01-01-2027. Each gate's
  comment says which way it errs.
