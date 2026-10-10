//! Every byte the crate puts on the wire validates against AEAT's own
//! schemas (`contracts/aeat-verifactu/`): whatever input the gates
//! ADMIT serializes schema-valid, so an off-contract shape is refused
//! here, before a byte leaves — never by AEAT after the record is
//! committed. The inputs are generated across the whole field space,
//! including the text the live matrix never tries (every Unicode
//! plane, markup, controls, over-long values).

use std::sync::{Arc, OnceLock};

use proptest::prelude::*;
use strum::IntoEnumIterator;
use uppsala::XsdValidator;
use verifactu::engine::consulta::{
    consulta_response_document, ClavePaginacion, ConsultaRequest, ConsultaSpec, EncadenamientoSpec,
    RegistroSpec, ResultadoConsulta,
};
use verifactu::engine::response::{EstadoDuplicado, EstadoEnvio, EstadoRegistro, TipoOperacion};
use verifactu::engine::transport::{respuesta_document, DuplicadoSpec, LineaSpec, RespuestaSpec};
use verifactu::engine::{
    ChainRecord, EmissionContext, FakeSigner, FakeVerifactuTransport, PrevRef, RechazoPrevio,
    Requerimiento, SiNo, SistemaInformaticoConfig, VerifactuEmitter,
};
use verifactu::{
    CalificacionOperacion, ChainKind, ClaveRegimen, ConsultaFilter, DetalleDesglose,
    EstadoAlmacenado, FacturaId, FechaExpedicion, FechaHuso, IDDestinatario, IdOtro, IdOtroType,
    IdentificacionDestinatario, ImporteRectificacion, Impuesto, Mes, Modality, Money, Obligado,
    OperacionExenta, Predecessor, Series, TipoFactura, TipoRectificativa,
};

// ---- the oracle --------------------------------------------------------

fn schema(file: &'static str, cell: &'static OnceLock<XsdValidator>) -> &'static XsdValidator {
    cell.get_or_init(|| {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/aeat-verifactu");
        let xsd = std::fs::read_to_string(dir.join(file)).expect("the vendored schema reads");
        let document = uppsala::parse(&xsd).expect("the vendored schema parses");
        XsdValidator::from_schema_with_base_path(&document, Some(&dir))
            .expect("the vendored schema builds")
    })
}

fn suministro_lr() -> &'static XsdValidator {
    static CELL: OnceLock<XsdValidator> = OnceLock::new();
    schema("SuministroLR.xsd", &CELL)
}

fn consulta_lr() -> &'static XsdValidator {
    static CELL: OnceLock<XsdValidator> = OnceLock::new();
    schema("ConsultaLR.xsd", &CELL)
}

fn respuesta_suministro() -> &'static XsdValidator {
    static CELL: OnceLock<XsdValidator> = OnceLock::new();
    schema("RespuestaSuministro.xsd", &CELL)
}

fn respuesta_consulta_lr() -> &'static XsdValidator {
    static CELL: OnceLock<XsdValidator> = OnceLock::new();
    schema("RespuestaConsultaLR.xsd", &CELL)
}

/// Well-formedness on the document as sent; validity on a copy whose
/// non-ASCII characters are widened to one `x` per UTF-16 code unit.
/// uppsala's length facets count UTF-8 bytes, where XSD counts
/// characters and AEAT's Java validator UTF-16 units — the widening
/// makes the oracle count the units the crate's gates enforce (a
/// non-ASCII character never satisfies an enumeration or a digit
/// pattern, before or after).
fn assert_schema_valid(validator: &XsdValidator, document: &str) {
    uppsala::parse(document)
        .unwrap_or_else(|error| panic!("not well-formed XML ({error}): {document}"));
    let widened: String = document
        .chars()
        .map(|ch| {
            if ch.is_ascii() {
                ch.to_string()
            } else {
                "x".repeat(ch.len_utf16())
            }
        })
        .collect();
    let parsed = uppsala::parse(&widened).expect("the widened copy stays well-formed");
    let violations = validator.validate(&parsed);
    assert!(
        violations.is_empty(),
        "schema violations {violations:?} in {document}"
    );
}

