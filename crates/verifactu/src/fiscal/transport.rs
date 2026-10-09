//! The transport port and its stateful fake. Endpoint family:
//! `sfVerifactu` (remission) and `sfRequerimiento` (conservation bajo
//! requerimiento) — same XSDs, different endpoints; selection is the
//! caller's configuration.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use crate::fiscal::response::{
    parse_fault, parse_respuesta, EstadoDuplicado, EstadoEnvio, EstadoRegistro,
    RespuestaSuministro, SoapFault, TipoOperacion,
};
use crate::fiscal::xml::{
    sent_request, tag, SentRecord, NS_RESPUESTA, NS_SOAP_ENV, NS_SUMINISTRO, P_ENV, P_RS,
};

#[derive(Debug)]
pub enum TransportOutcome {
    Response(RespuestaSuministro),
    /// The consulta operation's payload root
    /// (`RespuestaConsultaFactuSistemaFacturacion`) — same read edge,
    /// different respuesta vocabulary.
    Consulta(crate::fiscal::consulta::ConsultaAnswer),
    Fault(SoapFault),
}

/// Always the `Transient` class: the outbox retries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportFailure {
    pub detail: String,
}

/// Boxed (not RPITIT) to keep the trait dyn-compatible. The `+ Send`
/// bound serves multi-threaded hosts (tokio, axum handlers); the
/// `edge` feature drops it so single-threaded runtimes (wasm32: a JS
/// `Promise` is structurally `!Send`) implement the port directly over
/// their runtime's fetch — no spawn/oneshot bridge at the consumer.
#[cfg(not(feature = "edge"))]
pub type SendFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TransportOutcome, TransportFailure>> + Send + 'a>>;

#[cfg(feature = "edge")]
pub type SendFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TransportOutcome, TransportFailure>> + 'a>>;

pub trait VerifactuTransport: Send + Sync {
    fn send<'a>(&'a self, soap_envelope: &'a str) -> SendFuture<'a>;
}

/// The production wire's read edge: `Body/Fault` (any HTTP status) or
/// `Body/RespuestaRegFactuSistemaFacturacion`; the fake's bare-root
/// documents parse through the same edge — one read law, offline and
/// live.
///
/// # Errors
/// A prose `Err` when the document carries neither shape — the
/// `Transient` class: an unparseable answer is surfaced, never guessed
/// at.
pub fn parse_soap_answer(xml: &str) -> Result<TransportOutcome, String> {
    let root = crate::fiscal::xml::parse_tree(xml)?;
    let body = if root.local == "Envelope" {
        Some(root.child("Body").ok_or("SOAP envelope without Body")?)
    } else {
        None
    };
    let fault = if root.local == "Fault" {
        Some(&root)
    } else {
        body.and_then(|body| body.child("Fault"))
    };
    if let Some(fault) = fault {
        return Ok(TransportOutcome::Fault(
            crate::fiscal::response::fault_of_tree(fault)?,
        ));
    }
    let payload = if let Some(body) = body {
        body.children
            .iter()
            .find(|node| {
                node.local == "RespuestaRegFactuSistemaFacturacion"
                    || node.local == "RespuestaConsultaFactuSistemaFacturacion"
            })
            .ok_or("SOAP Body carries neither RespuestaRegFactuSistemaFacturacion nor RespuestaConsultaFactuSistemaFacturacion")?
    } else if root.local == "RespuestaRegFactuSistemaFacturacion" {
        &root
    } else if root.local == "RespuestaConsultaFactuSistemaFacturacion" {
        return Ok(TransportOutcome::Consulta(
            crate::fiscal::consulta::consulta_of_tree(&root)?,
        ));
    } else {
        return Err(format!(
            "root element is {}, not a SOAP envelope, Fault, or either respuesta",
            root.local
        ));
    };
    if payload.local == "RespuestaConsultaFactuSistemaFacturacion" {
        return Ok(TransportOutcome::Consulta(
            crate::fiscal::consulta::consulta_of_tree(payload)?,
        ));
    }
    Ok(TransportOutcome::Response(
        crate::fiscal::response::respuesta_of_tree(payload)?,
    ))
}

