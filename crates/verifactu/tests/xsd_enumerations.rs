//! `wire()` spellings must be exactly the SI.xsd enumerations — every
//! variant, not just the ones the live bench's matrix exercises.

use strum::IntoEnumIterator;

fn xsd_enumerations(simple_type: &str) -> Vec<String> {
    let xsd_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/aeat-verifactu/SuministroInformacion.xsd");
    let xsd = std::fs::read_to_string(&xsd_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", xsd_path.display()));
    let open = format!("<simpleType name=\"{simple_type}\">");
    let start = xsd
        .find(&open)
        .unwrap_or_else(|| panic!("{simple_type} not in SI.xsd"));
    let end = xsd[start..]
        .find("</simpleType>")
        .expect("unterminated simpleType")
        + start;
    let block = &xsd[start..end];
    let mut values: Vec<String> = block
        .split("<enumeration value=\"")
        .skip(1)
        .map(|rest| rest.split('"').next().expect("unquoted value").to_owned())
        .collect();
    values.sort();
    values
}

#[test]
fn emission_vocabularies_wire_spellings_are_the_xsd_enumerations() {
    let cases: [(&str, Vec<&str>); 6] = [
        (
            "ImpuestoType",
            verifactu::Impuesto::iter()
                .map(verifactu::Impuesto::wire)
                .collect(),
        ),
        (
            "IdOperacionesTrascendenciaTributariaType",
            verifactu::ClaveRegimen::iter()
                .map(verifactu::ClaveRegimen::wire)
                .collect(),
        ),
        (
            "CalificacionOperacionType",
            verifactu::CalificacionOperacion::iter()
                .map(verifactu::CalificacionOperacion::wire)
                .collect(),
        ),
        (
            "OperacionExentaType",
            verifactu::OperacionExenta::iter()
                .map(verifactu::OperacionExenta::wire)
                .collect(),
        ),
        (
            "RechazoPrevioType",
            verifactu::engine::RechazoPrevio::iter()
                .map(verifactu::engine::RechazoPrevio::wire)
                .collect(),
        ),
        (
            "PersonaFisicaJuridicaIDTypeType",
            verifactu::IdOtroType::iter()
                .map(verifactu::IdOtroType::wire)
                .collect(),
        ),
    ];
    for (simple_type, wires) in cases {
        let mut wires: Vec<String> = wires.into_iter().map(str::to_owned).collect();
        wires.sort();
        assert_eq!(
            wires,
            xsd_enumerations(simple_type),
            "{simple_type}: wire() spellings must be the SI.xsd enumeration set"
        );
    }
}

/// `parse` is the exact inverse of `wire` over every closed vocabulary
/// — the table a JSON/CLI consumer leans on instead of hand-mirroring.
#[test]
fn vocabularies_parse_back_their_wire_spellings() {
    use verifactu::{CalificacionOperacion, ClaveRegimen, IdOtroType, Impuesto, OperacionExenta};

    for clave in ClaveRegimen::iter() {
        assert_eq!(ClaveRegimen::parse(clave.wire()), Some(clave));
    }
    for impuesto in Impuesto::iter() {
        assert_eq!(Impuesto::parse(impuesto.wire()), Some(impuesto));
    }
    for calificacion in CalificacionOperacion::iter() {
        assert_eq!(
            CalificacionOperacion::parse(calificacion.wire()),
            Some(calificacion)
        );
    }
    for exenta in OperacionExenta::iter() {
        assert_eq!(OperacionExenta::parse(exenta.wire()), Some(exenta));
    }
    for id_type in IdOtroType::iter() {
        assert_eq!(IdOtroType::parse(id_type.wire()), Some(id_type));
    }
    for (tipo, code) in [
        (verifactu::TipoRectificativa::Sustitutiva, "S"),
        (verifactu::TipoRectificativa::Incremental, "I"),
    ] {
        assert_eq!(verifactu::TipoRectificativa::parse(code), Some(tipo));
    }

    // The gaps parse to None — a typo is a None, never a wrong variant.
    assert_eq!(
        ClaveRegimen::parse("12"),
        None,
        "12 is not in the closed set"
    );
    assert_eq!(
        ClaveRegimen::parse("13"),
        None,
        "13 is not in the closed set"
    );
    assert_eq!(
        ClaveRegimen::parse("16"),
        None,
        "16 is not in the closed set"
    );
    assert_eq!(Impuesto::parse("04"), None);
    assert_eq!(CalificacionOperacion::parse("S3"), None);
}