/// The SOAP body's payload, with any record signature cut out: the
/// signature's own shape is the `XAdES` suite's to pin (the schemas
/// import xmldsig by a remote location this oracle never fetches).
fn payload_of(envelope: &str) -> String {
    let start = envelope.find("<soapenv:Body>").expect("a SOAP body") + "<soapenv:Body>".len();
    let end = envelope
        .rfind("</soapenv:Body>")
        .expect("a closed SOAP body");
    let mut payload = envelope[start..end].to_owned();
    while let Some(at) = payload.find("<ds:Signature") {
        let close = payload[at..]
            .find("</ds:Signature>")
            .expect("a closed signature")
            + at
            + "</ds:Signature>".len();
        payload.replace_range(at..close, "");
    }
    payload
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime")
        .block_on(future)
}

// ---- the input space ---------------------------------------------------

/// One field in about this many is generated off-contract: rare per
/// field, so most records are wholly valid and reach the oracle, while
/// across a record's dozens of fields the gates still see a steady
/// stream of single defects.
const WILD: u32 = 60;

/// Text within `max` UTF-16 units — ASCII, Spanish letters, markup,
/// line breaks, emoji — or, rarely, wild: any Unicode scalar (controls
/// and non-characters included), up to `max + 5` chars.
fn text(max: usize) -> BoxedStrategy<String> {
    let clean = prop_oneof![
        6 => proptest::char::range(' ', '~'),
        2 => proptest::sample::select(vec!['ñ', 'Á', 'ü', '€', 'ç', '·', 'ª']),
        1 => proptest::sample::select(vec!['&', '<', '>', '"', '\'', '\r', '\n', '\t']),
        1 => proptest::char::range('\u{1F300}', '\u{1FAFF}'),
    ];
    let clean = proptest::collection::vec(clean, 0..=max).prop_map(move |chars| {
        let mut out = String::new();
        for ch in chars {
            if out.encode_utf16().count() + ch.len_utf16() > max {
                break;
            }
            out.push(ch);
        }
        out
    });
    let wild = proptest::collection::vec(proptest::char::any(), 0..=max + 5)
        .prop_map(|chars| chars.into_iter().collect());
    prop_oneof![WILD => clean, 1 => wild].boxed()
}

/// A NIF: well-formed but for the rare wild one.
fn nif() -> BoxedStrategy<String> {
    prop_oneof![
        WILD => "[0-9]{8}[A-Z]",
        1 => text(11),
    ]
    .boxed()
}

fn obligado() -> impl Strategy<Value = Obligado> {
    (text(120), nif()).prop_map(|(nombre_razon, nif)| Obligado { nombre_razon, nif })
}

fn money(max_cents: i64) -> impl Strategy<Value = Money> {
    (-max_cents..=max_cents).prop_map(Money::from_cents)
}

/// Cents spanning the in-bound amounts and the 13-integer-digit edge.
fn importe() -> BoxedStrategy<Money> {
    prop_oneof![
        WILD => money(10_000_000),
        1 => money(100_000_000_000_000),
    ]
    .boxed()
}

fn rate() -> BoxedStrategy<Option<Money>> {
    proptest::option::of(prop_oneof![
        WILD => (0_i64..=10_000).prop_map(Money::from_cents),
        1 => (-100_i64..=200_000).prop_map(Money::from_cents),
    ])
    .boxed()
}

fn pick<E: IntoEnumIterator + Clone + std::fmt::Debug + 'static>() -> impl Strategy<Value = E> {
    proptest::sample::select(E::iter().collect::<Vec<_>>())
}

fn detalle() -> impl Strategy<Value = DetalleDesglose> {
    (
        proptest::option::of(pick::<Impuesto>()),
        proptest::option::weighted(0.99, pick::<ClaveRegimen>()),
        prop_oneof![
            WILD => pick::<CalificacionOperacion>().prop_map(|c| (Some(c), None)),
            WILD => pick::<OperacionExenta>().prop_map(|e| (None, Some(e))),
            1 => Just((None, None)),
        ],
        rate(),
        importe(),
        proptest::option::of(importe()),
        rate(),
        proptest::option::of(importe()),
    )
        .prop_map(
            |(
                impuesto,
                clave_regimen,
                (calificacion, operacion_exenta),
                tipo,
                base,
                cuota,
                tipo_re,
                cuota_re,
            )| {
                DetalleDesglose {
                    impuesto,
                    clave_regimen,
                    calificacion,
                    operacion_exenta,
                    tipo_impositivo: tipo,
                    base_imponible: base,
                    cuota_repercutida: cuota,
                    tipo_recargo_equivalencia: tipo_re,
                    cuota_recargo_equivalencia: cuota_re,
                }
            },
        )
}

