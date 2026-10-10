//! The transport port and its stateful fake. Endpoint family:
//! `sfVerifactu` (remission) and `sfRequerimiento` (conservation bajo
//! requerimiento) — same XSDs, different endpoints; selection is the
//! caller's configuration.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use crate::fiscal::consulta::{
    consulta_of_tree, consulta_response_document, ConsultaSpec, Mes, ResultadoConsulta,
};
use crate::fiscal::response::{
    fault_of_tree, respuesta_of_tree, EstadoDuplicado, EstadoEnvio, EstadoRegistro,
    RespuestaSuministro, SoapFault, TipoOperacion,
};
use crate::fiscal::xml::{
    parse_tree, sent_request, tag, SentRecord, Tree, NS_RESPUESTA, NS_SOAP_ENV, NS_SUMINISTRO,
    P_ENV, P_RS,
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

/// The wire leg failed (unreachable, timeout, refused, unparseable
/// answer) — always the `Transient` class: the caller retries.
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

/// The production wire's read edge: a SOAP envelope whose `Body`
/// carries a `Fault` (any HTTP status) or either respuesta, or that
/// payload as a bare root — the fake's documents parse through this same
/// edge, so one read law holds offline and live.
///
/// # Errors
/// A prose `Err` when the document carries none of those shapes — the
/// `Transient` class: an unparseable answer is surfaced, never guessed
/// at.
pub fn parse_soap_answer(xml: &str) -> Result<TransportOutcome, String> {
    const PAYLOADS: [&str; 3] = [
        "Fault",
        "RespuestaRegFactuSistemaFacturacion",
        "RespuestaConsultaFactuSistemaFacturacion",
    ];
    let root = parse_tree(xml)?;
    let payload = if root.local == "Envelope" {
        root.child("Body")
            .ok_or("SOAP envelope without Body")?
            .children
            .iter()
            .find(|node| PAYLOADS.contains(&node.local.as_str()))
            .ok_or("SOAP Body carries neither a Fault nor a respuesta")?
    } else {
        &root
    };
    match payload.local.as_str() {
        "Fault" => Ok(TransportOutcome::Fault(fault_of_tree(payload)?)),
        "RespuestaRegFactuSistemaFacturacion" => {
            Ok(TransportOutcome::Response(respuesta_of_tree(payload)?))
        }
        "RespuestaConsultaFactuSistemaFacturacion" => {
            Ok(TransportOutcome::Consulta(consulta_of_tree(payload)?))
        }
        other => Err(format!(
            "root element is {other}, not a SOAP envelope, Fault, or either respuesta"
        )),
    }
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
    /// shape needs its own variant).
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

/// Answers every envío through the real read edge: a dry submission
/// script answers a plain `Correcto` echo of the records sent, a dry
/// consulta script answers `SinDatos`.
#[derive(Debug, Default)]
pub struct FakeVerifactuTransport {
    sent: Mutex<Vec<String>>,
    script: Mutex<VecDeque<ScriptedOutcome>>,
    consulta_script: Mutex<VecDeque<ConsultaSpec>>,
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
    /// the next spec.
    ///
    /// # Panics
    /// If the script lock is poisoned.
    pub fn push_consulta(&self, spec: ConsultaSpec) {
        self.consulta_script
            .lock()
            .expect("fake transport script lock")
            .push_back(spec);
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

    /// Every scripted document parses back through
    /// [`parse_soap_answer`] — the fake never drifts from the real read
    /// edge.
    fn read_back(document: &str) -> TransportOutcome {
        parse_soap_answer(document)
            .expect("the document the fake built parses through the real read edge (bug if not)")
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
            // Real AEAT echoes the record's own RefExterna (RS.xsd).
            ref_externa: record.ref_externa.clone(),
            estado_registro: estado,
            codigo_error: codigo,
            descripcion_error: descripcion,
            duplicado,
        }
    }

    /// The respuesta echoing every record `soap_envelope` carried: one
    /// line per record, the script's lines first and `Correcto` padding
    /// the rest — or, for a wholesale rejection (`lineas: None`), no
    /// lines at all.
    fn respuesta_for(
        soap_envelope: &str,
        csv: Option<&str>,
        tiempo_espera_envio: u32,
        estado_envio: EstadoEnvio,
        lineas: Option<&[ScriptedLine]>,
    ) -> TransportOutcome {
        let request = sent_request(soap_envelope)
            .expect("the fake parses the submission envelope it was sent (bug if not)");
        let lineas = lineas.map_or_else(Vec::new, |lineas| {
            // Over-scripting is a test bug: `zip` would silently drop
            // the surplus.
            assert!(
                lineas.len() <= request.records.len(),
                "fake transport scripted {} line outcomes for {} sent records",
                lineas.len(),
                request.records.len()
            );
            let padded = lineas
                .iter()
                .chain(std::iter::repeat(&ScriptedLine::Correcto));
            request
                .records
                .iter()
                .zip(padded)
                .map(|(record, scripted)| Self::line_for(scripted, record))
                .collect()
        });
        Self::read_back(&respuesta_document(&RespuestaSpec {
            obligado_nombre: &request.obligado_nombre,
            obligado_nif: &request.obligado_nif,
            csv,
            tiempo_espera_envio,
            estado_envio,
            lineas,
        }))
    }

    /// The next scripted consulta page — or, on a dry script, a
    /// `SinDatos` page echoing the request's own cabecera and period.
    fn consulta_answer(&self, request: &Tree) -> TransportOutcome {
        let scripted = self
            .consulta_script
            .lock()
            .expect("fake transport script lock")
            .pop_front();
        let spec = scripted.unwrap_or_else(|| {
            let text = |path: &[&str]| {
                path.iter()
                    .try_fold(request, |node, local| node.child(local))
                    .map(|node| node.text.clone())
                    .unwrap_or_default()
            };
            ConsultaSpec {
                obligado_nombre: text(&["Cabecera", "ObligadoEmision", "NombreRazon"]),
                obligado_nif: text(&["Cabecera", "ObligadoEmision", "NIF"]),
                apoderado: text(&["Cabecera", "IndicadorRepresentante"]) == "S",
                ejercicio: text(&["FiltroConsulta", "PeriodoImputacion", "Ejercicio"])
                    .parse()
                    .unwrap_or_default(),
                mes: text(&["FiltroConsulta", "PeriodoImputacion", "Periodo"])
                    .parse()
                    .ok()
                    .and_then(|mes| Mes::try_new(mes).ok())
                    .unwrap_or(Mes::ENERO),
                resultado: ResultadoConsulta::SinDatos,
                paginacion_pendiente: false,
                clave_paginacion: None,
                registros: Vec::new(),
            }
        });
        Self::read_back(&consulta_response_document(&spec))
    }

    fn submission_answer(&self, soap_envelope: &str) -> Result<TransportOutcome, TransportFailure> {
        let outcome = self
            .script
            .lock()
            .expect("fake transport script lock")
            .pop_front()
            .unwrap_or(ScriptedOutcome::Correcto {
                csv: None,
                tiempo_espera_envio: 60,
            });
        Ok(match outcome {
            ScriptedOutcome::Unreachable => {
                return Err(TransportFailure {
                    detail: String::from("fake transport: scripted unreachable"),
                })
            }
            ScriptedOutcome::FaultServer(string) => {
                Self::read_back(&fault_document("soapenv:Server", &string))
            }
            ScriptedOutcome::FaultClient(string) => {
                Self::read_back(&fault_document("soapenv:Client", &string))
            }
            ScriptedOutcome::Correcto {
                csv,
                tiempo_espera_envio,
            } => Self::respuesta_for(
                soap_envelope,
                csv.as_deref(),
                tiempo_espera_envio,
                EstadoEnvio::Correcto,
                Some(&[]),
            ),
            ScriptedOutcome::Lineas {
                csv,
                tiempo_espera_envio,
                estado_envio,
                lineas,
            } => Self::respuesta_for(
                soap_envelope,
                csv.as_deref(),
                tiempo_espera_envio,
                estado_envio,
                Some(&lineas),
            ),
            ScriptedOutcome::EnvioIncorrecto {
                tiempo_espera_envio,
            } => Self::respuesta_for(
                soap_envelope,
                None,
                tiempo_espera_envio,
                EstadoEnvio::Incorrecto,
                None,
            ),
        })
    }
}

impl VerifactuTransport for FakeVerifactuTransport {
    fn send<'a>(&'a self, soap_envelope: &'a str) -> SendFuture<'a> {
        // Record verbatim first: even a scripted failure is a send.
        self.sent
            .lock()
            .expect("fake transport state lock")
            .push(soap_envelope.to_owned());
        // Route on the PARSED payload's name — a submission envelope may
        // legally carry that literal string inside user text.
        let root = parse_tree(soap_envelope).ok();
        let consulta = root
            .as_ref()
            .and_then(|root| root.child("Body"))
            .and_then(|body| body.child("ConsultaFactuSistemaFacturacion"));
        let answer = match consulta {
            Some(request) => Ok(self.consulta_answer(request)),
            None => self.submission_answer(soap_envelope),
        };
        Box::pin(std::future::ready(answer))
    }
}