#[derive(Debug, Clone)]
pub enum ScriptedOutcome {
    Correcto {
        csv: Option<String>,
        tiempo_espera_envio: u32,
    },
    /// The line blocks echo the records actually sent — the fake parses
    /// the envelope.
    Lineas {
        csv: Option<String>,
        tiempo_espera_envio: u32,
        estado_envio: EstadoEnvio,
        lineas: Vec<ScriptedLine>,
    },
    /// The wholesale envío-level rejection: `EstadoEnvio=Incorrecto`
    /// with ZERO `RespuestaLinea` blocks — no per-line echo is
    /// fabricated (`Lineas` pads short scripts with `Correcto`, so this
    /// shape needs its own variant; an outbox suite scripts it as the
    /// regulatory-surface path).
    EnvioIncorrecto {
        tiempo_espera_envio: u32,
    },
    FaultServer(String),
    FaultClient(String),
    Unreachable,
}

#[derive(Debug, Clone)]
pub enum ScriptedLine {
    Correcto,
    AceptadoConErrores {
        codigo: i64,
        descripcion: String,
    },
    Incorrecto {
        codigo: i64,
        descripcion: String,
    },
    Duplicado {
        codigo: i64,
        id_peticion: String,
        estado_duplicado: EstadoDuplicado,
    },
}

// Response-side document builders. Every AEAT-vocabulary field carries
// the ENUM — an off-vocabulary byte cannot even be scripted.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineaSpec {
    pub nif: String,
    pub num_serie: String,
    pub fecha: String,
    pub tipo_operacion: TipoOperacion,
    pub ref_externa: Option<String>,
    pub estado_registro: EstadoRegistro,
    pub codigo_error: Option<i64>,
    pub descripcion_error: Option<String>,
    pub duplicado: Option<DuplicadoSpec>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicadoSpec {
    pub id_peticion: String,
    pub estado_duplicado: EstadoDuplicado,
    pub codigo_error: Option<i64>,
    pub descripcion_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RespuestaSpec<'a> {
    pub obligado_nombre: &'a str,
    pub obligado_nif: &'a str,
    pub csv: Option<&'a str>,
    pub tiempo_espera_envio: u32,
    pub estado_envio: EstadoEnvio,
    pub lineas: Vec<LineaSpec>,
}

/// Namespace routing per RS.xsd: it is `elementFormDefault="qualified"`,
/// so every element declared locally in RS.xsd carries the RS namespace —
/// only the elements whose TYPES live in SI.xsd carry the SI namespace.
#[must_use]
pub fn respuesta_document(spec: &RespuestaSpec<'_>) -> String {
    let mut out = String::with_capacity(1024);
    out.push('<');
    out.push_str(P_RS);
    out.push_str(":RespuestaRegFactuSistemaFacturacion xmlns:sfR=\"");
    out.push_str(NS_RESPUESTA);
    out.push_str("\" xmlns:sum1=\"");
    out.push_str(NS_SUMINISTRO);
    out.push_str("\">");
    if let Some(csv) = spec.csv {
        tag(&mut out, "sfR:CSV", csv);
    }
    out.push_str("<sfR:Cabecera>");
    out.push_str("<sum1:ObligadoEmision>");
    tag(&mut out, "sum1:NombreRazon", spec.obligado_nombre);
    tag(&mut out, "sum1:NIF", spec.obligado_nif);
    out.push_str("</sum1:ObligadoEmision>");
    out.push_str("</sfR:Cabecera>");
    tag(
        &mut out,
        "sfR:TiempoEsperaEnvio",
        &spec.tiempo_espera_envio.to_string(),
    );
    tag(&mut out, "sfR:EstadoEnvio", spec.estado_envio.as_str());
    for linea in &spec.lineas {
        out.push_str("<sfR:RespuestaLinea>");
        out.push_str("<sfR:IDFactura>");
        tag(&mut out, "sum1:IDEmisorFactura", &linea.nif);
        tag(&mut out, "sum1:NumSerieFactura", &linea.num_serie);
        tag(&mut out, "sum1:FechaExpedicionFactura", &linea.fecha);
        out.push_str("</sfR:IDFactura>");
        out.push_str("<sfR:Operacion>");
        tag(
            &mut out,
            "sum1:TipoOperacion",
            linea.tipo_operacion.as_str(),
        );
        out.push_str("</sfR:Operacion>");
        if let Some(ref_ext) = &linea.ref_externa {
            tag(&mut out, "sfR:RefExterna", ref_ext);
        }
        tag(
            &mut out,
            "sfR:EstadoRegistro",
            linea.estado_registro.as_str(),
        );
        if let Some(codigo) = linea.codigo_error {
            tag(&mut out, "sfR:CodigoErrorRegistro", &codigo.to_string());
        }
        if let Some(descripcion) = &linea.descripcion_error {
            tag(&mut out, "sfR:DescripcionErrorRegistro", descripcion);
        }
        if let Some(dup) = &linea.duplicado {
            out.push_str("<sfR:RegistroDuplicado>");
            tag(
                &mut out,
                "sum1:IdPeticionRegistroDuplicado",
                &dup.id_peticion,
            );
            tag(
                &mut out,
                "sum1:EstadoRegistroDuplicado",
                dup.estado_duplicado.as_str(),
            );
            if let Some(codigo) = dup.codigo_error {
                tag(&mut out, "sum1:CodigoErrorRegistro", &codigo.to_string());
            }
            if let Some(descripcion) = &dup.descripcion_error {
                tag(&mut out, "sum1:DescripcionErrorRegistro", descripcion);
            }
            out.push_str("</sfR:RegistroDuplicado>");
        }
        out.push_str("</sfR:RespuestaLinea>");
    }
    out.push_str("</sfR:RespuestaRegFactuSistemaFacturacion>");
    out
}