fn destinatario() -> impl Strategy<Value = IDDestinatario> {
    let codigo_pais = prop_oneof![
        8 => Just(None),
        8 => proptest::sample::select(verifactu::engine::xml::CODIGOS_PAIS.to_vec())
            .prop_map(|c| Some(c.to_owned())),
        2 => "[A-Za-z]{2}".prop_map(Some),
        1 => text(3).prop_map(Some),
    ];
    let identificacion = prop_oneof![
        nif().prop_map(IdentificacionDestinatario::Nif),
        (codigo_pais, pick::<IdOtroType>(), text(20)).prop_map(|(codigo_pais, id_type, id)| {
            IdentificacionDestinatario::Otro(IdOtro {
                codigo_pais,
                id_type,
                id,
            })
        }),
    ];
    (text(120), identificacion).prop_map(|(nombre_razon, identificacion)| IDDestinatario {
        nombre_razon,
        identificacion,
    })
}

fn fecha() -> impl Strategy<Value = FechaExpedicion> {
    (1_u32..=28, 1_u32..=12, 2024_u32..=2030).prop_map(|(d, m, y)| {
        FechaExpedicion::parse(&format!("{d:02}-{m:02}-{y}")).expect("dd-mm-yyyy")
    })
}

/// The builtin series, a plausible custom prefix, or (rarely) any text.
fn series() -> BoxedStrategy<Series> {
    prop_oneof![
        WILD => proptest::sample::select(vec![Series::T, Series::F, Series::R]),
        WILD => "[A-Z0-9][A-Z0-9/ ._-]{0,40}[A-Z0-9]".prop_map(Series::Custom),
        1 => text(14).prop_map(Series::Custom),
    ]
    .boxed()
}

fn num_serie() -> BoxedStrategy<String> {
    prop_oneof![
        WILD => "[A-Z0-9][A-Z0-9/ ._-]{0,50}[0-9]",
        1 => text(62),
    ]
    .boxed()
}

fn prev() -> impl Strategy<Value = Option<Predecessor>> {
    proptest::option::of(
        (nif(), series(), 1_u64..=99_999_999, fecha(), "[0-9A-F]{64}").prop_map(
            |(issuer, serie, number, fecha_expedicion, huella)| {
                Predecessor::Factura(PrevRef {
                    issuer,
                    serie,
                    number,
                    fecha_expedicion,
                    huella,
                })
            },
        ),
    )
}

type Rectificacion = (
    Option<TipoRectificativa>,
    Vec<FacturaId>,
    Option<ImporteRectificacion>,
);

fn importe_rectificacion() -> impl Strategy<Value = ImporteRectificacion> {
    (importe(), importe(), proptest::option::of(importe())).prop_map(
        |(base_rectificada, cuota_rectificada, cuota_recargo_rectificado)| ImporteRectificacion {
            base_rectificada,
            cuota_rectificada,
            cuota_recargo_rectificado,
        },
    )
}

/// The rectification block the tipo demands (one case in ten: any).
fn rectificacion(tipo: TipoFactura) -> BoxedStrategy<Rectificacion> {
    let factura = (nif(), num_serie(), fecha())
        .prop_map(|(nif, num_serie, fecha_expedicion)| FacturaId {
            nif,
            num_serie,
            fecha_expedicion,
        })
        .boxed();
    let arbitrary = (
        proptest::option::of(pick::<TipoRectificativa>()),
        proptest::collection::vec(factura.clone(), 0..=2),
        proptest::option::of(importe_rectificacion()),
    );
    let coherent = if tipo.is_rectificativa() {
        pick::<TipoRectificativa>()
            .prop_flat_map(move |tipo_rect| {
                let importe = match tipo_rect {
                    TipoRectificativa::Sustitutiva => {
                        importe_rectificacion().prop_map(Some).boxed()
                    }
                    TipoRectificativa::Incremental => Just(None).boxed(),
                };
                (
                    Just(Some(tipo_rect)),
                    proptest::collection::vec(factura.clone(), 1..=2),
                    importe,
                )
            })
            .boxed()
    } else {
        Just((None, Vec::new(), None)).boxed()
    };
    prop_oneof![WILD => coherent, 1 => arbitrary].boxed()
}

