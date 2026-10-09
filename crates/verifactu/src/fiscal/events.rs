//! Event records: the `TipoEvento` vocabulary AS DATA and the event
//! constructors. The vocabulary is NOT public law — it is the incumbent
//! L1E set recovered from a decompile, kept as a const list and enforced
//! at the emit boundary. INTERPRETATION: the vocabulary names; the
//! numbering-gap decision (a consumed-but-uncommitted number IS an
//! integrity anomaly); the motivo's `MotivoAnomalia` rendering.

use crate::domain::chain::EventData;

pub(crate) const TIPOS_EVENTO: [(&str, &str); 11] = [
    (
        "INICIO_NO_VERIFACTU",
        "la instalación empieza a operar en modalidad no VERI*FACTU",
    ),
    (
        "FIN_NO_VERIFACTU",
        "la instalación deja de operar en modalidad no VERI*FACTU",
    ),
    (
        "LANZAMIENTO_DETECCION_ANOMALIAS_FACTURACION",
        "se lanza la detección de anomalías de facturación",
    ),
    (
        "DETECCION_ANOMALIAS_FACTURACION",
        "se detecta una anomalía de facturación (p. ej. hueco de numeración)",
    ),
    (
        "LANZAMIENTO_DETECCION_ANOMALIAS_EVENTO",
        "se lanza la detección de anomalías de eventos",
    ),
    (
        "DETECCION_ANOMALIAS_EVENTO",
        "se detecta una anomalía de eventos",
    ),
    (
        "RESTAURACION_COPIA_SEGURIDAD",
        "se restaura desde una copia de seguridad",
    ),
    (
        "EXPORTACION_REGISTROS_FACTURACION",
        "se exportan registros de facturación",
    ),
    (
        "EXPORTACION_REGISTROS_EVENTO",
        "se exportan registros de eventos",
    ),
    ("REGISTRO_RESUMEN_EVENTOS", "resumen periódico de eventos"),
    ("OTROS_EVENTOS_VOLUNTARIOS", "otros eventos voluntarios"),
];

pub(crate) const RESTAURACION_COPIA_SEGURIDAD: &str = "RESTAURACION_COPIA_SEGURIDAD";

/// A consumed-but-uncommitted number IS an integrity anomaly (decided).
pub(crate) const DETECCION_ANOMALIAS_FACTURACION: &str = "DETECCION_ANOMALIAS_FACTURACION";

/// The hashed subset of the emitter-level `SistemaInformaticoConfig`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventoSif {
    /// Empty when `id` is the informed identity.
    pub nif: String,
    /// Empty when `nif` is informed.
    pub id: String,
    pub id_sistema_informatico: String,
    pub version: String,
    pub numero_instalacion: String,
}

/// The seal adds instants/chaining and enforces HS §3's `NIF`/`ID`
/// exclusivity.
#[must_use]
pub fn evento_data(sif: &EventoSif, nif_obligado: &str, tipo_evento: &str) -> EventData {
    EventData {
        nif: sif.nif.clone(),
        id: sif.id.clone(),
        id_sistema_informatico: sif.id_sistema_informatico.clone(),
        version: sif.version.clone(),
        numero_instalacion: sif.numero_instalacion.clone(),
        nif_obligado: nif_obligado.to_owned(),
        tipo_evento: tipo_evento.to_owned(),
    }
}

#[must_use]
pub fn evento_restore(sif: &EventoSif, nif_obligado: &str) -> EventData {
    evento_data(sif, nif_obligado, RESTAURACION_COPIA_SEGURIDAD)
}

/// The `motivo` rides the emission context as `MotivoAnomalia` — NOT a
/// hashed field.
#[must_use]
pub fn evento_numbering_gap(sif: &EventoSif, nif_obligado: &str) -> EventData {
    evento_data(sif, nif_obligado, DETECCION_ANOMALIAS_FACTURACION)
}

/// Unknown tipos are rejected upstream of the hash — a typo can never
/// enter the event chain.
#[must_use]
pub(crate) fn is_known_tipo_evento(tipo: &str) -> bool {
    TIPOS_EVENTO.iter().any(|(code, _)| *code == tipo)
}
