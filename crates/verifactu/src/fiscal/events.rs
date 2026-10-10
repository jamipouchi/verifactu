//! Event records: the `TipoEvento` vocabulary AS DATA and the event
//! constructors. AEAT publishes no event XSD, so the vocabulary is an
//! INTERPRETATION kept as a closed list and enforced at the emit
//! boundary — as are the numbering-gap decision (a
//! consumed-but-uncommitted number IS an integrity anomaly) and the
//! motivo's `MotivoAnomalia` rendering.

use crate::domain::chain::EventData;

const TIPOS_EVENTO: [&str; 11] = [
    // la instalación empieza / deja de operar en modalidad no VERI*FACTU
    "INICIO_NO_VERIFACTU",
    "FIN_NO_VERIFACTU",
    // se lanza / se detecta (p. ej. un hueco de numeración) la detección
    // de anomalías de facturación
    "LANZAMIENTO_DETECCION_ANOMALIAS_FACTURACION",
    DETECCION_ANOMALIAS_FACTURACION,
    // ...y de anomalías de eventos
    "LANZAMIENTO_DETECCION_ANOMALIAS_EVENTO",
    "DETECCION_ANOMALIAS_EVENTO",
    // se restaura desde una copia de seguridad
    RESTAURACION_COPIA_SEGURIDAD,
    // se exportan registros de facturación / de eventos
    "EXPORTACION_REGISTROS_FACTURACION",
    "EXPORTACION_REGISTROS_EVENTO",
    // resumen periódico de eventos; otros eventos voluntarios
    "REGISTRO_RESUMEN_EVENTOS",
    "OTROS_EVENTOS_VOLUNTARIOS",
];

const RESTAURACION_COPIA_SEGURIDAD: &str = "RESTAURACION_COPIA_SEGURIDAD";

/// A consumed-but-uncommitted number IS an integrity anomaly (decided).
const DETECCION_ANOMALIAS_FACTURACION: &str = "DETECCION_ANOMALIAS_FACTURACION";

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
    TIPOS_EVENTO.contains(&tipo)
}