/// Everything an alta's emission context borrows, owned.
#[derive(Debug, Clone)]
struct AltaInput {
    tipo: TipoFactura,
    serie: Series,
    number: u64,
    cuota_total: Money,
    importe_total: Money,
    descripcion: Option<String>,
    ref_externa: Option<String>,
    lines: Vec<DetalleDesglose>,
    destinatarios: Vec<IDDestinatario>,
    tipo_rectificativa: Option<TipoRectificativa>,
    rectificadas: Vec<FacturaId>,
    importe_rectificacion: Option<ImporteRectificacion>,
    subsanacion: Option<SiNo>,
    rechazo_previo: Option<RechazoPrevio>,
    macrodato: Option<SiNo>,
}

fn alta_input() -> impl Strategy<Value = AltaInput> {
    pick::<TipoFactura>()
        .prop_flat_map(|tipo| {
            let destinatarios = if tipo.requires_destinatario() {
                prop_oneof![
                    WILD => proptest::collection::vec(destinatario(), 1..=2),
                    1 => proptest::collection::vec(destinatario(), 0..=2),
                ]
                .boxed()
            } else {
                proptest::collection::vec(destinatario(), 0..=2).boxed()
            };
            let lines = prop_oneof![
                WILD => proptest::collection::vec(detalle(), 1..=4),
                1 => proptest::collection::vec(detalle(), 0..=13),
            ];
            let flags = (
                proptest::option::of(pick_si_no()),
                proptest::option::of(pick::<RechazoPrevio>()),
                proptest::option::of(pick_si_no()),
            );
            (
                (
                    Just(tipo),
                    series(),
                    1_u64..=99_999_999,
                    importe(),
                    importe(),
                ),
                (
                    proptest::option::weighted(0.99, text(500)),
                    proptest::option::of(text(60)),
                ),
                lines,
                destinatarios,
                rectificacion(tipo),
                flags,
            )
        })
        .prop_map(
            |(
                (tipo, serie, number, cuota_total, importe_total),
                (descripcion, ref_externa),
                lines,
                destinatarios,
                (tipo_rectificativa, rectificadas, importe_rectificacion),
                (subsanacion, rechazo_previo, macrodato),
            )| AltaInput {
                tipo,
                serie,
                number,
                cuota_total,
                importe_total,
                descripcion,
                ref_externa,
                lines,
                destinatarios,
                tipo_rectificativa,
                rectificadas,
                importe_rectificacion,
                subsanacion,
                rechazo_previo,
                macrodato,
            },
        )
}

fn pick_si_no() -> impl Strategy<Value = SiNo> {
    proptest::sample::select(vec![SiNo::Si, SiNo::No])
}

fn sif(obligado: &Obligado, version: String) -> SistemaInformaticoConfig {
    SistemaInformaticoConfig {
        nombre_razon: obligado.nombre_razon.clone(),
        nif: obligado.nif.clone(),
        nombre_sistema_informatico: String::from("mi-facturacion"),
        id_sistema_informatico: String::from("MF"),
        version,
        numero_instalacion: String::from("000042"),
    }
}

/// The record the generated input seals (issuer = the obligado, so the
/// identity gate is not what every case trips on).
fn seal(
    input: &AltaInput,
    issuer: &str,
    anulacion: bool,
    prev: Option<&Predecessor>,
) -> ChainRecord {
    let fecha_expedicion = FechaExpedicion::parse("05-10-2026").expect("dd-mm-yyyy");
    let fecha_huso_gen = FechaHuso::parse("2026-10-05T12:00:00+02:00").expect("ISO-8601");
    let kind = if anulacion {
        ChainKind::Anulacion {
            issuer: issuer.to_owned(),
            serie: input.serie.clone(),
            number: input.number,
            fecha_expedicion,
            fecha_huso_gen,
        }
    } else {
        ChainKind::Alta {
            issuer: issuer.to_owned(),
            serie: input.serie.clone(),
            number: input.number,
            fecha_expedicion,
            tipo_factura: input.tipo,
            cuota_total: input.cuota_total,
            importe_total: input.importe_total,
            fecha_huso_gen,
        }
    };
    ChainRecord::seal(kind, 1_791_201_600, prev)
}