#[must_use]
pub fn fault_document(faultcode: &str, faultstring: &str) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    out.push('<');
    out.push_str(P_ENV);
    out.push_str(":Envelope xmlns:soapenv=\"");
    out.push_str(NS_SOAP_ENV);
    out.push_str("\">");
    out.push_str("<soapenv:Body>");
    out.push_str("<soapenv:Fault>");
    tag(&mut out, "faultcode", faultcode);
    tag(&mut out, "faultstring", faultstring);
    out.push_str("</soapenv:Fault>");
    out.push_str("</soapenv:Body>");
    out.push_str("</soapenv:Envelope>");
    out
}

/// A dry script answers a plain `Correcto`.
#[derive(Debug, Default)]
pub struct FakeVerifactuTransport {
    sent: Mutex<Vec<String>>,
    script: Mutex<VecDeque<ScriptedOutcome>>,
    consulta_script: Mutex<VecDeque<crate::fiscal::consulta::ConsultaSpec>>,
}

impl FakeVerifactuTransport {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Interior state: an `Arc`-shared fake can be scripted after wiring.
    ///
    /// # Panics
    /// If the script lock is poisoned.
    pub fn push_outcome(&self, outcome: ScriptedOutcome) {
        self.script
            .lock()
            .expect("fake transport script lock")
            .push_back(outcome);
    }

    /// The consulta half's script: every sent consulta envelope answers
    /// the next spec (a dry script answers `SinDatos`) — rendered and
    /// re-parsed through the real read edge, like the submission half.
    ///
    /// # Panics
    /// If the script lock is poisoned.
    pub fn push_consulta(&self, spec: crate::fiscal::consulta::ConsultaSpec) {
        self.consulta_script
            .lock()
            .expect("fake transport script lock")
            .push_back(spec);
    }

    fn consulta_answer(spec: &crate::fiscal::consulta::ConsultaSpec) -> TransportOutcome {
        let doc = crate::fiscal::consulta::consulta_response_document(spec);
        TransportOutcome::Consulta(
            crate::fiscal::consulta::parse_consulta(&doc)
                .expect("the consulta document the fake built must parse (bug if not)"),
        )
    }

    fn dry_consulta_answer() -> TransportOutcome {
        Self::consulta_answer(&crate::fiscal::consulta::ConsultaSpec {
            obligado_nombre: String::new(),
            obligado_nif: String::new(),
            apoderado: false,
            ejercicio: 1970,
            mes: crate::fiscal::consulta::Mes::try_new(1).expect("enero"),
            resultado: crate::fiscal::consulta::ResultadoConsulta::SinDatos,
            paginacion_pendiente: false,
            clave_paginacion: None,
            registros: Vec::new(),
        })
    }

