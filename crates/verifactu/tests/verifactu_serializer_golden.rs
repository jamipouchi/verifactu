//! Golden vectors for the event-record wire shapes — no live authority
//! can answer them.

use verifactu::engine::xml;
use verifactu::engine::EmissionContext;

fn instante(value: &str) -> verifactu::FechaHuso {
    verifactu::FechaHuso::parse(value)
        .expect("the fixture renders the ISO-8601-with-offset grammar")
}

/// Pins the UNSIGNED signing unit; the wrapper embeds it exactly as the
/// emitter would after signing.
#[test]
fn golden_registro_evento_numbering_gap() {
    let sif = verifactu::engine::events::EventoSif {
        nif: String::from("89890001K"),
        id: String::new(),
        id_sistema_informatico: String::from("77"),
        version: String::from("1.0.03"),
        numero_instalacion: String::from("383"),
    };
    let primer = verifactu::engine::ChainRecord::seal(
        verifactu::ChainKind::Evento {
            event: verifactu::engine::events::evento_restore(&sif, "89890001K"),
            fecha_huso_gen_evento: instante("2025-02-05T08:00:00+01:00"),
        },
        1_738_745_600,
        None,
    );
    let record = verifactu::engine::ChainRecord::seal(
        verifactu::ChainKind::Evento {
            event: verifactu::engine::events::evento_numbering_gap(&sif, "89890001K"),
            fecha_huso_gen_evento: instante("2025-02-05T08:05:00+01:00"),
        },
        1_738_745_900,
        Some(&primer.predecessor()),
    );
    let ctx = EmissionContext {
        motivo_evento: Some("hueco de numeración: T00000043 consumido no remitido"),
        ..EmissionContext::default()
    };
    let signed_unit = xml::evento_node(&record, &ctx).expect("the evento serializes");
    insta::assert_snapshot!("evento_node_numbering_gap", signed_unit);
    let wire = xml::registro_evento(&signed_unit);
    insta::assert_snapshot!("registro_evento_numbering_gap", wire);
}
