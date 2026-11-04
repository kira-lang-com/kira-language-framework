use tvisa_format::DataKind;

#[derive(Clone, Copy)]
pub(super) enum TargetKind {
    Data(DataKind),
    DataImport,
    DefinedFunction {
        gpu: bool,
        address_taken: bool,
        tag: Option<u32>,
    },
    ImportedFunction {
        tag: Option<u32>,
    },
    VariantSet,
    DispatchSet,
    GpuEntry,
}