    /// # Panics
    /// If the state lock is poisoned.
    #[must_use]
    pub fn sent_envelopes(&self) -> Vec<String> {
        self.sent.lock().expect("fake transport state lock").clone()
    }

    /// # Panics
    /// If the state lock is poisoned.
    #[must_use]
    pub fn send_count(&self) -> usize {
        self.sent.lock().expect("fake transport state lock").len()
    }

    fn line_for(scripted: &ScriptedLine, record: &SentRecord) -> LineaSpec {
        let (estado, codigo, descripcion, duplicado) = match scripted {
            ScriptedLine::Correcto => (EstadoRegistro::Correcto, None, None, None),
            ScriptedLine::AceptadoConErrores {
                codigo,
                descripcion,
            } => (
                EstadoRegistro::AceptadoConErrores,
                Some(*codigo),
                Some(descripcion.clone()),
                None,
            ),
            ScriptedLine::Incorrecto {
                codigo,
                descripcion,
            } => (
                EstadoRegistro::Incorrecto,
                Some(*codigo),
                Some(descripcion.clone()),
                None,
            ),
            ScriptedLine::Duplicado {
                codigo,
                id_peticion,
                estado_duplicado,
            } => (
                EstadoRegistro::Incorrecto,
                Some(*codigo),
                Some(String::from("registro duplicado")),
                Some(DuplicadoSpec {
                    id_peticion: id_peticion.clone(),
                    estado_duplicado: *estado_duplicado,
                    codigo_error: None,
                    descripcion_error: None,
                }),
            ),
        };
        LineaSpec {
            nif: record.nif.clone(),
            num_serie: record.num_serie.clone(),
            fecha: record.fecha.clone(),
            tipo_operacion: record.tipo_operacion,
            // The echo answers the record's own RefExterna back — real
            // AEAT does (RS.xsd echoes it), so the fake must too.
            ref_externa: record.ref_externa.clone(),
            estado_registro: estado,
            codigo_error: codigo,
            descripcion_error: descripcion,
            duplicado,
        }
    }

    /// Pads with `Correcto` when the script runs short — the echo must
    /// cover every record.
    fn response_document_for(
        soap_envelope: &str,
        csv: Option<&str>,
        tiempo_espera_envio: u32,
        estado_envio: EstadoEnvio,
        lineas: &[ScriptedLine],
    ) -> Result<String, TransportFailure> {
        let request = sent_request(soap_envelope).map_err(|detail| TransportFailure {
            detail: format!("fake transport could not parse the sent envelope: {detail}"),
        })?;
        // Over-scripting is a test bug: `zip` would silently drop the
        // surplus.
        debug_assert!(
            lineas.len() <= request.records.len(),
            "fake transport scripted {} line outcomes for {} sent records — over-scripted",
            lineas.len(),
            request.records.len()
        );
        let padded = lineas
            .iter()
            .chain(std::iter::repeat(&ScriptedLine::Correcto));
        let spec = RespuestaSpec {
            obligado_nombre: &request.obligado_nombre,
            obligado_nif: &request.obligado_nif,
            csv,
            tiempo_espera_envio,
            estado_envio,
            lineas: request
                .records
                .iter()
                .zip(padded)
                .map(|(record, scripted)| Self::line_for(scripted, record))
                .collect(),
        };
        Ok(respuesta_document(&spec))
    }

    /// Parsed back through the real read path — the fake never drifts
    /// from it.
    fn fault_answer(code: &str, faultstring: &str) -> TransportOutcome {
        let doc = fault_document(code, faultstring);
        TransportOutcome::Fault(
            parse_fault(&doc).expect("the fault document the fake built must parse (bug if not)"),
        )
    }

    fn response_answer(
        soap_envelope: &str,
        csv: Option<&str>,
        tiempo_espera_envio: u32,
        estado_envio: EstadoEnvio,
        lineas: &[ScriptedLine],
    ) -> TransportOutcome {
        let doc = Self::response_document_for(
            soap_envelope,
            csv,
            tiempo_espera_envio,
            estado_envio,
            lineas,
        )
        .expect("the document builds from our own envelope");
        TransportOutcome::Response(
            parse_respuesta(&doc)
                .expect("the response document the fake built must parse (bug if not)"),
        )
    }

