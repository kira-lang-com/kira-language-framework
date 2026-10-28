use klf_defsvc::{DefinitionServiceBridge, ServiceSink};
use klf_facts::{AnalysisMode, Entity, ReferenceRole, TokenClass};
use klf_rt::ServiceMemory;
use klf_services::{DefinitionPhase, DefinitionPhaseArguments, DefinitionUnit, Session};

const SOURCE: &str = "function area(r) { return r }";

struct Bytes(Vec<u8>);

impl ServiceMemory for Bytes {
    fn read_buffer(&self, address: u64, destination: &mut [u8]) -> bool {
        let start = usize::try_from(address).unwrap_or(usize::MAX);
        self.0
            .get(start..start + destination.len())
            .is_some_and(|bytes| {
                destination.copy_from_slice(bytes);
                true
            })
    }

    fn write_buffer(&mut self, _: u64, _: &[u8]) -> bool {
        false
    }
}

fn record(mode: AnalysisMode) -> Session {
    let mut session = Session::new("aarch64-apple-darwin".parse().expect("valid target"));
    session.set_mode(mode);
    let file = session.add_file("area.k", SOURCE);
    let unit = DefinitionUnit::new(file);
    session.register_unit(unit.clone());
    let arguments = DefinitionPhaseArguments::Parse {
        unit,
        program: Vec::new(),
    };
    let bridge =
        DefinitionServiceBridge::new(&mut session, DefinitionPhase::Parse, &arguments, false)
            .expect("bridge opens");
    let mut sink = ServiceSink::new();
    bridge.register(&mut sink);
    let table = sink.into_table();
    let mut memory = Bytes(b"(r: Float) -> Float".to_vec());
    let mut call =
        |name: &str, arguments: &[u64]| table[name].call_with_memory(arguments, &mut memory);
    let current = u64::MAX;
    let keyword = u64::from(TokenClass::Keyword.code());
    assert_eq!(
        call("klfsvc_editor_classify", &[current, 0, 8, keyword, 0]),
        u64::from(mode == AnalysisMode::Editor)
    );
    call(
        "klfsvc_editor_symbol",
        &[7, current, 0, 29, 9, 13, 11, u64::MAX],
    );
    call("klfsvc_editor_symbol_text", &[7, 1, 0, 19]);
    call(
        "klfsvc_editor_reference",
        &[current, 26, 27, 8, u64::from(ReferenceRole::Read.code())],
    );
    assert_eq!(
        call("klfsvc_editor_classify", &[current, 20, 99, keyword, 0]),
        0,
        "a span past the end is refused"
    );
    assert_eq!(
        call("klfsvc_editor_mode", &[]),
        u64::from(mode == AnalysisMode::Editor)
    );
    bridge.commit_records(&mut session).expect("records commit");
    session
}

#[test]
fn editor_mode_records_facts_into_the_session() {
    let session = record(AnalysisMode::Editor);
    let facts = session.facts();
    let file = session.files()[0].id();
    let symbol = facts.symbol(Entity(7)).expect("symbol declared");
    assert_eq!(symbol.name, "area");
    assert_eq!(symbol.signature, "(r: Float) -> Float");
    assert_eq!(facts.tokens(file).len(), 1);
    assert_eq!(
        facts.entity_at(file, 26).map(|(_, entity)| entity),
        Some(Entity(8))
    );
}

#[test]
fn build_mode_records_nothing() {
    assert!(record(AnalysisMode::Build).facts().is_empty());
}