// ---- the properties ----------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Whatever the emitter admits, it remits schema-valid: the
    /// `RegFactuSistemaFacturacion` it sends validates against
    /// SuministroLR.xsd (and through it SuministroInformacion.xsd), in
    /// both modalities, with and without a Representante.
    #[test]
    fn every_admitted_envio_validates_against_suministro_lr(
        input in alta_input(),
        obligado in obligado(),
        representante in proptest::option::of(obligado()),
        version in text(50),
        anulacion in proptest::bool::weighted(0.2),
        prev in prev(),
        conservation in proptest::bool::ANY,
        referencia in "[A-Za-z0-9]{1,18}",
        incidencia in proptest::bool::ANY,
    ) {
        let Ok(emitter) = VerifactuEmitter::new(
            if conservation { Modality::Conservation } else { Modality::Remission },
            obligado.clone(),
            sif(&obligado, version),
            Arc::new(FakeSigner::new()),
        ) else {
            return Ok(());
        };
        let emitter = match representante {
            Some(representante) => match emitter.with_representante(representante) {
                Ok(emitter) => emitter,
                Err(_) => return Ok(()),
            },
            None => emitter,
        };
        let transport = Arc::new(FakeVerifactuTransport::new());
        let emitter = emitter.with_transport(Arc::clone(&transport) as _);
        let record = seal(&input, &obligado.nif, anulacion, prev.as_ref());
        let context = EmissionContext {
            ref_externa: input.ref_externa.as_deref(),
            descripcion_operacion: input.descripcion.as_deref(),
            desglose: &input.lines,
            tipo_rectificativa: input.tipo_rectificativa,
            facturas_rectificadas: &input.rectificadas,
            importe_rectificacion: input.importe_rectificacion.clone(),
            destinatarios: &input.destinatarios,
            subsanacion: input.subsanacion,
            rechazo_previo: input.rechazo_previo,
            macrodato: input.macrodato,
            incidencia: incidencia && !conservation,
            requerimiento: conservation.then_some(Requerimiento { referencia, fin: true }),
            ..EmissionContext::default()
        };
        if block_on(emitter.submit(&[(record, context)])).is_err() {
            return Ok(());
        }
        let sent = transport.sent_envelopes();
        prop_assert_eq!(sent.len(), 1);
        assert_schema_valid(suministro_lr(), &payload_of(&sent[0]));
    }

    /// Whatever consulta the emitter admits validates against
    /// ConsultaLR.xsd.
    #[test]
    fn every_admitted_consulta_validates_against_consulta_lr(
        obligado in obligado(),
        apoderado in proptest::bool::ANY,
        ejercicio in prop_oneof![2024_u16..=2030, 0_u16..=u16::MAX],
        mes in 1_u8..=12,
        num_serie in proptest::option::of(num_serie()),
        ref_externa in proptest::option::of(text(60)),
        clave in proptest::option::of((nif(), num_serie(), "[0-9]{2}-[0-9]{2}-[0-9]{4}")),
    ) {
        let transport = Arc::new(FakeVerifactuTransport::new());
        let Ok(emitter) = VerifactuEmitter::new(
            Modality::Remission,
            obligado.clone(),
            sif(&obligado, String::from("1.0")),
            Arc::new(FakeSigner::new()),
        ) else {
            return Ok(());
        };
        let emitter = emitter.with_transport(Arc::clone(&transport) as _);
        let mut filtro = ConsultaFilter::new(ejercicio, Mes::try_new(mes).expect("1..=12"));
        filtro.num_serie = num_serie.as_deref();
        filtro.ref_externa = ref_externa.as_deref();
        let clave = clave.map(|(id_emisor_factura, num_serie_factura, fecha_expedicion_factura)| {
            ClavePaginacion { id_emisor_factura, num_serie_factura, fecha_expedicion_factura }
        });
        let request = ConsultaRequest {
            obligado: &obligado,
            apoderado,
            filtro: &filtro,
            clave_paginacion: clave.as_ref(),
        };
        if block_on(emitter.consulta_with(&request)).is_err() {
            return Ok(());
        }
        assert_schema_valid(consulta_lr(), &payload_of(&transport.sent_envelopes()[0]));
    }

    /// The fake's submission answers are AEAT-shaped: every respuesta
    /// it can be scripted to render validates against
    /// RespuestaSuministro.xsd — a hermetic test never rides a shape
    /// the real AEAT could not send.
    #[test]
    fn the_fakes_respuesta_documents_validate_against_respuesta_suministro(
        csv in proptest::option::of("[A-Z0-9]{16}"),
        tiempo_espera_envio in 0_u32..=9999,
        estado_envio in pick::<EstadoEnvio>(),
        lineas in proptest::collection::vec(
            (
                "[0-9]{8}[A-Z]",
                "[A-Z]{1,4}[0-9]{8}",
                pick::<TipoOperacion>(),
                proptest::option::of("[A-Za-z0-9-]{1,20}"),
                pick::<EstadoRegistro>(),
                proptest::option::of((1000_i64..=9999, "[a-z ]{1,40}")),
                proptest::option::of(("[0-9]{1,20}", pick::<EstadoDuplicado>())),
            ),
            0..=3,
        ),
    ) {
        let lineas = lineas
            .into_iter()
            .map(|(nif, num_serie, tipo_operacion, ref_externa, estado_registro, error, duplicado)| {
                LineaSpec {
                    nif,
                    num_serie,
                    fecha: String::from("05-10-2026"),
                    tipo_operacion,
                    ref_externa,
                    estado_registro,
                    codigo_error: error.as_ref().map(|(codigo, _)| *codigo),
                    descripcion_error: error.map(|(_, descripcion)| descripcion),
                    duplicado: duplicado.map(|(id_peticion, estado_duplicado)| DuplicadoSpec {
                        id_peticion,
                        estado_duplicado,
                        codigo_error: None,
                        descripcion_error: None,
                    }),
                }
            })
            .collect();
        let document = respuesta_document(&RespuestaSpec {
            obligado_nombre: "ANA RUIZ PEREZ",
            obligado_nif: "12345678Z",
            csv: csv.as_deref(),
            tiempo_espera_envio,
            estado_envio,
            lineas,
        });
        assert_schema_valid(respuesta_suministro(), &document);
    }

    /// ...and the consulta answers validate against
    /// RespuestaConsultaLR.xsd.
    #[test]
    fn the_fakes_consulta_documents_validate_against_respuesta_consulta_lr(
        apoderado in proptest::bool::ANY,
        mes in 1_u8..=12,
        resultado in prop_oneof![Just(ResultadoConsulta::ConDatos), Just(ResultadoConsulta::SinDatos)],
        paginacion_pendiente in proptest::bool::ANY,
        registros in proptest::collection::vec(
            (
                "[A-Z]{1,4}[0-9]{8}",
                pick::<EstadoAlmacenado>(),
                proptest::option::of((1000_i64..=9999, "[a-z ]{1,40}")),
                proptest::option::of((pick::<TipoFactura>(), money(1_000_000), money(1_000_000))),
                proptest::option::of("[0-9A-F]{64}"),
                proptest::option::of("[0-9A-F]{64}"),
            ),
            0..=3,
        ),
    ) {
        let registros = registros
            .into_iter()
            .map(|(num_serie_factura, estado, error, montos, previa, huella)| RegistroSpec {
                id_emisor_factura: String::from("12345678Z"),
                num_serie_factura,
                fecha_expedicion_factura: String::from("05-10-2026"),
                estado,
                timestamp_ultima_modificacion: String::from("2026-10-05T12:00:00+02:00"),
                codigo_error: error.as_ref().map(|(codigo, _)| *codigo),
                descripcion_error: error.map(|(_, descripcion)| descripcion),
                tipo_factura: montos.map(|(tipo, _, _)| tipo),
                cuota_total: montos.map(|(_, cuota, _)| cuota),
                importe_total: montos.map(|(_, _, importe)| importe),
                fecha_huso_gen: Some(String::from("2026-10-05T12:00:00+02:00")),
                huella_previa: previa.map(|huella| EncadenamientoSpec {
                    num_serie_factura: String::from("T00000001"),
                    fecha_expedicion_factura: String::from("04-10-2026"),
                    huella,
                }),
                huella,
            })
            .collect();
        let document = consulta_response_document(&ConsultaSpec {
            obligado_nombre: String::from("ANA RUIZ PEREZ"),
            obligado_nif: String::from("12345678Z"),
            apoderado,
            ejercicio: 2026,
            mes: Mes::try_new(mes).expect("1..=12"),
            resultado,
            paginacion_pendiente,
            clave_paginacion: paginacion_pendiente.then(|| ClavePaginacion {
                id_emisor_factura: String::from("12345678Z"),
                num_serie_factura: String::from("T00000009"),
                fecha_expedicion_factura: String::from("05-10-2026"),
            }),
            registros,
        });
        assert_schema_valid(respuesta_consulta_lr(), &document);
    }
}