    /// The wholesale envío-level rejection document: `EstadoEnvio=
    /// Incorrecto`, ZERO `RespuestaLinea` blocks (the obligado is still
    /// parsed so the document stays RS.xsd-shaped). Deliberately NOT
    /// `response_answer` — that helper pads short scripts with
    /// `Correcto` lines, and this shape must carry none.
    fn envio_incorrecto_answer(
        soap_envelope: &str,
        tiempo_espera_envio: u32,
    ) -> Result<TransportOutcome, TransportFailure> {
        let request = sent_request(soap_envelope).map_err(|detail| TransportFailure {
            detail: format!("fake transport could not parse the sent envelope: {detail}"),
        })?;
        let spec = RespuestaSpec {
            obligado_nombre: &request.obligado_nombre,
            obligado_nif: &request.obligado_nif,
            csv: None,
            tiempo_espera_envio,
            estado_envio: EstadoEnvio::Incorrecto,
            lineas: Vec::new(),
        };
        let doc = respuesta_document(&spec);
        Ok(TransportOutcome::Response(parse_respuesta(&doc).expect(
            "the response document the fake built must parse (bug if not)",
        )))
    }
}

impl VerifactuTransport for FakeVerifactuTransport {
    fn send<'a>(&'a self, soap_envelope: &'a str) -> SendFuture<'a> {
        // Record verbatim first: even a scripted failure is a send.
        self.sent
            .lock()
            .expect("fake transport state lock")
            .push(soap_envelope.to_owned());
        // Consulta envelopes answer the consulta script; the submission
        // echo below is submission-shaped and would refuse them. Route
        // on the PARSED payload's name — a submission envelope may
        // legally carry that literal string inside user text.
        let is_consulta = crate::fiscal::xml::parse_tree(soap_envelope)
            .ok()
            .and_then(|root| {
                root.child("Body").map(|body| {
                    body.children
                        .iter()
                        .any(|node| node.local == "ConsultaFactuSistemaFacturacion")
                })
            })
            .unwrap_or(false);
        let answer = if is_consulta {
            let spec = self
                .consulta_script
                .lock()
                .expect("fake transport script lock")
                .pop_front();
            match spec {
                Some(spec) => Ok(Self::consulta_answer(&spec)),
                None => Ok(Self::dry_consulta_answer()),
            }
        } else {
            let outcome = self
                .script
                .lock()
                .expect("fake transport script lock")
                .pop_front()
                .unwrap_or(ScriptedOutcome::Correcto {
                    csv: None,
                    tiempo_espera_envio: 60,
                });
            match outcome {
                ScriptedOutcome::Unreachable => Err(TransportFailure {
                    detail: String::from("fake transport: scripted unreachable"),
                }),
                ScriptedOutcome::FaultServer(string) => {
                    Ok(Self::fault_answer("soapenv:Server", &string))
                }
                ScriptedOutcome::FaultClient(string) => {
                    Ok(Self::fault_answer("soapenv:Client", &string))
                }
                ScriptedOutcome::Correcto {
                    csv,
                    tiempo_espera_envio,
                } => Ok(Self::response_answer(
                    soap_envelope,
                    csv.as_deref(),
                    tiempo_espera_envio,
                    EstadoEnvio::Correcto,
                    &[],
                )),
                ScriptedOutcome::Lineas {
                    csv,
                    tiempo_espera_envio,
                    estado_envio,
                    lineas,
                } => Ok(Self::response_answer(
                    soap_envelope,
                    csv.as_deref(),
                    tiempo_espera_envio,
                    estado_envio,
                    &lineas,
                )),
                ScriptedOutcome::EnvioIncorrecto {
                    tiempo_espera_envio,
                } => Ok(
                    Self::envio_incorrecto_answer(soap_envelope, tiempo_espera_envio)
                        .expect("the wholesale-rejection document builds from our own envelope"),
                ),
            }
        };
        Box::pin(std::future::ready(answer))
    }
}
